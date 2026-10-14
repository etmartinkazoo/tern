//! One background task per account owns the IMAP session. The UI talks to it with [`Cmd`]s
//! and hears back through [`Event`]s, so a slow server never blocks a keypress.
//!
//! The protocol itself is io-imap's (Pimalaya): its coroutines decide what to send and how
//! to read the replies; this file only supplies tokio sockets and TLS. A second connection
//! per account sits in IDLE on the open folder, so new mail shows up as it arrives instead
//! of on the next poll.

use crate::auth::{self, Login, Proto};
use crate::config::{Account, Tls};
use crate::model::{Cmd, Envelope, Event, Folder, Role};
use crate::parse;
use anyhow::{anyhow, bail, Context, Result};
use chrono::Local;
use io_imap::client::{ImapClientAsync, ImapClientError};
use io_imap::codec::fragmentizer::Fragmentizer;
use io_imap::coroutine::*;
use io_imap::rfc3501::append::ImapMessageAppendOptions;
use io_imap::rfc3501::copy::ImapMessageCopyOptions;
use io_imap::rfc3501::fetch::ImapMessageFetchOptions;
use io_imap::rfc3501::search::ImapMessageSearchOptions;
use io_imap::rfc3501::select::ImapMailboxSelectOptions;
use io_imap::rfc3501::store::ImapMessageStoreOptions;
use io_imap::rfc6851::r#move::ImapMessageMoveOptions;
use io_imap::session::*;
use io_imap::types::core::Vec1;
use io_imap::types::fetch::{MessageDataItem, MessageDataItemName, Section};
use io_imap::types::flag::{Flag, FlagFetch, FlagNameAttribute, StoreType};
use io_imap::types::mailbox::{ListMailbox, Mailbox};
use io_imap::types::response::Capability;
use io_imap::types::search::SearchKey;
use io_imap::types::sequence::SequenceSet;
use io_imap::types::status::{StatusDataItem, StatusDataItemName};
use io_imap::watch::*;
use io_sasl::login::SaslLoginCreds;
use io_sasl::mechanism::Sasl;
use io_sasl::xoauth2::SaslXoauth2Creds;
use std::borrow::Cow;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

const MAX_MESSAGE: u32 = 100 * 1024 * 1024;
const READ_BUF: usize = 16 * 1024;

pub fn tls_config() -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(cert);
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    Arc::new(
        rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("tls versions")
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

// ------------------------------------------------------------------- transport

enum Stream {
    Tcp(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl Stream {
    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Tcp(s) => s.read(buf).await,
            Stream::Tls(s) => s.read(buf).await,
        }
    }

    async fn write_all(&mut self, b: &[u8]) -> std::io::Result<()> {
        match self {
            Stream::Tcp(s) => s.write_all(b).await,
            Stream::Tls(s) => s.write_all(b).await,
        }
    }
}

async fn tls(host: &str, tcp: TcpStream) -> Result<Stream> {
    let name = rustls::pki_types::ServerName::try_from(host.to_string())?;
    let s = TlsConnector::from(tls_config()).connect(name, tcp).await.context("TLS handshake")?;
    Ok(Stream::Tls(Box::new(s)))
}

async fn tcp(host: &str, port: u16) -> Result<TcpStream> {
    tokio::time::timeout(std::time::Duration::from_secs(20), TcpStream::connect((host, port)))
        .await
        .map_err(|_| anyhow!("timed out connecting to {host}:{port}"))?
        .with_context(|| format!("connecting to {host}:{port}"))
}

/// An authenticated connection. io-imap's `ImapClientAsync` supplies every command on
/// top of `run`, which just shuttles bytes for its coroutines.
pub struct Conn {
    stream: Stream,
    fragmentizer: Fragmentizer,
    pub caps: Vec<Capability<'static>>,
}

impl Conn {
    pub fn has(&self, cap: &str) -> bool {
        self.caps.iter().any(|c| c.to_string().eq_ignore_ascii_case(cap))
    }
}

impl ImapClientAsync for Conn {
    #[allow(clippy::manual_async_fn)]
    fn run<C, T, E>(&mut self, mut coroutine: C) -> impl std::future::Future<Output = Result<T, ImapClientError>> + Send
    where
        C: ImapCoroutine<Yield = ImapYield, Return = Result<T, E>> + Send,
        T: Send,
        E: Send,
        ImapClientError: From<E>,
    {
        async move {
            let mut buf = vec![0u8; READ_BUF];
            let mut n = 0;
            let mut have = false;
            loop {
                let arg = if have { Some(&buf[..n]) } else { None };
                have = false;
                match coroutine.resume(&mut self.fragmentizer, arg) {
                    ImapCoroutineState::Complete(r) => return r.map_err(Into::into),
                    ImapCoroutineState::Yielded(ImapYield::WantsRead) => {
                        n = self.stream.read(&mut buf).await?;
                        if n == 0 {
                            let e = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "IMAP server closed the connection");
                            return Err(e.into());
                        }
                        have = true;
                    }
                    ImapCoroutineState::Yielded(ImapYield::WantsWrite(bytes)) => {
                        self.stream.write_all(&bytes).await?;
                    }
                }
            }
        }
    }
}

pub async fn connect(acct: &Account) -> Result<Conn> {
    let c = &acct.imap;
    let sasl: Sasl = match auth::login(acct, Proto::Imap).await? {
        Login::Password { user, password } => Sasl::Login(SaslLoginCreds { username: user, password: password.into() }),
        Login::Token { user, token } => Sasl::Xoauth2(SaslXoauth2Creds { username: user, token: token.into() }),
    };
    let transport = match c.tls {
        Tls::Implicit => ImapSessionTransport::Tls { host: c.host.clone(), port: c.port },
        Tls::Starttls | Tls::None => ImapSessionTransport::Tcp { host: c.host.clone(), port: c.port },
    };
    let opts = ImapSessionOpenOptions { starttls: c.tls == Tls::Starttls, ..Default::default() };
    let mut session = ImapSessionOpen::new(transport, Some(sasl), opts);
    let mut fragmentizer = Fragmentizer::new(MAX_MESSAGE);
    let mut stream: Option<Stream> = None;
    let mut buf = vec![0u8; READ_BUF];
    let mut n = 0;
    let mut have = false;
    loop {
        let arg = if have { Some(&buf[..n]) } else { None };
        have = false;
        match session.resume(&mut fragmentizer, arg) {
            ImapCoroutineState::Complete(Ok(data)) => {
                let stream = stream.ok_or_else(|| anyhow!("IMAP session finished without connecting"))?;
                return Ok(Conn { stream, fragmentizer, caps: data.capability });
            }
            ImapCoroutineState::Complete(Err(e)) => bail!("{} ({}): {e}", c.host, c.username),
            ImapCoroutineState::Yielded(ImapSessionOpenYield::WantsTcpConnect { host, port }) => {
                stream = Some(Stream::Tcp(tcp(&host, port).await?));
            }
            ImapCoroutineState::Yielded(ImapSessionOpenYield::WantsTlsConnect { host, port }) => {
                stream = Some(tls(&host, tcp(&host, port).await?).await?);
            }
            ImapCoroutineState::Yielded(ImapSessionOpenYield::WantsTlsUpgrade) => match stream.take() {
                Some(Stream::Tcp(s)) => stream = Some(tls(&c.host, s).await?),
                _ => bail!("STARTTLS on a connection that isn't plain TCP"),
            },
            ImapCoroutineState::Yielded(ImapSessionOpenYield::WantsUnixConnect(_)) => bail!("unix sockets not supported"),
            ImapCoroutineState::Yielded(ImapSessionOpenYield::WantsRead) => {
                let s = stream.as_mut().ok_or_else(|| anyhow!("read before connect"))?;
                n = s.read(&mut buf).await?;
                if n == 0 {
                    bail!("{} closed the connection", c.host);
                }
                have = true;
            }
            ImapCoroutineState::Yielded(ImapSessionOpenYield::WantsWrite(bytes)) => {
                stream.as_mut().ok_or_else(|| anyhow!("write before connect"))?.write_all(&bytes).await?;
            }
        }
    }
}

// ------------------------------------------------------------------- helpers

fn mailbox(name: &str) -> Result<Mailbox<'static>> {
    Mailbox::try_from(name.to_string()).map_err(|e| anyhow!("mailbox {name}: {e:?}"))
}

fn mailbox_name(m: &Mailbox<'_>) -> String {
    match m {
        Mailbox::Inbox => "INBOX".into(),
        Mailbox::Other(o) => String::from_utf8_lossy(o.as_ref()).into_owned(),
    }
}

fn uid_set(uids: &[u32]) -> Result<SequenceSet> {
    let v: Vec<NonZeroU32> = uids.iter().filter_map(|&u| NonZeroU32::new(u)).collect();
    SequenceSet::try_from(v).map_err(|e| anyhow!("uid set: {e:?}"))
}

fn seq(s: &str) -> Result<SequenceSet> {
    SequenceSet::try_from(s).map_err(|e| anyhow!("sequence {s}: {e:?}"))
}

fn flag(name: &str) -> Flag<'static> {
    match name {
        "\\Flagged" => Flag::Flagged,
        "\\Answered" => Flag::Answered,
        "\\Deleted" => Flag::Deleted,
        "\\Draft" => Flag::Draft,
        _ => Flag::Seen,
    }
}

fn flag_str(f: &FlagFetch<'_>) -> String {
    match f {
        FlagFetch::Flag(Flag::Seen) => "\\Seen".into(),
        FlagFetch::Flag(Flag::Answered) => "\\Answered".into(),
        FlagFetch::Flag(Flag::Flagged) => "\\Flagged".into(),
        FlagFetch::Flag(Flag::Deleted) => "\\Deleted".into(),
        FlagFetch::Flag(Flag::Draft) => "\\Draft".into(),
        FlagFetch::Flag(other) => other.to_string(),
        FlagFetch::Recent => "\\Recent".into(),
    }
}

fn header_items() -> Vec<MessageDataItemName<'static>> {
    vec![
        MessageDataItemName::Uid,
        MessageDataItemName::Flags,
        MessageDataItemName::InternalDate,
        MessageDataItemName::Rfc822Size,
        MessageDataItemName::BodyExt { section: Some(Section::Header(None)), partial: None, peek: true },
    ]
}

fn full_items() -> Vec<MessageDataItemName<'static>> {
    vec![MessageDataItemName::Uid, MessageDataItemName::BodyExt { section: None, partial: None, peek: true }]
}

/// Pull an envelope out of one FETCH response.
fn to_envelope(items: &[MessageDataItem<'static>]) -> Option<Envelope> {
    let mut uid = None;
    let mut flags = Vec::new();
    let mut header: Vec<u8> = Vec::new();
    let mut internal = None;
    for it in items {
        match it {
            MessageDataItem::Uid(u) => uid = Some(u.get()),
            MessageDataItem::Flags(f) => flags = f.iter().map(flag_str).collect(),
            MessageDataItem::InternalDate(d) => internal = Some(d.as_ref().with_timezone(&Local)),
            MessageDataItem::BodyExt { data, .. } | MessageDataItem::Rfc822Header(data) => {
                if let Some(b) = data.clone().into_option() {
                    header = b.into_owned();
                }
            }
            _ => {}
        }
    }
    Some(parse::envelope(uid?, &header, &flags, internal))
}

fn to_raw(items: &[MessageDataItem<'static>]) -> Option<(u32, Vec<u8>)> {
    let mut uid = None;
    let mut raw = None;
    for it in items {
        match it {
            MessageDataItem::Uid(u) => uid = Some(u.get()),
            MessageDataItem::BodyExt { data, .. } | MessageDataItem::Rfc822(data) => {
                raw = data.clone().into_option().map(|b| b.into_owned());
            }
            _ => {}
        }
    }
    Some((uid?, raw?))
}

// ------------------------------------------------------------------- worker

struct Worker {
    idx: usize,
    acct: Account,
    tx: UnboundedSender<Event>,
    conn: Option<Conn>,
    selected: Option<String>,
    /// (folder, exists, uid_next) seen on last select, to skip refetching unchanged folders.
    last_state: Option<(String, u32, u32)>,
    /// The IDLE watcher on the open folder, and the switch that stops it.
    watching: Option<(String, Arc<AtomicBool>)>,
}

pub fn spawn(idx: usize, acct: Account, mut rx: UnboundedReceiver<Cmd>, tx: UnboundedSender<Event>) {
    tokio::spawn(async move {
        let mut w = Worker { idx, acct, tx, conn: None, selected: None, last_state: None, watching: None };
        while let Some(cmd) = rx.recv().await {
            // Collapse a burst of body requests (holding j) into the last one.
            let cmd = match cmd {
                Cmd::Body { .. } => {
                    let mut last = cmd;
                    while let Ok(next) = rx.try_recv() {
                        if matches!(next, Cmd::Body { .. }) {
                            last = next;
                        } else {
                            w.run(last).await;
                            last = next;
                        }
                    }
                    last
                }
                c => c,
            };
            w.run(cmd).await;
        }
        w.stop_watch();
        if let Some(mut c) = w.conn.take() {
            let _ = c.logout().await;
        }
    });
}

impl Worker {
    fn send(&self, e: Event) {
        let _ = self.tx.send(e);
    }

    async fn run(&mut self, cmd: Cmd) {
        let desc = cmd.label();
        // One retry after a reconnect: servers drop idle connections all the time.
        for attempt in 0..2 {
            if self.conn.is_none() {
                self.send(Event::Status(self.idx, format!("connecting to {}…", self.acct.imap.host)));
                match connect(&self.acct).await {
                    Ok(c) => {
                        self.conn = Some(c);
                        self.selected = None;
                        self.send(Event::Connected(self.idx));
                    }
                    Err(e) => {
                        self.send(Event::Error(self.idx, format!("{e:#}")));
                        return;
                    }
                }
            }
            match self.handle(&cmd).await {
                Ok(()) => return,
                Err(e) => {
                    self.conn = None;
                    self.selected = None;
                    // Something may have half-applied; make the next poll refetch.
                    self.last_state = None;
                    if attempt == 1 {
                        self.send(Event::Error(self.idx, format!("{desc}: {e:#}")));
                    }
                }
            }
        }
    }

    fn c(&mut self) -> &mut Conn {
        self.conn.as_mut().expect("connected")
    }

    async fn select(&mut self, folder: &str) -> Result<(u32, u32)> {
        let data = self
            .c()
            .select(mailbox(folder)?, ImapMailboxSelectOptions::default())
            .await
            .with_context(|| format!("select {folder}"))?;
        self.selected = Some(folder.to_string());
        Ok((data.exists.unwrap_or(0), data.uid_next.map(|u| u.get()).unwrap_or(0)))
    }

    async fn ensure_selected(&mut self, folder: &str) -> Result<()> {
        if self.selected.as_deref() != Some(folder) {
            self.select(folder).await?;
        }
        Ok(())
    }

    async fn handle(&mut self, cmd: &Cmd) -> Result<()> {
        match cmd {
            Cmd::Folders => self.folders().await,
            Cmd::Select { folder } => {
                self.envelopes(folder, true).await?;
                self.start_watch(folder);
                Ok(())
            }
            Cmd::Poll { folder } => {
                self.envelopes(folder, false).await?;
                self.folders().await
            }
            Cmd::Body { folder, uid } => self.bodies(folder, &[*uid]).await,
            Cmd::Bodies { folder, uids } => self.bodies(folder, uids).await,
            Cmd::SetFlag { folder, uids, flag: f, on } => {
                self.ensure_selected(folder).await?;
                let kind = if *on { StoreType::Add } else { StoreType::Remove };
                self.c().store(uid_set(uids)?, kind, vec![flag(f)], ImapMessageStoreOptions { uid: true }).await?;
                Ok(())
            }
            Cmd::Move { folder, uids, to } => {
                self.ensure_selected(folder).await?;
                let set = uid_set(uids)?;
                if self.c().has("MOVE") {
                    self.c().r#move(set, mailbox(to)?, ImapMessageMoveOptions { uid: true }).await?;
                } else {
                    self.c().copy(set.clone(), mailbox(to)?, ImapMessageCopyOptions { uid: true }).await?;
                    self.expunge_uids(set).await?;
                }
                self.send(Event::Removed { acct: self.idx, folder: folder.clone(), uids: uids.clone() });
                Ok(())
            }
            Cmd::Expunge { folder, uids } => {
                self.ensure_selected(folder).await?;
                self.expunge_uids(uid_set(uids)?).await?;
                self.send(Event::Removed { acct: self.idx, folder: folder.clone(), uids: uids.clone() });
                Ok(())
            }
            Cmd::Append { folder, raw, flags } => {
                let mut fl = vec![Flag::Seen];
                if flags.contains("Draft") {
                    fl.push(Flag::Draft);
                }
                let opts = ImapMessageAppendOptions { flags: fl, ..Default::default() };
                self.c().append(mailbox(folder)?, raw, opts).await?;
                Ok(())
            }
            Cmd::Ensure { folder } => {
                let pattern = ListMailbox::try_from("*").map_err(|e| anyhow!("{e:?}"))?;
                let exists = self.c().list(mailbox("")?, pattern).await?.iter().any(|(m, _, _)| mailbox_name(m) == *folder);
                if !exists {
                    self.c().create(mailbox(folder)?).await?;
                    self.folders().await?;
                }
                Ok(())
            }
            Cmd::Unsnooze { folder, message_id, to } => {
                let (exists, _) = self.select(folder).await?;
                let mut found = false;
                if exists > 0 {
                    let want = message_id.trim_matches(|c| c == '<' || c == '>');
                    let all = self.fetch_headers(seq("1:*")?, false).await?;
                    if let Some(e) = all.iter().find(|e| e.message_id.trim_matches(|c| c == '<' || c == '>') == want) {
                        let set = uid_set(&[e.uid])?;
                        self.c().store(set.clone(), StoreType::Remove, vec![Flag::Seen], ImapMessageStoreOptions { uid: true }).await?;
                        if self.c().has("MOVE") {
                            self.c().r#move(set, mailbox(to)?, ImapMessageMoveOptions { uid: true }).await?;
                        } else {
                            self.c().copy(set.clone(), mailbox(to)?, ImapMessageCopyOptions { uid: true }).await?;
                            self.expunge_uids(set).await?;
                        }
                        found = true;
                    }
                }
                self.selected = None;
                self.last_state = None;
                self.send(Event::Unsnoozed { acct: self.idx, message_id: message_id.clone(), found });
                Ok(())
            }
            Cmd::Search { folder, query } => {
                self.ensure_selected(folder).await?;
                let text = query.clone().try_into().map_err(|e| anyhow!("search text: {e:?}"))?;
                let mut uids: Vec<u32> = self
                    .c()
                    .search(Vec1::from(SearchKey::Text(text)), ImapMessageSearchOptions { uid: true })
                    .await?
                    .into_iter()
                    .map(|u| u.get())
                    .collect();
                uids.sort_unstable_by(|a, b| b.cmp(a));
                uids.truncate(500);
                let items = if uids.is_empty() { vec![] } else { self.fetch_headers(uid_set(&uids)?, true).await? };
                self.send(Event::SearchResult { acct: self.idx, folder: folder.clone(), items, query: query.clone() });
                Ok(())
            }
        }
    }

    async fn bodies(&mut self, folder: &str, uids: &[u32]) -> Result<()> {
        self.ensure_selected(folder).await?;
        let out = self
            .c()
            .fetch(uid_set(uids)?, full_items().into(), ImapMessageFetchOptions { uid: true, ..Default::default() })
            .await?;
        let raws: Vec<(u32, Vec<u8>)> = out.values().filter_map(|items| to_raw(items.as_ref())).collect();
        if raws.is_empty() && uids.len() == 1 {
            bail!("message {} not found", uids[0]);
        }
        let bodies =
            tokio::task::spawn_blocking(move || raws.into_iter().map(|(u, r)| parse::body(u, r)).collect::<Vec<_>>()).await?;
        for body in bodies {
            self.send(Event::Body { acct: self.idx, folder: folder.to_string(), body: Box::new(body) });
        }
        Ok(())
    }

    async fn expunge_uids(&mut self, set: SequenceSet) -> Result<()> {
        self.c().store(set.clone(), StoreType::Add, vec![Flag::Deleted], ImapMessageStoreOptions { uid: true }).await?;
        if self.c().has("UIDPLUS") {
            self.c().uid_expunge(set).await?;
        } else {
            self.c().expunge().await?;
        }
        Ok(())
    }

    async fn folders(&mut self) -> Result<()> {
        let pattern = ListMailbox::try_from("*").map_err(|e| anyhow!("{e:?}"))?;
        let listing = self.c().list(mailbox("")?, pattern).await?;
        let mut out = Vec::new();
        let overrides = self.acct.folders.clone();
        for (mb, delim, attrs) in listing {
            let name = mailbox_name(&mb);
            let mut role = Role::Other;
            let mut selectable = true;
            for a in &attrs {
                match a {
                    FlagNameAttribute::Noselect => selectable = false,
                    other => match other.to_string().to_lowercase().as_str() {
                        "\\sent" => role = Role::Sent,
                        "\\drafts" => role = Role::Drafts,
                        "\\archive" => role = Role::Archive,
                        "\\trash" => role = Role::Trash,
                        "\\junk" => role = Role::Junk,
                        "\\all" => role = Role::All,
                        "\\flagged" => role = Role::Flagged,
                        "\\nonexistent" => selectable = false,
                        _ => {}
                    },
                }
            }
            if name.eq_ignore_ascii_case("INBOX") {
                role = Role::Inbox;
            }
            for (cfg, r) in [
                (&overrides.sent, Role::Sent),
                (&overrides.drafts, Role::Drafts),
                (&overrides.archive, Role::Archive),
                (&overrides.trash, Role::Trash),
                (&overrides.junk, Role::Junk),
            ] {
                if cfg.as_deref() == Some(name.as_str()) {
                    role = r;
                }
            }
            if role == Role::Other {
                role = guess_role(&name);
            }
            let (label, depth) = match delim.map(|d| d.inner()) {
                Some(d) => {
                    let parts: Vec<&str> = name.split(d).collect();
                    (parts.last().unwrap_or(&"").to_string(), parts.len() - 1)
                }
                None => (name.clone(), 0),
            };
            let mut unread = 0;
            if selectable {
                let items: Cow<'static, [StatusDataItemName]> = Cow::Borrowed(&[StatusDataItemName::Unseen]);
                if let Ok(st) = self.c().status(mailbox(&name)?, items).await {
                    for i in st {
                        if let StatusDataItem::Unseen(n) = i {
                            unread = n;
                        }
                    }
                }
            }
            let label = if role == Role::Inbox { "Inbox".into() } else { label };
            out.push(Folder { name, label, depth, role, unread, selectable });
        }
        // Special folders first in a familiar order, then everything else by path, which
        // keeps children directly under their parents.
        let rank = |r: Role| match r {
            Role::Inbox => 0,
            Role::Flagged => 1,
            Role::Drafts => 2,
            Role::Sent => 3,
            Role::Archive => 4,
            Role::All => 5,
            Role::Junk => 6,
            Role::Trash => 7,
            Role::Other => 8,
        };
        out.sort_by_key(|f| (rank(f.role), f.name.to_lowercase()));
        self.send(Event::Folders(self.idx, out));
        Ok(())
    }

    async fn envelopes(&mut self, folder: &str, force: bool) -> Result<()> {
        let (exists, uid_next) = self.select(folder).await?;
        let state = (folder.to_string(), exists, uid_next);
        if !force && self.last_state.as_ref() == Some(&state) {
            return Ok(());
        }
        self.last_state = Some(state);
        if exists == 0 {
            self.send(Event::Envelopes { acct: self.idx, folder: folder.into(), items: vec![], total: 0 });
            return Ok(());
        }
        let start = exists.saturating_sub(self.acct.imap.page_size).max(1);
        let items = self.fetch_headers(seq(&format!("{start}:*"))?, false).await?;
        self.send(Event::Envelopes { acct: self.idx, folder: folder.into(), items, total: exists });
        Ok(())
    }

    async fn fetch_headers(&mut self, set: SequenceSet, by_uid: bool) -> Result<Vec<Envelope>> {
        let out = self
            .c()
            .fetch(set, header_items().into(), ImapMessageFetchOptions { uid: by_uid, ..Default::default() })
            .await?;
        let mut items: Vec<Envelope> = out.values().filter_map(|i| to_envelope(i.as_ref())).collect();
        items.sort_by(|a, b| b.date.cmp(&a.date).then(b.uid.cmp(&a.uid)));
        Ok(items)
    }

    fn stop_watch(&mut self) {
        if let Some((_, stop)) = self.watching.take() {
            stop.store(true, Ordering::SeqCst);
        }
    }

    /// Keep a second connection in IDLE on `folder`. Any change there becomes a
    /// `Changed` event, and the UI asks this worker to refresh.
    fn start_watch(&mut self, folder: &str) {
        if self.watching.as_ref().is_some_and(|(f, _)| f == folder) {
            return;
        }
        self.stop_watch();
        let stop = Arc::new(AtomicBool::new(false));
        self.watching = Some((folder.to_string(), stop.clone()));
        let (acct, idx, tx, folder) = (self.acct.clone(), self.idx, self.tx.clone(), folder.to_string());
        tokio::spawn(async move {
            // Reconnect with backoff until told to stop: IDLE connections get dropped by
            // NATs and servers routinely.
            let mut delay = 5;
            while !stop.load(Ordering::SeqCst) {
                match watch(&acct, idx, &folder, &tx, &stop).await {
                    Ok(()) => return,
                    Err(_) => {
                        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                        delay = (delay * 2).min(300);
                    }
                }
            }
        });
    }
}

async fn watch(acct: &Account, idx: usize, folder: &str, tx: &UnboundedSender<Event>, stop: &Arc<AtomicBool>) -> Result<()> {
    let mut conn = connect(acct).await?;
    if !conn.has("IDLE") {
        // The regular poll covers servers without IDLE.
        return Ok(());
    }
    let mut watcher = ImapMailboxWatch::new(&conn.caps, mailbox(folder)?, stop.clone(), Default::default());
    let mut buf = vec![0u8; READ_BUF];
    let mut arg: Option<Vec<u8>> = None;
    // Several events usually arrive together (EXISTS, then FETCH); coalesce them into
    // one refresh.
    let mut dirty = false;
    loop {
        match watcher.resume(&mut conn.fragmentizer, arg.as_deref()) {
            ImapCoroutineState::Complete(Ok(())) => return Ok(()),
            ImapCoroutineState::Complete(Err(e)) => return Err(anyhow!("watch {folder}: {e}")),
            ImapCoroutineState::Yielded(ImapMailboxWatchYield::Event(_)) => {
                dirty = true;
                arg = None;
            }
            ImapCoroutineState::Yielded(ImapMailboxWatchYield::WantsWait) => arg = None,
            ImapCoroutineState::Yielded(ImapMailboxWatchYield::WantsWrite(bytes)) => {
                conn.stream.write_all(&bytes).await?;
                arg = None;
            }
            ImapCoroutineState::Yielded(ImapMailboxWatchYield::WantsRead) => {
                if dirty {
                    dirty = false;
                    let _ = tx.send(Event::Changed { acct: idx, folder: folder.to_string() });
                }
                // Wake now and then so a stop request is noticed on a quiet mailbox.
                let stopped = async {
                    while !stop.load(Ordering::SeqCst) {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await
                    }
                };
                let n = tokio::select! {
                    r = conn.stream.read(&mut buf) => r?,
                    _ = stopped => {
                        arg = None;
                        continue;
                    }
                };
                if n == 0 {
                    bail!("server closed the IDLE connection");
                }
                arg = Some(buf[..n].to_vec());
            }
        }
    }
}

fn guess_role(name: &str) -> Role {
    let last = name.rsplit(['/', '.']).next().unwrap_or(name).to_lowercase();
    match last.as_str() {
        "sent" | "sent items" | "sent mail" | "sent messages" => Role::Sent,
        "drafts" | "draft" => Role::Drafts,
        "archive" | "archives" | "all mail" => Role::Archive,
        "trash" | "deleted" | "deleted items" | "deleted messages" | "bin" => Role::Trash,
        "junk" | "spam" | "junk e-mail" | "bulk mail" => Role::Junk,
        "starred" | "flagged" => Role::Flagged,
        _ => Role::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Network check of the TLS stack: handshake with a public server and read its
    /// greeting. No credentials are sent. `cargo test -- --ignored`
    #[tokio::test]
    #[ignore]
    async fn tls_greeting() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let t = tcp("imap.gmail.com", 993).await.unwrap();
        let mut s = tls("imap.gmail.com", t).await.unwrap();
        let mut buf = [0u8; 256];
        let n = s.read(&mut buf).await.unwrap();
        assert!(buf[..n].starts_with(b"* OK"));
    }
}
