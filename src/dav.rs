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
        host: url.host_str().unwrap_or_default().to_string(),
        port: url.port_or_known_default().unwrap_or(443),
        https: url.scheme() == "https",
        name: src.dav.name.clone(),
    })
}

impl Client {
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

fn last_segment(href: &str) -> String {
    href.trim_end_matches('/').rsplit('/').next().unwrap_or(href).to_string()
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
            c.call("address books", |c| c.addressbook_home_set())?;
            for book in c.call("address books", |c| c.list_addressbooks())? {
                let cards = c.call(&book.id, |c| c.list_cards(&book.id, &Default::default()))?;
                for card in cards.cards {
                    all.extend(parse_vcards(&String::from_utf8_lossy(&card.data)));
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
    all.retain(|c| !c.emails.is_empty());
    all.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok((all, errors))
}

/// Create a contact in the first address book of `src`.
pub fn add_contact(src: &Source, name: &str, email: &str) -> Result<()> {
    let mut c = client(src)?;
    c.call("address books", |c| c.addressbook_home_set())?;
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
    /// Resource id → raw iCalendar.
    pub items: BTreeMap<String, String>,
    /// Resource id → ETag, to fetch only what changed when diffing a full listing.
    #[serde(default)]
    pub etags: BTreeMap<String, String>,
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

/// Bring the cache up to date with every source: only what changed since the last sync
/// token when the server supports it, a full listing when it doesn't.
pub fn sync_calendars(sources: &[Source]) -> Result<(Vec<CachedCalendar>, Vec<String>)> {
    let old = load_cache();
    let mut out: Vec<CachedCalendar> = Vec::new();
    let mut errors = Vec::new();
    for src in sources {
        let r: Result<()> = (|| {
            let mut c = client(src)?;
            c.call("calendars", |c| c.calendar_home_set())?;
            for cal in c.call("calendars", |c| c.list_calendars())? {
                if !cal.components.is_empty() && !cal.components.iter().any(|x| x.eq_ignore_ascii_case("VEVENT")) {
                    continue;
                }
                let mut entry = old
                    .iter()
                    .find(|o| o.source == src.dav.name && o.id == cal.id)
                    .cloned()
                    .unwrap_or_default();
                entry.source = src.dav.name.clone();
                entry.id = cal.id.clone();
                entry.name = cal.display_name.clone().unwrap_or_else(|| cal.id.clone());
                entry.color = cal.color.clone();

                // Incremental REPORT first. Without a token the server lists everything,
                // which is a full snapshot; with one it sends only changes and deletions.
                let token = entry.sync_token.clone();
                let report = c.call("sync", |c| c.sync_items(&cal.id, token.as_deref(), WebdavSyncCollectionOptions::default()));
                let (delta, snapshot) = match report {
                    Ok(d) if !d.truncated => {
                        let full = token.is_none();
                        (d, full)
                    }
                    // Unsupported REPORT, an expired token, or a truncated answer: list the
                    // collection and diff it ourselves.
                    _ => {
                        let opts = WebdavSyncCollectionOptions { fallback: true };
                        (c.call("listing", |c| c.sync_items(&cal.id, None, opts))?, true)
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
                    for item in c.call(&entry.name, |c| c.multiget_items(&cal.id, &refs))? {
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
