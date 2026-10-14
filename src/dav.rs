//! CalDAV and CardDAV through io-webdav (Pimalaya): discovery, listing and incremental
//! sync for every `[[dav]]` source. Its client is blocking, so everything here runs on a
//! blocking thread; credentials are resolved beforehand (OAuth may need the network).
//!
//! Calendars are kept in a local cache so the agenda opens instantly and works offline.
//! The cache sits behind `load_cache`/`save_cache` only, so a different store (Pimalaya's
//! pimdir, once its format settles) can replace it without touching the rest.

use crate::config::{cache_dir, Dav};
use crate::contacts::{parse_vcards, Contact};
use anyhow::{anyhow, bail, Context, Result};
use io_http::rfc6750::bearer::HttpAuthBearer;
use io_http::rfc7617::basic::HttpAuthBasic;
use io_webdav::client::WebdavClientStd;
use io_webdav::rfc4918::WebdavAuth;
use io_webdav::rfc6578::sync_collection::WebdavSyncCollectionOptions;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A source with its credentials resolved.
pub struct Source {
    pub dav: Dav,
    pub auth: WebdavAuth,
}

pub async fn resolve(dav: &Dav, cfg: &crate::config::Config) -> Result<Source> {
    let auth = match dav.account.as_ref().and_then(|a| cfg.accounts.iter().find(|x| &x.name == a)) {
        Some(acct) if acct.oauth.is_some() => {
            let token = crate::oauth::access_token(&acct.name, acct.oauth.as_ref().unwrap()).await?;
            WebdavAuth::Bearer(HttpAuthBearer::new(token))
        }
        Some(acct) if dav.secret.password.is_none() && dav.secret.password_cmd.is_none() => {
            // Same server, same password as the mail account.
            WebdavAuth::Basic(HttpAuthBasic::new(dav.username.clone(), acct.imap.secret.resolve()?))
        }
        _ => WebdavAuth::Basic(HttpAuthBasic::new(dav.username.clone(), dav.secret.resolve()?)),
    };
    Ok(Source { dav: dav.clone(), auth })
}

/// io-webdav's client keeps one connection for every request. Servers close it whenever
/// they like (Radicale after every response, others when idle), so each call reopens the
/// connection and retries when it finds it closed. Results the client caches (the
/// principal and home-set URLs) survive, so a multi-request call makes progress.
struct Client {
    c: WebdavClientStd,
    auth: WebdavAuth,
    host: String,
    port: u16,
    https: bool,
    name: String,
}

fn client(src: &Source) -> Result<Client> {
    let url: url::Url = src.dav.url.parse().with_context(|| format!("{}: bad url {}", src.dav.name, src.dav.url))?;
    let c = WebdavClientStd::connect(&url, src.auth.clone(), Default::default())
        .map_err(|e| anyhow!("{}: connecting to {}: {e}", src.dav.name, src.dav.url))?;
    Ok(Client {
        c,
        auth: src.auth.clone(),
        host: url.host_str().unwrap_or_default().to_string(),
        port: url.port_or_known_default().unwrap_or(443),
        https: url.scheme() == "https",
        name: src.dav.name.clone(),
    })
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Calendars,
    Contacts,
}

impl Client {
    /// The calendar or address book home, found from the configured URL, else the way
    /// other clients look: the server's /.well-known redirect (RFC 6764), then /dav/
    /// and Nextcloud's /remote.php/dav/. Fastmail, for one, answers 404 at its bare
    /// host but announces /dav/ through .well-known.
    fn home(&mut self, kind: Kind) -> Result<url::Url> {
        let first = self.try_home(kind);
        if first.is_ok() {
            return first;
        }
        let configured = self.c.base_url.clone();
        let mut candidates = Vec::new();
        if let Some(loc) = self.well_known(kind) {
            candidates.push(loc);
        }
        candidates.push("/dav/".to_string());
        candidates.push("/remote.php/dav/".to_string());
        for path in candidates {
            let Ok(url) = configured.join(&path) else { continue };
            if url.path() == configured.path() || url.host_str() != configured.host_str() {
                continue;
            }
            self.c.base_url = url;
            self.c.principal_url = None;
            self.c.calendar_home_set = None;
            self.c.addressbook_home_set = None;
            if let Ok(h) = self.try_home(kind) {
                return Ok(h);
            }
        }
        self.c.base_url = configured;
        first
    }

    fn try_home(&mut self, kind: Kind) -> Result<url::Url> {
        match kind {
            Kind::Calendars => self.call("calendars", |c| c.calendar_home_set()),
            Kind::Contacts => self.call("address books", |c| c.addressbook_home_set()),
        }
    }

    /// Where the server's /.well-known/caldav (or carddav) redirects, if it does.
    fn well_known(&self, kind: Kind) -> Option<String> {
        use pimalaya_stream::stream::Stream;
        use std::io::{Read, Write};
        let path = match kind {
            Kind::Calendars => "/.well-known/caldav",
            Kind::Contacts => "/.well-known/carddav",
        };
        let mut s = if self.https {
            Stream::connect_tls(&self.host, self.port, Default::default()).ok()?
        } else {
            Stream::connect_tcp(&self.host, self.port, Default::default()).ok()?
        };
        let req = format!(
            "PROPFIND {path} HTTP/1.1\r\nHost: {}\r\nDepth: 0\r\nContent-Length: 0\r\nUser-Agent: tern\r\nConnection: close\r\n\r\n",
            self.host
        );
        s.write_all(req.as_bytes()).ok()?;
        let mut head = Vec::new();
        let mut buf = [0u8; 4096];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 64 * 1024 {
            let n = s.read(&mut buf).ok()?;
            if n == 0 {
                break;
            }
            head.extend_from_slice(&buf[..n]);
        }
        let text = String::from_utf8_lossy(&head);
        let status = text.split_whitespace().nth(1)?;
        if !status.starts_with('3') {
            return None;
        }
        text.lines()
            .find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("location")).map(|(_, v)| v.trim().to_string()))
    }

    fn reopen(&mut self) -> Result<()> {
        use pimalaya_stream::stream::Stream;
        let s = if self.https {
            Stream::connect_tls(&self.host, self.port, Default::default())
        } else {
            Stream::connect_tcp(&self.host, self.port, Default::default())
        }
        .map_err(|e| anyhow!("{}: reconnecting: {e}", self.name))?;
        self.c.set_stream(s);
        Ok(())
    }

    fn call<T, E: std::fmt::Display>(&mut self, what: &str, mut f: impl FnMut(&mut WebdavClientStd) -> Result<T, E>) -> Result<T> {
        for attempt in 0..8 {
            match f(&mut self.c) {
                Ok(v) => return Ok(v),
                Err(e) => {
                    let msg = e.to_string();
                    let closed = ["unexpected EOF", "reset by peer", "Broken pipe", "connection closed", "UnexpectedEof"]
                        .iter()
                        .any(|m| msg.contains(m));
                    if closed && attempt < 7 {
                        self.reopen()?;
                        continue;
                    }
                    bail!("{}: {what}: {msg}", self.name);
                }
            }
        }
        unreachable!()
    }
}

/// The resource name from an href, percent-decoded so it matches what we PUT.
fn last_segment(href: &str) -> String {
    let seg = href.trim_end_matches('/').rsplit('/').next().unwrap_or(href);
    url::form_urlencoded::parse(format!("x={}", seg.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| seg.to_string())
}

/// For `tern doctor`: how many calendars and address books a source finds, each on its
/// own so one missing doesn't hide the other.
pub fn probe(src: &Source) -> (Result<usize>, Result<usize>) {
    let cals = client(src).and_then(|mut c| list_calendars(&mut c)).map(|v| v.len());
    let books = client(src).and_then(|mut c| {
        c.home(Kind::Contacts)?;
        Ok(c.call("address books", |c| c.list_addressbooks())?.len())
    });
    (cals, books)
}

// ------------------------------------------------------------------- contacts

/// Every card in every address book of every source. A source that fails is reported
/// but doesn't stop the others.
pub fn sync_contacts(sources: &[Source]) -> Result<(Vec<Contact>, Vec<String>)> {
    let mut all = Vec::new();
    let mut errors = Vec::new();
    for src in sources {
        let r: Result<()> = (|| {
            let mut c = client(src)?;
            c.home(Kind::Contacts)?;
            for book in c.call("address books", |c| c.list_addressbooks())? {
                let cards = c.call(&book.id, |c| c.list_cards(&book.id, &Default::default()))?;
                for card in cards.cards {
                    let raw = String::from_utf8_lossy(&card.data).into_owned();
                    for mut ct in parse_vcards(&raw) {
                        ct.source = src.dav.name.clone();
                        ct.book = book.id.clone();
                        ct.id = last_segment(&card.id);
                        ct.etag = card.etag.clone();
                        ct.raw = raw.clone();
                        all.push(ct);
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = r {
            errors.push(format!("{e:#}"));
        }
    }
    if all.is_empty() && !errors.is_empty() && errors.len() == sources.len() {
        bail!("{}", errors.join("; "));
    }
    all.retain(|c| !c.name.is_empty() || !c.emails.is_empty());
    all.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok((all, errors))
}

/// Create a contact in the first address book of `src`.
pub fn add_contact(src: &Source, name: &str, email: &str) -> Result<()> {
    let mut c = client(src)?;
    c.home(Kind::Contacts)?;
    let book = c
        .call("address books", |c| c.list_addressbooks())?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("{} has no address book", src.dav.name))?;
    let uid = uuid::Uuid::new_v4().to_string();
    let esc = |s: &str| s.replace('\\', "\\\\").replace(',', "\\,").replace(';', "\\;");
    let (given, family) = name.rsplit_once(' ').unwrap_or((name, ""));
    let card = format!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:{uid}\r\nFN:{}\r\nN:{};{};;;\r\nEMAIL;TYPE=INTERNET:{email}\r\nEND:VCARD\r\n",
        esc(name),
        esc(family),
        esc(given)
    );
    let bytes = card.into_bytes();
    c.call("creating card", |c| c.create_card(&book.id, &format!("{uid}.vcf"), bytes.clone()))?;
    Ok(())
}

// ------------------------------------------------------------------- calendars

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CachedCalendar {
    pub source: String,
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub sync_token: Option<String>,
    /// The calendar's parent collection, and its own last path segment: io-webdav
    /// addresses a calendar as home + segment, so tern points "home" at the real parent.
    /// (Google keeps every calendar at `…/<calendar>/events/`.)
    #[serde(default)]
    pub home: String,
    #[serde(default)]
    pub remote: String,
    /// Resource id → raw iCalendar.
    pub items: BTreeMap<String, String>,
    /// Resource id → ETag, to fetch only what changed when diffing a full listing.
    #[serde(default)]
    pub etags: BTreeMap<String, String>,
}

impl CachedCalendar {
    /// Where writes for this calendar go: (parent collection, last segment).
    pub fn target(&self) -> (String, String) {
        let remote = if self.remote.is_empty() { self.id.clone() } else { self.remote.clone() };
        (self.home.clone(), remote)
    }
}

fn cache_file() -> std::path::PathBuf {
    cache_dir().join("calendars.json")
}

pub fn load_cache() -> Vec<CachedCalendar> {
    std::fs::read(cache_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_cache(c: &[CachedCalendar]) -> Result<()> {
    std::fs::create_dir_all(cache_dir())?;
    let tmp = cache_file().with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(c)?)?;
    std::fs::rename(tmp, cache_file())?;
    Ok(())
}

/// A calendar collection as listed, with where it really lives.
struct Found {
    /// Unique within its source: the last segment, or (when several calendars share it,
    /// as Google's `events` do) the distinguishing segment before it.
    key: String,
    remote: String,
    home: url::Url,
    name: String,
    color: Option<String>,
    components: Vec<String>,
}

fn decode(seg: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", seg.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| seg.to_string())
}

/// PROPFIND the calendar home ourselves (io-webdav's listing keeps only each calendar's
/// last path segment) so every calendar keeps its full address.
fn list_calendars(c: &mut Client) -> Result<Vec<Found>> {
    use io_webdav::coroutine::{WebdavCoroutine, WebdavCoroutineState, WebdavYield};
    use io_webdav::rfc4791::calendar::{CALENDAR, CALENDAR_COLOR, LIST_PROPS, SUPPORTED_CALENDAR_COMPONENT_SET};
    use io_webdav::rfc4918::propfind::WebdavPropfind;
    use io_webdav::rfc4918::{DISPLAYNAME, RESOURCETYPE};
    use std::io::{Read, Write};

    let home = c.home(Kind::Calendars)?;
    let auth = c.auth.clone();
    let ms = c.call("calendars", |w| -> std::result::Result<_, String> {
        let mut co = WebdavPropfind::new(&w.base_url, &auth, &w.user_agent.clone(), home.path(), 1, LIST_PROPS);
        let mut buf = vec![0u8; 16 * 1024];
        let mut n: Option<usize> = None;
        loop {
            let arg = n.take().map(|k| &buf[..k]);
            match co.resume(arg) {
                WebdavCoroutineState::Complete(r) => return r.map_err(|e| e.to_string()),
                WebdavCoroutineState::Yielded(WebdavYield::WantsRead) => {
                    n = Some(w.stream.read(&mut buf).map_err(|e| e.to_string())?);
                }
                WebdavCoroutineState::Yielded(WebdavYield::WantsWrite(bytes)) => {
                    w.stream.write_all(&bytes).map_err(|e| e.to_string())?;
                }
            }
        }
    })?;
    let mut found = Vec::new();
    for e in &ms.responses {
        if !e.has_resource_type(RESOURCETYPE, CALENDAR) {
            continue;
        }
        let Ok(url) = home.join(&e.href) else { continue };
        let segs: Vec<String> = url.path().trim_end_matches('/').split('/').map(decode).collect();
        let Some(remote) = segs.last().cloned().filter(|s| !s.is_empty()) else { continue };
        let parent_path = url.path().trim_end_matches('/').rsplit_once('/').map(|(p, _)| format!("{p}/")).unwrap_or_else(|| "/".into());
        let mut parent = url.clone();
        parent.set_path(&parent_path);
        let components = e
            .prop(SUPPORTED_CALENDAR_COMPONENT_SET)
            .map(|item| item.children.iter().filter_map(|ch| ch.name.clone()).collect())
            .unwrap_or_default();
        found.push(Found {
            key: remote.clone(),
            remote,
            home: parent,
            name: e.text(DISPLAYNAME).map(String::from).unwrap_or_default(),
            color: e.text(CALENDAR_COLOR).map(String::from),
            components,
        });
        let _ = segs;
    }
    // Several calendars ending in the same segment: name them by the one before it.
    let mut counts = std::collections::HashMap::new();
    for f in &found {
        *counts.entry(f.remote.clone()).or_insert(0) += 1;
    }
    for f in found.iter_mut() {
        if counts[&f.remote] > 1 {
            let segs: Vec<String> = f.home.path().trim_end_matches('/').split('/').map(decode).collect();
            if let Some(p) = segs.last().filter(|s| !s.is_empty()) {
                f.key = p.clone();
            }
        }
        if f.name.is_empty() {
            f.name = f.key.clone();
        }
    }
    Ok(found)
}

/// Bring the cache up to date with every source: only what changed since the last sync
/// token when the server supports it, a full listing when it doesn't.
pub fn sync_calendars(sources: &[Source]) -> Result<(Vec<CachedCalendar>, Vec<String>)> {
    let old = load_cache();
    let mut out: Vec<CachedCalendar> = Vec::new();
    let mut errors = Vec::new();
    for src in sources {
        let r: Result<()> = (|| {
            let mut c = client(src)?;
            for cal in list_calendars(&mut c)? {
                if !cal.components.is_empty() && !cal.components.iter().any(|x| x.eq_ignore_ascii_case("VEVENT")) {
                    continue;
                }
                // A cached copy is reused only when it is the same collection (an old
                // cache entry with a colliding id would otherwise leak into it).
                let mut entry = old
                    .iter()
                    .find(|o| o.source == src.dav.name && o.id == cal.key && (o.home.is_empty() || o.home == cal.home.as_str()))
                    .filter(|o| !o.home.is_empty())
                    .cloned()
                    .unwrap_or_default();
                entry.source = src.dav.name.clone();
                entry.id = cal.key.clone();
                entry.name = cal.name.clone();
                entry.color = cal.color.clone();
                entry.home = cal.home.to_string();
                entry.remote = cal.remote.clone();
                c.c.calendar_home_set = Some(cal.home.clone());
                let cal_id = cal.remote.clone();

                // Incremental REPORT first. Without a token the server lists everything,
                // which is a full snapshot; with one it sends only changes and deletions.
                let token = entry.sync_token.clone();
                let report = c.call("sync", |c| c.sync_items(&cal_id, token.as_deref(), WebdavSyncCollectionOptions::default()));
                let (delta, snapshot) = match report {
                    Ok(d) if !d.truncated => {
                        let full = token.is_none();
                        (d, full)
                    }
                    // Unsupported REPORT, an expired token, or a truncated answer: list the
                    // collection and diff it ourselves.
                    _ => {
                        let opts = WebdavSyncCollectionOptions { fallback: true };
                        (c.call("listing", |c| c.sync_items(&cal_id, None, opts))?, true)
                    }
                };
                let listed: BTreeMap<String, Option<String>> =
                    delta.changed.iter().map(|ch| (last_segment(&ch.href), ch.etag.clone())).collect();
                if snapshot {
                    entry.items.retain(|id, _| listed.contains_key(id));
                    entry.etags.retain(|id, _| listed.contains_key(id));
                }
                for href in &delta.vanished {
                    let id = last_segment(href);
                    entry.items.remove(&id);
                    entry.etags.remove(&id);
                }
                // Fetch what is new or whose ETag moved.
                let wanted: Vec<String> = listed
                    .iter()
                    .filter(|(id, etag)| !entry.items.contains_key(*id) || etag.is_none() || entry.etags.get(*id) != etag.as_ref())
                    .map(|(id, _)| id.clone())
                    .collect();
                for chunk in wanted.chunks(100) {
                    let refs: Vec<&str> = chunk.iter().map(String::as_str).collect();
                    for item in c.call(&entry.name, |c| c.multiget_items(&cal_id, &refs))? {
                        let id = last_segment(&item.id);
                        if let Some(e) = &item.etag {
                            entry.etags.insert(id.clone(), e.clone());
                        }
                        entry.items.insert(id, String::from_utf8_lossy(&item.data).into_owned());
                    }
                }
                entry.sync_token = delta.sync_token;
                out.push(entry);
            }
            Ok(())
        })();
        if let Err(e) = r {
            errors.push(format!("{e:#}"));
            // Keep what we had for a source that failed this time.
            out.extend(old.iter().filter(|o| o.source == src.dav.name).cloned());
        }
    }
    save_cache(&out)?;
    Ok((out, errors))
}

// ------------------------------------------------------------------- async entry points

async fn sources(cfg: &crate::config::Config) -> (Vec<Source>, Vec<String>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for d in cfg.dav_sources() {
        match resolve(&d, cfg).await {
            Ok(s) => out.push(s),
            Err(e) => errors.push(format!("{}: {e:#}", d.name)),
        }
    }
    (out, errors)
}

/// Sync contacts from every source into the local cache.
pub async fn refresh_contacts(cfg: crate::config::Config) -> Result<(Vec<Contact>, Vec<String>)> {
    let (srcs, mut errors) = sources(&cfg).await;
    if srcs.is_empty() {
        bail!("{}", if errors.is_empty() { "no [[dav]] sources configured".into() } else { errors.join("; ") });
    }
    let (contacts, more) = tokio::task::spawn_blocking(move || sync_contacts(&srcs)).await??;
    errors.extend(more);
    crate::contacts::save_cache(&contacts)?;
    Ok((contacts, errors))
}

pub async fn refresh_calendars(cfg: crate::config::Config) -> Result<(Vec<CachedCalendar>, Vec<String>)> {
    let (srcs, mut errors) = sources(&cfg).await;
    if srcs.is_empty() {
        bail!("{}", if errors.is_empty() { "no [[dav]] sources configured".into() } else { errors.join("; ") });
    }
    let (cals, more) = tokio::task::spawn_blocking(move || sync_calendars(&srcs)).await??;
    errors.extend(more);
    Ok((cals, errors))
}

pub async fn add_contact_to_first(cfg: crate::config::Config, name: String, email: String) -> Result<()> {
    let d = cfg.dav_sources().into_iter().next().ok_or_else(|| anyhow!("no [[dav]] source configured"))?;
    let src = resolve(&d, &cfg).await?;
    tokio::task::spawn_blocking(move || add_contact(&src, &name, &email)).await?
}

// ------------------------------------------------------------------- writing events

/// Point the client at a calendar: `(home, remote)` from the cache, or (for entries
/// cached before tern tracked addresses) the discovered home and the calendar's id.
fn aim(c: &mut Client, target: &(String, String)) -> Result<String> {
    let (home, remote) = target;
    match url::Url::parse(home) {
        Ok(u) if !home.is_empty() => {
            c.c.calendar_home_set = Some(u);
            Ok(remote.clone())
        }
        _ => {
            c.home(Kind::Calendars)?;
            Ok(remote.clone())
        }
    }
}

/// Create or replace an event. `etag` guards an update against overwriting a change
/// made elsewhere since the last sync.
pub async fn put_event(cfg: crate::config::Config, source: String, target: (String, String), item: String, ics: String, etag: Option<String>) -> Result<()> {
    let d = cfg.dav_sources().into_iter().find(|d| d.name == source).ok_or_else(|| anyhow!("no [[dav]] source named {source}"))?;
    let src = resolve(&d, &cfg).await?;
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut c = client(&src)?;
        let calendar = aim(&mut c, &target)?;
        let bytes = ics.into_bytes();
        match etag {
            None => {
                c.call("creating event", |c| c.create_item(&calendar, &item, bytes.clone()))?;
            }
            Some(tag) => {
                c.call("saving event", |c| c.update_item(&calendar, &item, bytes.clone(), Some(&tag)))
                    .map_err(|e| anyhow!("{e:#} — it may have changed elsewhere; $ to sync, then try again"))?;
            }
        }
        Ok(())
    })
    .await?
}

pub async fn delete_event(cfg: crate::config::Config, source: String, target: (String, String), item: String, etag: Option<String>) -> Result<()> {
    let d = cfg.dav_sources().into_iter().find(|d| d.name == source).ok_or_else(|| anyhow!("no [[dav]] source named {source}"))?;
    let src = resolve(&d, &cfg).await?;
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut c = client(&src)?;
        let calendar = aim(&mut c, &target)?;
        c.call("deleting event", |c| c.delete_item(&calendar, &item, etag.as_deref()))?;
        Ok(())
    })
    .await?
}

// ------------------------------------------------------------------- writing contacts

/// Create (`existing` None: in the first address book of the first source) or update a
/// card, or delete it (`spec` None).
pub async fn write_contact(
    cfg: crate::config::Config,
    existing: Option<crate::contacts::Contact>,
    spec: Option<crate::contacts::Spec>,
) -> Result<String> {
    let source = match &existing {
        Some(c) => cfg.dav_sources().into_iter().find(|d| d.name == c.source),
        None => cfg.dav_sources().into_iter().next(),
    }
    .ok_or_else(|| anyhow!("no [[dav]] source to keep contacts in"))?;
    let src = resolve(&source, &cfg).await?;
    tokio::task::spawn_blocking(move || -> Result<String> {
        let mut c = client(&src)?;
        c.home(Kind::Contacts)?;
        match (existing, spec) {
            (Some(ct), None) => {
                c.call("deleting contact", |w| w.delete_card(&ct.book, &ct.id, ct.etag.as_deref()))?;
                Ok(format!("deleted {}", ct.name))
            }
            (Some(ct), Some(s)) => {
                let card = crate::contacts::vcard_edit(&ct.raw, &s).into_bytes();
                c.call("saving contact", |w| w.update_card(&ct.book, &ct.id, card.clone(), ct.etag.as_deref()))
                    .map_err(|e| anyhow!("{e:#} — it may have changed elsewhere; gc to sync, then try again"))?;
                Ok(format!("saved {}", s.name))
            }
            (None, Some(s)) => {
                let book = c
                    .call("address books", |w| w.list_addressbooks())?
                    .into_iter()
                    .next()
                    .ok_or_else(|| anyhow!("{} has no address book", src.dav.name))?;
                let uid = uuid::Uuid::new_v4().to_string();
                let card = crate::contacts::vcard_new(&s, &uid).into_bytes();
                c.call("creating contact", |w| w.create_card(&book.id, &format!("{uid}.vcf"), card.clone()))?;
                Ok(format!("added {}", s.name))
            }
            (None, None) => Ok(String::new()),
        }
    })
    .await?
}
