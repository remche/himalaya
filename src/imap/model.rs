use error_chain::error_chain;
use imap;
use log::{debug, trace};
use native_tls::{self, TlsConnector, TlsStream};
use std::{collections::HashSet, iter::FromIterator, net::TcpStream};

use crate::{
    config::model::{Account, Config},
    flag::model::Flag,
    mbox::model::{Mbox, Mboxes},
    msg::model::Msg,
};

error_chain! {
    links {
        Config(crate::config::model::Error, crate::config::model::ErrorKind);
    }
}

#[derive(Debug)]
pub struct ImapConnector<'a> {
    pub account: &'a Account,
    pub sess: imap::Session<TlsStream<TcpStream>>,
}

impl<'ic> ImapConnector<'ic> {
    pub fn new(account: &'ic Account) -> Result<Self> {
        debug!("create TLS builder");
        let insecure = account.imap_insecure();
        let tls = TlsConnector::builder()
            .danger_accept_invalid_certs(insecure)
            .danger_accept_invalid_hostnames(insecure)
            .build()
            .chain_err(|| "Cannot create TLS connector")?;

        debug!("create client");
        let client = if account.imap_starttls() {
            imap::connect_starttls(account.imap_addr(), &account.imap_host, &tls)
                .chain_err(|| "Cannot connect using STARTTLS")
        } else {
            imap::connect(account.imap_addr(), &account.imap_host, &tls)
                .chain_err(|| "Cannot connect using TLS")
        }?;

        debug!("create session");
        let sess = client
            .login(&account.imap_login, &account.imap_passwd()?)
            .map_err(|res| res.0)
            .chain_err(|| "Cannot login to IMAP server")?;

        Ok(Self { account, sess })
    }

    pub fn logout(&mut self) {
        debug!("logout");
        match self.sess.logout() {
            _ => (),
        }
    }

    pub fn set_flags(&mut self, mbox: &str, uid_seq: &str, flags: &str) -> Result<()> {
        self.sess
            .select(mbox)
            .chain_err(|| format!("Cannot select mailbox `{}`", mbox))?;

        self.sess
            .uid_store(uid_seq, format!("FLAGS ({})", flags))
            .chain_err(|| format!("Cannot set flags `{}`", &flags))?;

        Ok(())
    }

    pub fn add_flags(&mut self, mbox: &str, uid_seq: &str, flags: &str) -> Result<()> {
        self.sess
            .select(mbox)
            .chain_err(|| format!("Cannot select mailbox `{}`", mbox))?;

        self.sess
            .uid_store(uid_seq, format!("+FLAGS ({})", flags))
            .chain_err(|| format!("Cannot add flags `{}`", &flags))?;

        Ok(())
    }

    pub fn remove_flags(&mut self, mbox: &str, uid_seq: &str, flags: &str) -> Result<()> {
        self.sess
            .select(mbox)
            .chain_err(|| format!("Cannot select mailbox `{}`", mbox))?;

        self.sess
            .uid_store(uid_seq, format!("-FLAGS ({})", flags))
            .chain_err(|| format!("Cannot remove flags `{}`", &flags))?;

        Ok(())
    }

    fn search_new_msgs(&mut self) -> Result<Vec<u32>> {
        let uids: Vec<u32> = self
            .sess
            .uid_search("NEW")
            .chain_err(|| "Could not search new messages")?
            .into_iter()
            .collect();
        debug!("found {} new messages", uids.len());
        trace!("uids: {:?}", uids);

        Ok(uids)
    }

    pub fn idle(&mut self, config: &Config, mbox: &str) -> Result<()> {
        debug!("examine mailbox: {}", mbox);
        self.sess
            .examine(mbox)
            .chain_err(|| format!("Could not examine mailbox `{}`", mbox))?;

        debug!("init messages hashset");
        let mut msgs_set: HashSet<u32> =
            HashSet::from_iter(self.search_new_msgs()?.iter().cloned());
        trace!("messages hashset: {:?}", msgs_set);

        loop {
            debug!("begin loop");
            self.sess
                .idle()
                .and_then(|mut idle| {
                    idle.set_keepalive(std::time::Duration::new(300, 0));
                    idle.wait_keepalive()
                })
                .chain_err(|| "Could not start the idle mode")?;

            let uids: Vec<u32> = self
                .search_new_msgs()?
                .into_iter()
                .filter(|uid| msgs_set.get(&uid).is_none())
                .collect();
            debug!("found {} new messages not in hashset", uids.len());
            trace!("messages hashet: {:?}", msgs_set);

            if !uids.is_empty() {
                let uids = uids
                    .iter()
                    .map(|uid| uid.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                let fetches = self
                    .sess
                    .uid_fetch(uids, "(ENVELOPE)")
                    .chain_err(|| "Could not fetch new messages enveloppe")?;

                for fetch in fetches.iter() {
                    let msg = Msg::from(fetch);
                    let uid = fetch.uid.ok_or_else(|| {
                        format!("Could not retrieve message {}'s UID", fetch.message)
                    })?;
                    config.run_notify_cmd(&msg.subject, &msg.sender)?;
                    debug!("notify message: {}", uid);
                    trace!("message: {:?}", msg);

                    debug!("insert message {} in hashset", uid);
                    msgs_set.insert(uid);
                    trace!("messages hashset: {:?}", msgs_set);
                }
            }

            config.exec_idle_hooks()?;
            debug!("end loop");
        }
    }

    pub fn list_mboxes(&mut self) -> Result<Mboxes> {
        let mboxes = self
            .sess
            .list(Some(""), Some("*"))
            .chain_err(|| "Cannot list mailboxes")?
            .iter()
            .map(Mbox::from_name)
            .collect::<Vec<_>>();

        Ok(Mboxes(mboxes))
    }

    pub fn list_msgs(
        &mut self,
        mbox: &str,
        page_size: &usize,
        page: &usize,
    ) -> Result<imap::types::ZeroCopy<Vec<imap::types::Fetch>>> {
        let last_seq = self
            .sess
            .select(mbox)
            .chain_err(|| format!("Cannot select mailbox `{}`", mbox))?
            .exists as i64;

        if last_seq == 0 {
            return Err(format!("The `{}` mailbox is empty", mbox).into());
        }

        // TODO: add tests, improve error management when empty page
        let range = if page_size > &0 {
            let cursor = (page * page_size) as i64;
            let begin = 1.max(last_seq - cursor);
            let end = begin - begin.min(*page_size as i64) + 1;
            format!("{}:{}", begin, end)
        } else {
            String::from("1:*")
        };

        let fetches = self
            .sess
            .fetch(range, "(UID FLAGS ENVELOPE INTERNALDATE)")
            .chain_err(|| "Cannot fetch messages")?;

        Ok(fetches)
    }

    pub fn search_msgs(
        &mut self,
        mbox: &str,
        query: &str,
        page_size: &usize,
        page: &usize,
    ) -> Result<imap::types::ZeroCopy<Vec<imap::types::Fetch>>> {
        self.sess
            .select(mbox)
            .chain_err(|| format!("Cannot select mailbox `{}`", mbox))?;

        let begin = page * page_size;
        let end = begin + (page_size - 1);
        let uids = self
            .sess
            .search(query)
            .chain_err(|| format!("Cannot search in `{}` with query `{}`", mbox, query))?
            .iter()
            .map(|seq| seq.to_string())
            .collect::<Vec<_>>();
        let range = uids[begin..end.min(uids.len())].join(",");

        let fetches = self
            .sess
            .fetch(&range, "(UID FLAGS ENVELOPE INTERNALDATE)")
            .chain_err(|| format!("Cannot fetch range `{}`", &range))?;
        // .iter()
        // .map(|fetch| Msg::from(fetch))
        // .collect::<Vec<_>>();

        Ok(fetches)
    }

    pub fn read_msg(&mut self, mbox: &str, uid: &str) -> Result<Vec<u8>> {
        self.sess
            .select(mbox)
            .chain_err(|| format!("Cannot select mailbox `{}`", mbox))?;

        match self
            .sess
            .uid_fetch(uid, "(FLAGS BODY[])")
            .chain_err(|| "Cannot fetch bodies")?
            .first()
        {
            None => Err(format!("Cannot find message `{}`", uid).into()),
            Some(fetch) => Ok(fetch.body().unwrap_or(&[]).to_vec()),
        }
    }

    pub fn append_msg(&mut self, mbox: &str, msg: &[u8], flags: &[Flag]) -> Result<()> {
        self.sess
            .append_with_flags(mbox, msg, flags)
            .chain_err(|| format!("Cannot append message to `{}`", mbox))?;

        Ok(())
    }

    pub fn expunge(&mut self, mbox: &str) -> Result<()> {
        self.sess
            .expunge()
            .chain_err(|| format!("Could not expunge `{}`", mbox))?;

        Ok(())
    }
}
