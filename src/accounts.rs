//! Adding and editing accounts from the settings view. A provider preset fills in the
//! servers, including the calendar and contacts servers, so one form sets up mail,
//! calendar and contacts together. Everything is written into config.toml in place,
//! leaving comments and anything else there untouched.

use crate::config::{Config, Tls};
use anyhow::{anyhow, bail, Context, Result};
use std::path::Path;
use toml_edit::{value, DocumentMut, Item, Table};

pub struct Preset {
    pub label: &'static str,
    pub imap: &'static str,
    pub smtp: &'static str,
    pub caldav: &'static str,
    pub carddav: &'static str,
    /// `google` / `microsoft`: sign in with OAuth instead of a password.
    pub oauth: &'static str,
    /// Gmail files sent mail itself; copying it again makes duplicates.
    pub save_sent: bool,
    pub note: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        label: "Fastmail",
        imap: "imap.fastmail.com:993",
        smtp: "smtp.fastmail.com:465",
        caldav: "https://caldav.fastmail.com/",
        carddav: "https://carddav.fastmail.com/",
        oauth: "",
        save_sent: true,
        note: "Make an app password (Settings → Privacy & Security → Manage app passwords) with mail, contacts and calendars access, and store it in your password manager.",
    },
    Preset {
        label: "Google (Gmail)",
        imap: "imap.gmail.com:993",
        smtp: "smtp.gmail.com:465",
        caldav: "https://apidata.googleusercontent.com/caldav/v2/",
        carddav: "https://www.googleapis.com/.well-known/carddav",
        oauth: "google",
        save_sent: false,
        note: "Signs in with OAuth: create a Desktop OAuth client in Google Cloud with the Gmail, CalDAV and CardDAV APIs enabled, then run `tern auth <name>` once.",
    },
    Preset {
        label: "iCloud",
        imap: "imap.mail.me.com:993",
        smtp: "smtp.mail.me.com:587",
        caldav: "https://caldav.icloud.com/",
        carddav: "https://contacts.icloud.com/",
        oauth: "",
        save_sent: true,
        note: "Make an app-specific password at account.apple.com → Sign-In and Security, and store it in your password manager.",
    },
    Preset {
        label: "Microsoft 365 / Outlook",
        imap: "outlook.office365.com:993",
        smtp: "smtp.office365.com:587",
        caldav: "",
        carddav: "",
        oauth: "microsoft",
        save_sent: true,
        note: "Signs in with OAuth: register an app in Azure (redirect http://localhost), then run `tern auth <name>` once. Calendars use Graph, which tern doesn't read yet.",
    },
    Preset {
        label: "Other (IMAP / SMTP)",
        imap: "",
        smtp: "",
        caldav: "",
        carddav: "",
        oauth: "",
        save_sent: true,
        note: "Hosts take an optional port: 993/465 use TLS, 143/587 use STARTTLS.",
    },
];

pub const LABELS: [&str; 10] =
    ["Name", "Email", "Your name", "Password cmd", "IMAP", "SMTP", "Calendar", "Contacts", "OAuth client", "OAuth secret"];
pub const PLACEHOLDERS: [&str; 10] = [
    "Personal, Work…",
    "you@example.com",
    "shown in From",
    "pass show mail/personal · secret-tool lookup …",
    "imap.example.com:993 · imap://host:port for no TLS",
    "smtp.example.com:465 · starttls://host:port",
    "CalDAV URL · empty for none",
    "CardDAV URL · empty for none",
    "OAuth client id (Google / Microsoft)",
    "command printing the client secret",
];

/// What the form holds, in LABELS order, plus the signature from the body.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Spec {
    pub name: String,
    pub email: String,
    pub display_name: String,
    pub password_cmd: String,
    pub imap: String,
    pub smtp: String,
    pub caldav: String,
    pub carddav: String,
    pub client_id: String,
    pub client_secret_cmd: String,
    pub signature: String,
    pub oauth: String,
    pub save_sent: bool,
}

impl Spec {
    pub fn fields(&self) -> Vec<String> {
        vec![
            self.name.clone(),
            self.email.clone(),
            self.display_name.clone(),
            self.password_cmd.clone(),
            self.imap.clone(),
            self.smtp.clone(),
            self.caldav.clone(),
            self.carddav.clone(),
            self.client_id.clone(),
            self.client_secret_cmd.clone(),
        ]
    }

    pub fn from_fields(f: &[String], signature: String, oauth: String, save_sent: bool) -> Spec {
        let g = |i: usize| f.get(i).map(|s| s.trim().to_string()).unwrap_or_default();
        Spec {
            name: g(0),
            email: g(1),
            display_name: g(2),
            password_cmd: g(3),
            imap: g(4),
            smtp: g(5),
            caldav: g(6),
            carddav: g(7),
            client_id: g(8),
            client_secret_cmd: g(9),
            signature,
            oauth,
            save_sent,
        }
    }

    pub fn from_preset(p: &Preset) -> Spec {
        Spec {
            imap: p.imap.into(),
            smtp: p.smtp.into(),
            caldav: p.caldav.into(),
            carddav: p.carddav.into(),
            oauth: p.oauth.into(),
            save_sent: p.save_sent,
            ..Default::default()
        }
    }

    /// The form's view of an account already in the config.
    pub fn from_config(cfg: &Config, i: usize) -> Spec {
        let a = &cfg.accounts[i];
        let dav = |suffix: &str| {
            cfg.dav
                .iter()
                .find(|d| d.account.as_deref() == Some(a.name.as_str()) && d.name.ends_with(suffix))
                .map(|d| d.url.clone())
                .unwrap_or_default()
        };
        Spec {
            name: a.name.clone(),
            email: a.email.clone(),
            display_name: a.display_name.clone().unwrap_or_default(),
            password_cmd: a.imap.secret.password_cmd.clone().unwrap_or_default(),
            imap: host_text("imap", &a.imap.host, a.imap.port, a.imap.tls),
            smtp: host_text("smtp", &a.smtp.host, a.smtp.port, a.smtp.tls),
            caldav: dav("· Calendar"),
            carddav: dav("· Contacts"),
            client_id: a.oauth.as_ref().map(|o| o.client_id.clone()).unwrap_or_default(),
            client_secret_cmd: a.oauth.as_ref().and_then(|o| o.client_secret_cmd.clone()).unwrap_or_default(),
            signature: a.signature.clone().unwrap_or_default(),
            oauth: a.oauth.as_ref().map(|o| o.provider.clone()).unwrap_or_default(),
            save_sent: a.save_sent,
        }
    }

    pub fn validate(&self, cfg: &Config, editing: Option<usize>) -> Result<()> {
        if self.name.is_empty() {
            bail!("the account needs a name");
        }
        if !self.email.contains('@') {
            bail!("the email address looks wrong");
        }
        if self.imap.is_empty() || self.smtp.is_empty() {
            bail!("IMAP and SMTP servers are both needed");
        }
        let clash = cfg.accounts.iter().enumerate().any(|(i, a)| Some(i) != editing && a.name.eq_ignore_ascii_case(&self.name));
        if clash {
            bail!("there's already an account called {}", self.name);
        }
        if self.oauth.is_empty() && self.password_cmd.is_empty() {
            let had_password = editing.is_some_and(|i| cfg.accounts[i].imap.secret.password.is_some());
            if !had_password {
                bail!("give a password command (e.g. pass show mail/{})", self.name.to_lowercase());
            }
        }
        if !self.oauth.is_empty() && self.client_id.is_empty() {
            bail!("{} sign-in needs an OAuth client id", self.oauth);
        }
        Ok(())
    }
}

/// How a server shows in the form: a scheme only when the port alone wouldn't say it.
fn host_text(proto: &str, host: &str, port: u16, tls: Tls) -> String {
    let implied = match port {
        143 | 587 | 25 => Tls::Starttls,
        _ => Tls::Implicit,
    };
    let scheme = match tls {
        _ if tls == implied => String::new(),
        Tls::None => format!("{proto}://"),
        Tls::Starttls => "starttls://".into(),
        Tls::Implicit => format!("{proto}s://"),
    };
    format!("{scheme}{host}:{port}")
}

/// `[scheme://]host[:port]` → host, port, TLS mode. Without a scheme the port decides
/// (143/587 STARTTLS, else TLS); `imap://` / `smtp://` mean no encryption, `starttls://`
/// forces STARTTLS and `imaps://` / `smtps://` force TLS.
pub fn server(s: &str, default_port: u16) -> Result<(String, u16, Tls)> {
    let s = s.trim().trim_end_matches('/');
    let (forced, s) = match s.split_once("://") {
        Some((scheme, rest)) => (
            Some(match scheme.to_lowercase().as_str() {
                "imaps" | "smtps" | "tls" => Tls::Implicit,
                "starttls" => Tls::Starttls,
                "imap" | "smtp" | "plain" => Tls::None,
                other => bail!("unknown scheme {other}:// (imaps, smtps, starttls, imap, smtp)"),
            }),
            rest,
        ),
        None => (None, s),
    };
    let (host, port) = match s.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().with_context(|| format!("port in {s}"))?),
        None => (s.to_string(), default_port),
    };
    if host.is_empty() {
        bail!("missing host");
    }
    let tls = forced.unwrap_or(match port {
        143 | 587 | 25 => Tls::Starttls,
        _ => Tls::Implicit,
    });
    Ok((host, port, tls))
}

fn tls_str(t: Tls) -> &'static str {
    match t {
        Tls::Implicit => "implicit",
        Tls::Starttls => "starttls",
        Tls::None => "none",
    }
}

fn set_or_remove(t: &mut Table, key: &str, v: &str) {
    if v.is_empty() {
        t.remove(key);
    } else {
        t[key] = value(v);
    }
}

fn server_table(t: &mut Table, host: &str, port: u16, tls: Tls, user: &str, cmd: &str) {
    t["host"] = value(host);
    t["port"] = value(port as i64);
    t["tls"] = value(tls_str(tls));
    t["username"] = value(user);
    if !cmd.is_empty() {
        t["password_cmd"] = value(cmd);
        t.remove("password");
    }
}

/// Write a new account (`index` None) or update one, with its calendar and contacts
/// sources. Returns the new file text.
pub fn write(text: &str, index: Option<usize>, old_name: Option<&str>, s: &Spec) -> Result<String> {
    let mut doc: DocumentMut = text.parse().context("parsing config.toml")?;
    let (ih, ip, it) = server(&s.imap, 993)?;
    let (sh, sp, st) = server(&s.smtp, 465)?;

    if doc.get("accounts").is_none() {
        doc["accounts"] = Item::ArrayOfTables(Default::default());
    }
    let accounts = doc["accounts"].as_array_of_tables_mut().ok_or_else(|| anyhow!("`accounts` isn't [[accounts]]"))?;
    let acct: &mut Table = match index {
        Some(i) => accounts.get_mut(i).ok_or_else(|| anyhow!("no account #{i}"))?,
        None => {
            accounts.push(Table::new());
            accounts.get_mut(accounts.len() - 1).unwrap()
        }
    };
    acct["name"] = value(&s.name);
    acct["email"] = value(&s.email);
    set_or_remove(acct, "display_name", &s.display_name);
    acct["save_sent"] = value(s.save_sent);
    set_or_remove(acct, "signature", s.signature.trim_matches('\n').trim_end());
    for (key, host, port, tls) in [("imap", &ih, ip, it), ("smtp", &sh, sp, st)] {
        if !acct.contains_key(key) {
            acct[key] = Item::Table(Table::new());
        }
        let t = acct[key].as_table_mut().ok_or_else(|| anyhow!("[accounts.{key}] isn't a table"))?;
        server_table(t, host, port, tls, &s.email, &s.password_cmd);
    }
    if s.oauth.is_empty() {
        acct.remove("oauth");
    } else {
        if !acct.contains_key("oauth") {
            acct["oauth"] = Item::Table(Table::new());
        }
        let o = acct["oauth"].as_table_mut().ok_or_else(|| anyhow!("[accounts.oauth] isn't a table"))?;
        o["provider"] = value(&s.oauth);
        o["client_id"] = value(&s.client_id);
        set_or_remove(o, "client_secret_cmd", &s.client_secret_cmd);
    }

    // Calendar and contacts sources that belong to this account, named after it.
    if doc.get("dav").is_none() {
        doc["dav"] = Item::ArrayOfTables(Default::default());
    }
    let dav = doc["dav"].as_array_of_tables_mut().ok_or_else(|| anyhow!("`dav` isn't [[dav]]"))?;
    let owner = old_name.unwrap_or(&s.name).to_string();
    for (suffix, url) in [("· Calendar", &s.caldav), ("· Contacts", &s.carddav)] {
        let pos = dav.iter().position(|t| {
            t.get("account").and_then(|v| v.as_str()) == Some(owner.as_str())
                && t.get("name").and_then(|v| v.as_str()).is_some_and(|n| n.ends_with(suffix))
        });
        match (pos, url.is_empty()) {
            (Some(i), true) => dav.remove(i),
            (_, true) => {}
            (pos, false) => {
                let t = match pos {
                    Some(i) => dav.get_mut(i).unwrap(),
                    None => {
                        dav.push(Table::new());
                        dav.get_mut(dav.len() - 1).unwrap()
                    }
                };
                t["name"] = value(format!("{} {suffix}", s.name));
                t["url"] = value(url.as_str());
                t["username"] = value(&s.email);
                t["account"] = value(&s.name);
            }
        }
    }
    // A rename carries the account's other sources with it.
    if old_name.is_some_and(|o| o != s.name) {
        for t in dav.iter_mut() {
            if t.get("account").and_then(|v| v.as_str()) == old_name {
                t["account"] = value(&s.name);
            }
        }
    }
    if dav.is_empty() {
        doc.remove("dav");
    }
    Ok(doc.to_string())
}

/// Remove account `i` and the sources that sign in through it.
pub fn remove(text: &str, i: usize, name: &str) -> Result<String> {
    let mut doc: DocumentMut = text.parse().context("parsing config.toml")?;
    let accounts = doc["accounts"].as_array_of_tables_mut().ok_or_else(|| anyhow!("no [[accounts]]"))?;
    accounts.remove(i);
    if let Some(dav) = doc.get_mut("dav").and_then(|d| d.as_array_of_tables_mut()) {
        dav.retain(|t| t.get("account").and_then(|v| v.as_str()) != Some(name));
    }
    Ok(doc.to_string())
}

pub fn save(path: &Path, text: &str) -> Result<()> {
    // Refuse to write something we can't read back.
    toml::from_str::<Config>(text).context("the result wouldn't parse; not saved")?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CFG: &str = "# my mail\n\n[[accounts]]\nname = \"Old\" # first one\nemail = \"o@x.y\"\n[accounts.imap]\nhost = \"i\"\nusername = \"o\"\npassword = \"p\"\n[accounts.smtp]\nhost = \"s\"\nusername = \"o\"\npassword = \"p\"\n\n[ui]\nicons = true # glyphs\n";

    fn fastmail() -> Spec {
        let mut s = Spec::from_preset(&PRESETS[0]);
        s.name = "Personal".into();
        s.email = "ted@fastmail.com".into();
        s.password_cmd = "pass show mail/fastmail".into();
        s
    }

    #[test]
    fn adds_account_with_calendar_and_contacts() {
        let out = write(CFG, None, None, &fastmail()).unwrap();
        assert!(out.contains("# my mail") && out.contains("# first one") && out.contains("# glyphs"));
        let cfg: Config = toml::from_str(&out).unwrap();
        assert_eq!(cfg.accounts.len(), 2);
        let a = &cfg.accounts[1];
        assert_eq!((a.imap.host.as_str(), a.imap.port, a.smtp.port), ("imap.fastmail.com", 993, 465));
        assert_eq!(a.imap.secret.password_cmd.as_deref(), Some("pass show mail/fastmail"));
        assert_eq!(cfg.dav.len(), 2);
        assert!(cfg.dav.iter().all(|d| d.account.as_deref() == Some("Personal") && d.username == "ted@fastmail.com"));
        assert_eq!(Spec::from_config(&cfg, 1).caldav, "https://caldav.fastmail.com/");
    }

    #[test]
    fn edits_renames_and_removes() {
        let out = write(CFG, None, None, &fastmail()).unwrap();
        let cfg: Config = toml::from_str(&out).unwrap();
        let mut s = Spec::from_config(&cfg, 1);
        s.name = "Home".into();
        s.carddav.clear();
        s.smtp = "smtp.fastmail.com:587".into();
        let out = write(&out, Some(1), Some("Personal"), &s).unwrap();
        let cfg: Config = toml::from_str(&out).unwrap();
        assert_eq!(cfg.accounts[1].name, "Home");
        assert_eq!(cfg.accounts[1].smtp.tls, Tls::Starttls);
        assert_eq!(cfg.dav.len(), 1);
        assert_eq!(cfg.dav[0].account.as_deref(), Some("Home"));
        let out = remove(&out, 1, "Home").unwrap();
        let cfg: Config = toml::from_str(&out).unwrap();
        assert_eq!(cfg.accounts.len(), 1);
        assert!(cfg.dav.is_empty());
        assert!(out.contains("# first one"));
    }

    #[test]
    fn schemes_pick_encryption() {
        assert_eq!(server("imap://127.0.0.1:3143", 993).unwrap(), ("127.0.0.1".into(), 3143, Tls::None));
        assert_eq!(server("starttls://mail.x:2525", 465).unwrap().2, Tls::Starttls);
        assert_eq!(server("smtp.x:587", 465).unwrap().2, Tls::Starttls);
        assert_eq!(server("imap.x", 993).unwrap(), ("imap.x".into(), 993, Tls::Implicit));
        assert_eq!(host_text("imap", "h", 3143, Tls::None), "imap://h:3143");
        assert_eq!(host_text("smtp", "h", 587, Tls::Starttls), "h:587");
    }

    #[test]
    fn google_uses_oauth_and_skips_sent_copies() {
        let mut s = Spec::from_preset(&PRESETS[1]);
        s.name = "Gmail".into();
        s.email = "ted@gmail.com".into();
        s.client_id = "abc.apps.googleusercontent.com".into();
        let out = write(CFG, None, None, &s).unwrap();
        let cfg: Config = toml::from_str(&out).unwrap();
        let a = &cfg.accounts[1];
        assert!(!a.save_sent);
        assert_eq!(a.oauth.as_ref().unwrap().provider, "google");
        assert!(s.validate(&cfg, Some(1)).is_ok());
    }
}

// ------------------------------------------------------------------- calendar & contacts sources

/// Servers for the Calendars tab's chooser: (label, URL).
pub const DAV_PRESETS: &[(&str, &str)] = &[
    ("Fastmail calendars", "https://caldav.fastmail.com/"),
    ("Fastmail contacts", "https://carddav.fastmail.com/"),
    ("iCloud calendars", "https://caldav.icloud.com/"),
    ("iCloud contacts", "https://contacts.icloud.com/"),
    ("Google calendar", "https://apidata.googleusercontent.com/caldav/v2/"),
    ("Google contacts", "https://www.googleapis.com/.well-known/carddav"),
    ("Nextcloud", "https://cloud.example.com/remote.php/dav/"),
    ("Other CalDAV / CardDAV", ""),
];

pub const DAV_LABELS: [&str; 5] = ["Name", "URL", "Username", "Password cmd", "Account"];
pub const DAV_PLACEHOLDERS: [&str; 5] = [
    "Work calendar, Family…",
    "any URL on the server; calendars and address books are found from it",
    "usually your email address",
    "pass show dav/work · empty to use the account's sign-in",
    "a mail account whose sign-in (password or OAuth) to reuse",
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DavSpec {
    pub name: String,
    pub url: String,
    pub username: String,
    pub password_cmd: String,
    pub account: String,
}

impl DavSpec {
    pub fn fields(&self) -> Vec<String> {
        vec![self.name.clone(), self.url.clone(), self.username.clone(), self.password_cmd.clone(), self.account.clone()]
    }

    pub fn from_fields(f: &[String]) -> DavSpec {
        let g = |i: usize| f.get(i).map(|s| s.trim().to_string()).unwrap_or_default();
        DavSpec { name: g(0), url: g(1), username: g(2), password_cmd: g(3), account: g(4) }
    }

    pub fn from_config(cfg: &Config, i: usize) -> DavSpec {
        let d = &cfg.dav[i];
        DavSpec {
            name: d.name.clone(),
            url: d.url.clone(),
            username: d.username.clone(),
            password_cmd: d.secret.password_cmd.clone().unwrap_or_default(),
            account: d.account.clone().unwrap_or_default(),
        }
    }

    pub fn validate(&self, cfg: &Config, editing: Option<usize>) -> Result<()> {
        if self.name.is_empty() {
            bail!("the source needs a name");
        }
        let url: url::Url = self.url.parse().map_err(|_| anyhow!("`{}` isn't a URL", self.url))?;
        if !matches!(url.scheme(), "http" | "https") {
            bail!("the URL should start with https://");
        }
        if url.host_str().is_some_and(|h| h.contains("example.com")) {
            bail!("put your own server in the URL");
        }
        if cfg.dav.iter().enumerate().any(|(i, d)| Some(i) != editing && d.name.eq_ignore_ascii_case(&self.name)) {
            bail!("there's already a source called {}", self.name);
        }
        if !self.account.is_empty() && !cfg.accounts.iter().any(|a| a.name == self.account) {
            bail!("no mail account called {}", self.account);
        }
        if self.account.is_empty() && self.password_cmd.is_empty() {
            let had = editing.is_some_and(|i| cfg.dav[i].secret.password.is_some());
            if !had {
                bail!("give a password command, or the name of the account whose sign-in to use");
            }
        }
        Ok(())
    }
}

fn dav_tables(doc: &mut DocumentMut) -> Result<&mut toml_edit::ArrayOfTables> {
    if doc.get("dav").is_none() {
        doc["dav"] = Item::ArrayOfTables(Default::default());
    }
    doc["dav"].as_array_of_tables_mut().ok_or_else(|| anyhow!("`dav` isn't [[dav]]"))
}

/// Add a source (`index` None) or update one.
pub fn write_dav(text: &str, index: Option<usize>, old_name: Option<&str>, s: &DavSpec) -> Result<String> {
    let mut doc: DocumentMut = text.parse().context("parsing config.toml")?;
    let dav = dav_tables(&mut doc)?;
    let t: &mut Table = match index {
        Some(i) => dav.get_mut(i).ok_or_else(|| anyhow!("no source #{i}"))?,
        None => {
            dav.push(Table::new());
            dav.get_mut(dav.len() - 1).unwrap()
        }
    };
    t["name"] = value(&s.name);
    t["url"] = value(&s.url);
    t["username"] = value(&s.username);
    if !s.password_cmd.is_empty() {
        t["password_cmd"] = value(&s.password_cmd);
        t.remove("password");
    } else if !s.account.is_empty() {
        t.remove("password_cmd");
    }
    set_or_remove(t, "account", &s.account);
    // The default calendar names its source; follow a rename.
    if let (Some(old), Some(ui)) = (old_name.filter(|o| *o != s.name), doc.get_mut("ui").and_then(|u| u.as_table_mut())) {
        if let Some(d) = ui.get("default_calendar").and_then(|v| v.as_str()).map(String::from) {
            if let Some(rest) = d.strip_prefix(&format!("{old}/")) {
                ui["default_calendar"] = value(format!("{}/{rest}", s.name));
            }
        }
    }
    Ok(doc.to_string())
}

pub fn remove_dav(text: &str, i: usize) -> Result<String> {
    let mut doc: DocumentMut = text.parse().context("parsing config.toml")?;
    let dav = dav_tables(&mut doc)?;
    dav.remove(i);
    if dav.is_empty() {
        doc.remove("dav");
    }
    Ok(doc.to_string())
}

/// Show or hide one calendar of a source.
pub fn set_hidden(text: &str, source: &str, calendar: &str, hidden: bool) -> Result<String> {
    let mut doc: DocumentMut = text.parse().context("parsing config.toml")?;
    let dav = dav_tables(&mut doc)?;
    let t = dav
        .iter_mut()
        .find(|t| t.get("name").and_then(|v| v.as_str()) == Some(source))
        .ok_or_else(|| anyhow!("no source called {source} in config.toml"))?;
    let mut list: Vec<String> = t
        .get("hidden")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    list.retain(|x| x != calendar);
    if hidden {
        list.push(calendar.to_string());
    }
    if list.is_empty() {
        t.remove("hidden");
    } else {
        t["hidden"] = value(toml_edit::Array::from_iter(list));
    }
    Ok(doc.to_string())
}

pub fn set_default_calendar(text: &str, key: &str) -> Result<String> {
    let mut doc: DocumentMut = text.parse().context("parsing config.toml")?;
    if doc.get("ui").is_none() {
        doc["ui"] = toml_edit::table();
    }
    let ui = doc["ui"].as_table_mut().ok_or_else(|| anyhow!("[ui] isn't a table"))?;
    ui["default_calendar"] = value(key);
    Ok(doc.to_string())
}

#[cfg(test)]
mod dav_tests {
    use super::*;

    const CFG: &str = "# keep\n[[accounts]]\nname = \"P\"\nemail = \"p@x.y\"\n[accounts.imap]\nhost = \"i\"\nusername = \"p\"\npassword = \"p\"\n[accounts.smtp]\nhost = \"s\"\nusername = \"p\"\npassword = \"p\"\n";

    #[test]
    fn add_hide_default_rename_remove() {
        let s = DavSpec { name: "Family".into(), url: "https://caldav.icloud.com/".into(), username: "me@icloud.com".into(), password_cmd: "pass icloud".into(), account: String::new() };
        let out = write_dav(CFG, None, None, &s).unwrap();
        let out = set_hidden(&out, "Family", "birthdays", true).unwrap();
        let out = set_default_calendar(&out, "Family/home").unwrap();
        let cfg: Config = toml::from_str(&out).unwrap();
        assert_eq!(cfg.dav[0].hidden, vec!["birthdays"]);
        assert_eq!(cfg.ui.default_calendar.as_deref(), Some("Family/home"));
        assert!(out.starts_with("# keep"));

        let mut s2 = DavSpec::from_config(&cfg, 0);
        s2.name = "Home".into();
        let out = write_dav(&out, Some(0), Some("Family"), &s2).unwrap();
        let cfg: Config = toml::from_str(&out).unwrap();
        assert_eq!(cfg.dav[0].name, "Home");
        assert_eq!(cfg.ui.default_calendar.as_deref(), Some("Home/home"));
        assert_eq!(cfg.dav[0].hidden, vec!["birthdays"]);

        let out = set_hidden(&out, "Home", "birthdays", false).unwrap();
        assert!(!out.contains("hidden"));
        let out = remove_dav(&out, 0).unwrap();
        let cfg: Config = toml::from_str(&out).unwrap();
        assert!(cfg.dav.is_empty());
    }

    #[test]
    fn validates() {
        let cfg: Config = toml::from_str(CFG).unwrap();
        let mut s = DavSpec { name: "W".into(), url: "caldav.x.com".into(), username: "u".into(), ..Default::default() };
        assert!(s.validate(&cfg, None).is_err());
        s.url = "https://caldav.x.com/".into();
        assert!(s.validate(&cfg, None).is_err(), "needs credentials");
        s.account = "P".into();
        assert!(s.validate(&cfg, None).is_ok());
        s.account = "Nope".into();
        assert!(s.validate(&cfg, None).is_err());
    }
}
