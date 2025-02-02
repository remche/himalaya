use clap;
use error_chain::error_chain;
use log::{debug, error, trace};
use std::{fs, ops::Deref};

use crate::{
    app::App,
    flag::model::Flag,
    imap::model::ImapConnector,
    input,
    mbox::cli::mbox_target_arg,
    msg::model::{Attachments, Msg, Msgs, ReadableMsg},
    smtp,
};

error_chain! {
    links {
        Imap(crate::imap::model::Error, crate::imap::model::ErrorKind);
        Input(crate::input::Error, crate::input::ErrorKind);
        MsgModel(crate::msg::model::Error, crate::msg::model::ErrorKind);
        Smtp(crate::smtp::Error, crate::smtp::ErrorKind);
    }
    foreign_links {
        Utf8(std::string::FromUtf8Error);
    }
}

pub fn uid_arg<'a>() -> clap::Arg<'a, 'a> {
    clap::Arg::with_name("uid")
        .help("Specifies the targetted message")
        .value_name("UID")
        .required(true)
}

fn reply_all_arg<'a>() -> clap::Arg<'a, 'a> {
    clap::Arg::with_name("reply-all")
        .help("Includes all recipients")
        .short("a")
        .long("all")
}

fn page_size_arg<'a>() -> clap::Arg<'a, 'a> {
    clap::Arg::with_name("page-size")
        .help("Page size")
        .short("s")
        .long("size")
        .value_name("INT")
}

fn page_arg<'a>() -> clap::Arg<'a, 'a> {
    clap::Arg::with_name("page")
        .help("Page number")
        .short("p")
        .long("page")
        .value_name("INT")
        .default_value("0")
}

fn attachment_arg<'a>() -> clap::Arg<'a, 'a> {
    clap::Arg::with_name("attachments")
        .help("Adds attachment to the message")
        .short("a")
        .long("attachment")
        .value_name("PATH")
        .multiple(true)
        .takes_value(true)
}

pub fn msg_subcmds<'a>() -> Vec<clap::App<'a, 'a>> {
    vec![
        clap::SubCommand::with_name("list")
            .aliases(&["lst", "l"])
            .about("Lists all messages")
            .arg(page_size_arg())
            .arg(page_arg()),
        clap::SubCommand::with_name("search")
            .aliases(&["query", "q", "s"])
            .about("Lists messages matching the given IMAP query")
            .arg(page_size_arg())
            .arg(page_arg())
            .arg(
                clap::Arg::with_name("query")
                    .help("IMAP query (see https://tools.ietf.org/html/rfc3501#section-6.4.4)")
                    .value_name("QUERY")
                    .multiple(true)
                    .required(true),
            ),
        clap::SubCommand::with_name("write")
            .aliases(&["w"])
            .about("Writes a new message")
            .arg(attachment_arg()),
        clap::SubCommand::with_name("send")
            .about("Sends a raw message")
            .arg(clap::Arg::with_name("message").raw(true)),
        clap::SubCommand::with_name("save")
            .about("Saves a raw message")
            .arg(clap::Arg::with_name("message").raw(true)),
        clap::SubCommand::with_name("read")
            .aliases(&["r"])
            .about("Reads text bodies of a message")
            .arg(uid_arg())
            .arg(
                clap::Arg::with_name("mime-type")
                    .help("MIME type to use")
                    .short("t")
                    .long("mime-type")
                    .value_name("MIME")
                    .possible_values(&["plain", "html"])
                    .default_value("plain"),
            )
            .arg(
                clap::Arg::with_name("raw")
                    .help("Reads raw message")
                    .long("raw")
                    .short("r"),
            ),
        clap::SubCommand::with_name("attachments")
            .aliases(&["attach", "att", "a"])
            .about("Downloads all message attachments")
            .arg(uid_arg()),
        clap::SubCommand::with_name("reply")
            .aliases(&["rep", "re"])
            .about("Answers to a message")
            .arg(uid_arg())
            .arg(reply_all_arg()),
        clap::SubCommand::with_name("forward")
            .aliases(&["fwd", "f"])
            .about("Forwards a message")
            .arg(uid_arg()),
        clap::SubCommand::with_name("copy")
            .aliases(&["cp", "c"])
            .about("Copies a message to the targetted mailbox")
            .arg(uid_arg())
            .arg(mbox_target_arg()),
        clap::SubCommand::with_name("move")
            .aliases(&["mv", "m"])
            .about("Moves a message to the targetted mailbox")
            .arg(uid_arg())
            .arg(mbox_target_arg()),
        clap::SubCommand::with_name("delete")
            .aliases(&["remove", "rm", "del", "d"])
            .about("Deletes a message")
            .arg(uid_arg()),
        clap::SubCommand::with_name("template")
            .aliases(&["tpl", "t"])
            .about("Generates a message template")
            .subcommand(
                clap::SubCommand::with_name("new")
                    .aliases(&["n"])
                    .about("Generates a new message template"),
            )
            .subcommand(
                clap::SubCommand::with_name("reply")
                    .aliases(&["rep", "r"])
                    .about("Generates a reply message template")
                    .arg(uid_arg())
                    .arg(reply_all_arg()),
            )
            .subcommand(
                clap::SubCommand::with_name("forward")
                    .aliases(&["fwd", "fw", "f"])
                    .about("Generates a forward message template")
                    .arg(uid_arg()),
            ),
    ]
}

pub fn msg_matches(app: &App) -> Result<bool> {
    if let Some(matches) = app.arg_matches.subcommand_matches("list") {
        debug!("list command matched");

        let page_size: usize = matches
            .value_of("page-size")
            .and_then(|s| s.parse().ok())
            .unwrap_or(app.config.default_page_size(&app.account));
        debug!("page size: {}", &page_size);
        let page: usize = matches
            .value_of("page")
            .unwrap()
            .parse()
            .unwrap_or_default();
        debug!("page: {}", &page);

        let mut imap_conn = ImapConnector::new(&app.account)?;
        let msgs = imap_conn.list_msgs(&app.mbox, &page_size, &page)?;
        let msgs = Msgs::from(&msgs);
        trace!("messages: {:?}", msgs);
        app.output.print(msgs);

        imap_conn.logout();
        return Ok(true);
    }

    if let Some(matches) = app.arg_matches.subcommand_matches("search") {
        debug!("search command matched");

        let page_size: usize = matches
            .value_of("page-size")
            .and_then(|s| s.parse().ok())
            .unwrap_or(app.config.default_page_size(&app.account));
        debug!("page size: {}", &page_size);
        let page: usize = matches
            .value_of("page")
            .unwrap()
            .parse()
            .unwrap_or_default();
        debug!("page: {}", &page);

        let query = matches
            .values_of("query")
            .unwrap_or_default()
            .fold((false, vec![]), |(escape, mut cmds), cmd| {
                match (cmd, escape) {
                    // Next command is an arg and needs to be escaped
                    ("subject", _) | ("body", _) | ("text", _) => {
                        cmds.push(cmd.to_string());
                        (true, cmds)
                    }
                    // Escaped arg commands
                    (_, true) => {
                        cmds.push(format!("\"{}\"", cmd));
                        (false, cmds)
                    }
                    // Regular commands
                    (_, false) => {
                        cmds.push(cmd.to_string());
                        (false, cmds)
                    }
                }
            })
            .1
            .join(" ");
        debug!("query: {}", &page);

        let mut imap_conn = ImapConnector::new(&app.account)?;
        let msgs = imap_conn.search_msgs(&app.mbox, &query, &page_size, &page)?;
        let msgs = Msgs::from(&msgs);
        trace!("messages: {:?}", msgs);
        app.output.print(msgs);

        imap_conn.logout();
        return Ok(true);
    }

    if let Some(matches) = app.arg_matches.subcommand_matches("read") {
        debug!("read command matched");

        let uid = matches.value_of("uid").unwrap();
        debug!("uid: {}", uid);
        let mime = format!("text/{}", matches.value_of("mime-type").unwrap());
        debug!("mime: {}", mime);
        let raw = matches.is_present("raw");
        debug!("raw: {}", raw);

        let mut imap_conn = ImapConnector::new(&app.account)?;
        let msg = imap_conn.read_msg(&app.mbox, &uid)?;
        if raw {
            let msg = String::from_utf8(msg)
                .chain_err(|| "Could not decode raw message as utf8 string")?;
            let msg = msg.trim_end_matches("\n");
            app.output.print(msg);
        } else {
            let msg = ReadableMsg::from_bytes(&mime, &msg)?;
            app.output.print(msg);
        }

        imap_conn.logout();
        return Ok(true);
    }

    if let Some(matches) = app.arg_matches.subcommand_matches("attachments") {
        debug!("attachments command matched");

        let uid = matches.value_of("uid").unwrap();
        debug!("uid: {}", &uid);

        let mut imap_conn = ImapConnector::new(&app.account)?;
        let msg = imap_conn.read_msg(&app.mbox, &uid)?;
        let attachments = Attachments::from_bytes(&msg)?;
        debug!(
            "{} attachment(s) found for message {}",
            &attachments.0.len(),
            &uid
        );
        for attachment in attachments.0.iter() {
            let filepath = app
                .config
                .downloads_filepath(&app.account, &attachment.filename);
            debug!("downloading {}…", &attachment.filename);
            fs::write(&filepath, &attachment.raw)
                .chain_err(|| format!("Could not save attachment {:?}", filepath))?;
        }

        debug!(
            "{} attachment(s) successfully downloaded",
            &attachments.0.len()
        );
        app.output.print(format!(
            "{} attachment(s) successfully downloaded",
            &attachments.0.len()
        ));

        imap_conn.logout();
        return Ok(true);
    }

    if let Some(matches) = app.arg_matches.subcommand_matches("write") {
        debug!("write command matched");

        let mut imap_conn = ImapConnector::new(&app.account)?;
        let attachments = matches
            .values_of("attachments")
            .unwrap_or_default()
            .map(String::from)
            .collect::<Vec<_>>();
        let tpl = Msg::build_new_tpl(&app.config, &app.account)?;
        let content = input::open_editor_with_tpl(tpl.to_string().as_bytes())?;
        let mut msg = Msg::from(content);
        msg.attachments = attachments;

        loop {
            match input::post_edit_choice() {
                Ok(choice) => match choice {
                    input::PostEditChoice::Send => {
                        debug!("sending message…");
                        let msg = msg.to_sendable_msg()?;
                        smtp::send(&app.account, &msg)?;
                        imap_conn.append_msg("Sent", &msg.formatted(), &[Flag::Seen])?;
                        input::remove_draft()?;
                        app.output.print("Message successfully sent");
                        break;
                    }
                    input::PostEditChoice::Edit => {
                        let content = input::open_editor_with_draft()?;
                        msg = Msg::from(content);
                    }
                    input::PostEditChoice::LocalDraft => break,
                    input::PostEditChoice::RemoteDraft => {
                        debug!("saving to draft…");
                        imap_conn.append_msg("Drafts", &msg.to_vec()?, &[Flag::Seen])?;
                        input::remove_draft()?;
                        app.output.print("Message successfully saved to Drafts");
                        break;
                    }
                    input::PostEditChoice::Discard => {
                        input::remove_draft()?;
                        break;
                    }
                },
                Err(err) => error!("{}", err),
            }
        }
        imap_conn.logout();
        return Ok(true);
    }

    if let Some(matches) = app.arg_matches.subcommand_matches("reply") {
        debug!("reply command matched");

        let uid = matches.value_of("uid").unwrap();
        debug!("uid: {}", uid);
        let attachments = matches
            .values_of("attachments")
            .unwrap_or_default()
            .map(String::from)
            .collect::<Vec<_>>();
        debug!("found {} attachments", attachments.len());
        trace!("attachments: {:?}", attachments);

        let mut imap_conn = ImapConnector::new(&app.account)?;
        let msg = Msg::from(imap_conn.read_msg(&app.mbox, &uid)?);
        let tpl = if matches.is_present("reply-all") {
            msg.build_reply_all_tpl(&app.config, &app.account)?
        } else {
            msg.build_reply_tpl(&app.config, &app.account)?
        };

        let content = input::open_editor_with_tpl(&tpl.to_string().as_bytes())?;
        let mut msg = Msg::from(content);
        msg.attachments = attachments;

        loop {
            match input::post_edit_choice() {
                Ok(choice) => match choice {
                    input::PostEditChoice::Send => {
                        debug!("sending message…");
                        let msg = msg.to_sendable_msg()?;
                        smtp::send(&app.account, &msg)?;
                        imap_conn.append_msg("Sent", &msg.formatted(), &[Flag::Seen])?;
                        imap_conn.add_flags(&app.mbox, uid, "\\Answered")?;
                        input::remove_draft()?;
                        app.output.print("Message successfully sent");
                        break;
                    }
                    input::PostEditChoice::Edit => {
                        let content = input::open_editor_with_draft()?;
                        msg = Msg::from(content);
                    }
                    input::PostEditChoice::LocalDraft => break,
                    input::PostEditChoice::RemoteDraft => {
                        debug!("saving to draft…");
                        imap_conn.append_msg("Drafts", &msg.to_vec()?, &[Flag::Seen])?;
                        input::remove_draft()?;
                        app.output.print("Message successfully saved to Drafts");
                        break;
                    }
                    input::PostEditChoice::Discard => {
                        input::remove_draft()?;
                        break;
                    }
                },
                Err(err) => error!("{}", err),
            }
        }

        imap_conn.logout();
        return Ok(true);
    }

    if let Some(matches) = app.arg_matches.subcommand_matches("forward") {
        debug!("forward command matched");

        let uid = matches.value_of("uid").unwrap();
        debug!("uid: {}", uid);
        let attachments = matches
            .values_of("attachments")
            .unwrap_or_default()
            .map(String::from)
            .collect::<Vec<_>>();
        debug!("found {} attachments", attachments.len());
        trace!("attachments: {:?}", attachments);

        let mut imap_conn = ImapConnector::new(&app.account)?;
        let msg = Msg::from(imap_conn.read_msg(&app.mbox, &uid)?);
        let tpl = msg.build_forward_tpl(&app.config, &app.account)?;
        let content = input::open_editor_with_tpl(&tpl.to_string().as_bytes())?;
        let mut msg = Msg::from(content);
        msg.attachments = attachments;

        loop {
            match input::post_edit_choice() {
                Ok(choice) => match choice {
                    input::PostEditChoice::Send => {
                        debug!("sending message…");
                        let msg = msg.to_sendable_msg()?;
                        smtp::send(&app.account, &msg)?;
                        imap_conn.append_msg("Sent", &msg.formatted(), &[Flag::Seen])?;
                        input::remove_draft()?;
                        app.output.print("Message successfully sent");
                        break;
                    }
                    input::PostEditChoice::Edit => {
                        let content = input::open_editor_with_draft()?;
                        msg = Msg::from(content);
                    }
                    input::PostEditChoice::LocalDraft => break,
                    input::PostEditChoice::RemoteDraft => {
                        debug!("saving to draft…");
                        imap_conn.append_msg("Drafts", &msg.to_vec()?, &[Flag::Seen])?;
                        input::remove_draft()?;
                        app.output.print("Message successfully saved to Drafts");
                        break;
                    }
                    input::PostEditChoice::Discard => {
                        input::remove_draft()?;
                        break;
                    }
                },
                Err(err) => error!("{}", err),
            }
        }

        imap_conn.logout();
        return Ok(true);
    }

    if let Some(matches) = app.arg_matches.subcommand_matches("template") {
        debug!("template command matched");

        if let Some(_) = matches.subcommand_matches("new") {
            debug!("new command matched");
            let tpl = Msg::build_new_tpl(&app.config, &app.account)?;
            trace!("tpl: {:?}", tpl);
            app.output.print(tpl);
        }

        if let Some(matches) = matches.subcommand_matches("reply") {
            debug!("reply command matched");

            let uid = matches.value_of("uid").unwrap();
            debug!("uid: {}", uid);

            let mut imap_conn = ImapConnector::new(&app.account)?;
            let msg = Msg::from(imap_conn.read_msg(&app.mbox, &uid)?);
            let tpl = if matches.is_present("reply-all") {
                msg.build_reply_all_tpl(&app.config, &app.account)?
            } else {
                msg.build_reply_tpl(&app.config, &app.account)?
            };
            trace!("tpl: {:?}", tpl);
            app.output.print(tpl);

            imap_conn.logout();
        }

        if let Some(matches) = matches.subcommand_matches("forward") {
            debug!("forward command matched");

            let uid = matches.value_of("uid").unwrap();
            debug!("uid: {}", uid);

            let mut imap_conn = ImapConnector::new(&app.account)?;
            let msg = Msg::from(imap_conn.read_msg(&app.mbox, &uid)?);
            let tpl = msg.build_forward_tpl(&app.config, &app.account)?;
            trace!("tpl: {:?}", tpl);
            app.output.print(tpl);

            imap_conn.logout();
        }

        return Ok(true);
    }

    if let Some(matches) = app.arg_matches.subcommand_matches("copy") {
        debug!("copy command matched");

        let uid = matches.value_of("uid").unwrap();
        debug!("uid: {}", &uid);
        let target = matches.value_of("target").unwrap();
        debug!("target: {}", &target);

        let mut imap_conn = ImapConnector::new(&app.account)?;
        let msg = Msg::from(imap_conn.read_msg(&app.mbox, &uid)?);
        let mut flags = msg.flags.deref().to_vec();
        flags.push(Flag::Seen);
        imap_conn.append_msg(target, &msg.raw, &flags)?;
        debug!("message {} successfully copied to folder `{}`", uid, target);
        app.output.print(format!(
            "Message {} successfully copied to folder `{}`",
            uid, target
        ));

        imap_conn.logout();
        return Ok(true);
    }

    if let Some(matches) = app.arg_matches.subcommand_matches("move") {
        debug!("move command matched");

        let uid = matches.value_of("uid").unwrap();
        debug!("uid: {}", &uid);
        let target = matches.value_of("target").unwrap();
        debug!("target: {}", &target);

        let mut imap_conn = ImapConnector::new(&app.account)?;
        let msg = Msg::from(imap_conn.read_msg(&app.mbox, &uid)?);
        let mut flags = msg.flags.deref().to_vec();
        flags.push(Flag::Seen);
        imap_conn.append_msg(target, &msg.raw, msg.flags.deref())?;
        imap_conn.add_flags(&app.mbox, uid, "\\Seen \\Deleted")?;
        debug!("message {} successfully moved to folder `{}`", uid, target);
        app.output.print(format!(
            "Message {} successfully moved to folder `{}`",
            uid, target
        ));

        imap_conn.expunge(&app.mbox)?;
        imap_conn.logout();
        return Ok(true);
    }

    if let Some(matches) = app.arg_matches.subcommand_matches("delete") {
        debug!("delete command matched");

        let uid = matches.value_of("uid").unwrap();
        debug!("uid: {}", &uid);

        let mut imap_conn = ImapConnector::new(&app.account)?;
        imap_conn.add_flags(&app.mbox, uid, "\\Seen \\Deleted")?;
        debug!("message {} successfully deleted", uid);
        app.output
            .print(format!("Message {} successfully deleted", uid));

        imap_conn.expunge(&app.mbox)?;
        imap_conn.logout();
        return Ok(true);
    }

    if let Some(matches) = app.arg_matches.subcommand_matches("send") {
        debug!("send command matched");

        let mut imap_conn = ImapConnector::new(&app.account)?;
        let msg = matches.value_of("message").unwrap();
        let msg = Msg::from(msg.to_string());
        let msg = msg.to_sendable_msg()?;
        smtp::send(&app.account, &msg)?;
        imap_conn.append_msg("Sent", &msg.formatted(), &[Flag::Seen])?;

        imap_conn.logout();
        return Ok(true);
    }

    if let Some(matches) = app.arg_matches.subcommand_matches("save") {
        debug!("save command matched");

        let mut imap_conn = ImapConnector::new(&app.account)?;
        let msg = matches.value_of("message").unwrap();
        let msg = Msg::from(msg.to_string());
        imap_conn.append_msg(&app.mbox, &msg.to_vec()?, &[Flag::Seen])?;

        imap_conn.logout();
        return Ok(true);
    }

    {
        debug!("default list command matched");

        let mut imap_conn = ImapConnector::new(&app.account)?;
        let msgs =
            imap_conn.list_msgs(&app.mbox, &app.config.default_page_size(&app.account), &0)?;
        let msgs = Msgs::from(&msgs);
        app.output.print(msgs);

        imap_conn.logout();
        Ok(true)
    }
}
