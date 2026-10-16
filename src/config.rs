use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    #[serde(default, rename = "accounts")]
    pub accounts: Vec<Account>,
    /// CalDAV / CardDAV servers: calendars and address books from each.
    #[serde(default, rename = "dav")]
    pub dav: Vec<Dav>,
    /// Older single-address-book form, still read.
    #[serde(default)]
    pub carddav: Option<CardDav>,
    #[serde(default)]
    pub ui: Ui,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Account {
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub display_name: Option<String>,
    pub imap: Imap,
    pub smtp: Smtp,
    /// Folder overrides for servers without SPECIAL-USE.
    #[serde(default)]
    pub folders: FolderNames,
    /// Copy sent mail into the Sent folder (turn off for Gmail, which does it itself).
    #[serde(default = "yes")]
    pub save_sent: bool,
    #[serde(default)]
    pub signature: Option<String>,
    /// Sign in with OAuth instead of a password (Gmail, Outlook / Microsoft 365).
    #[serde(default)]
    pub oauth: Option<OAuth>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct OAuth {
    /// `google`, `microsoft`, or `custom` (then give the endpoints and scopes below).
    pub provider: String,
    pub client_id: String,
    /// Google requires one even for desktop apps; Microsoft public clients don't.
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub client_secret_cmd: Option<String>,
    /// Microsoft tenant; `common` works for personal and most work accounts.
    #[serde(default)]
    pub tenant: Option<String>,
    #[serde(default)]
    pub auth_url: Option<String>,
    #[serde(default)]
    pub token_url: Option<String>,
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
}

impl OAuth {
    pub fn secret(&self) -> Result<Option<String>> {
        if let Some(cmd) = &self.client_secret_cmd {
            let s = Secret { password: None, password_cmd: Some(cmd.clone()) };
            return s.resolve().map(Some);
        }
        Ok(self.client_secret.clone())
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct FolderNames {
    pub sent: Option<String>,
    pub drafts: Option<String>,
    pub archive: Option<String>,
    pub trash: Option<String>,
    pub junk: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Imap {
    pub host: String,
    #[serde(default = "imap_port")]
    pub port: u16,
    pub username: String,
    #[serde(flatten)]
    pub secret: Secret,
    #[serde(default)]
    pub tls: Tls,
    /// How many of the newest messages to load per folder.
    #[serde(default = "page")]
    pub page_size: u32,
    /// Seconds between checks for new mail.
    #[serde(default = "poll")]
    pub poll_secs: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Smtp {
    pub host: String,
    #[serde(default = "smtp_port")]
    pub port: u16,
    pub username: String,
    #[serde(flatten)]
    pub secret: Secret,
    #[serde(default)]
    pub tls: Tls,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Dav {
    pub name: String,
    /// Any URL on the server; calendars and address books are discovered from it.
    pub url: String,
    pub username: String,
    #[serde(flatten)]
    pub secret: Secret,
    /// A mail account whose sign-in to reuse: its OAuth token, or its IMAP password when
    /// this source has none of its own.
    #[serde(default)]
    pub account: Option<String>,
    /// Calendars (by id) on this server not shown in the agenda.
    #[serde(default)]
    pub hidden: Vec<String>,
}

impl Config {
    /// Every DAV source, with an old `[carddav]` block counted as one.
    pub fn dav_sources(&self) -> Vec<Dav> {
        let mut v = self.dav.clone();
        if let Some(cd) = &self.carddav {
            v.push(Dav {
                name: "Contacts".into(),
                url: cd.url.clone(),
                username: cd.username.clone(),
                secret: cd.secret.clone(),
                account: None,
                hidden: vec![],
            });
        }
        v
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct CardDav {
    /// The address book collection URL.
    pub url: String,
    pub username: String,
    #[serde(flatten)]
    pub secret: Secret,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Secret {
    #[serde(default)]
    pub password: Option<String>,
    /// A shell command whose stdout is the password, e.g. `pass show mail/work`.
    #[serde(default)]
    pub password_cmd: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Tls {
    /// TLS from the first byte (IMAP 993, SMTP 465).
    #[default]
    Implicit,
    /// Plain connection upgraded with STARTTLS (IMAP 143, SMTP 587).
    Starttls,
    /// No encryption. Only for local bridges such as Proton Bridge.
    None,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Ui {
    #[serde(default = "sidebar")]
    pub sidebar_width: u16,
    /// Percent of the remaining width given to the message list.
    #[serde(default = "list_pct")]
    pub list_percent: u16,
    /// Nerd Font glyphs (Omarchy ships JetBrainsMono Nerd Font).
    #[serde(default = "yes")]
    pub icons: bool,
    /// Paint the theme background instead of leaving the terminal's (keeps transparency).
    #[serde(default)]
    pub paint_background: bool,
    /// Override the theme directory; defaults to ~/.config/omarchy/current/theme.
    #[serde(default)]
    pub theme_dir: Option<PathBuf>,
    #[serde(default = "date_fmt")]
    pub date_format: String,
    #[serde(default)]
    pub editor: Option<String>,
    #[serde(default = "download_dir")]
    pub download_dir: PathBuf,
    /// How long a message must stay open before it is marked read, so skimming past
    /// with j/k doesn't. 0 marks immediately.
    #[serde(default = "mark_read_ms")]
    pub mark_read_ms: u64,
    /// Click and scroll. Off gives the terminal back its own text selection.
    #[serde(default = "yes")]
    pub mouse: bool,
    /// Draw pictures in messages (kitty, sixel or iTerm2 graphics; half blocks elsewhere).
    #[serde(default = "yes")]
    pub images: bool,
    /// One line per message in the list instead of three (sender, subject, preview).
    #[serde(default)]
    pub compact_list: bool,
    /// Group the list into conversations.
    #[serde(default = "yes")]
    pub threads: bool,
    /// Show each conversation as one row until opened with `za`.
    #[serde(default = "yes")]
    pub collapse_threads: bool,
    /// Calendar times are shown and entered in this zone (IANA name or est/pst/…).
    #[serde(default = "calendar_timezone")]
    pub calendar_timezone: String,
    /// Seconds a sent message waits in the outbox, undoable with ^z. 0 sends at once.
    #[serde(default = "undo_send")]
    pub undo_send_secs: u64,
    /// Show your next meetings under the folders in the mail view.
    #[serde(default = "yes")]
    pub agenda_in_mail: bool,
    /// Desktop notifications for new mail and upcoming meetings.
    #[serde(default = "yes")]
    pub notifications: bool,
    /// Minutes before a meeting to remind you, when it has no alarm of its own. 0 = only
    /// events' own alarms.
    #[serde(default = "reminder_minutes")]
    pub reminder_minutes: u64,
    /// Your personal Zoom meeting link, put on events with `gz`.
    #[serde(default)]
    pub zoom_link: Option<String>,
    /// The Google account `gm` creates Meet links with (default: the first Google one).
    #[serde(default)]
    pub meet_account: Option<String>,
    /// Where new events and accepted invitations go: "<source>/<calendar id>".
    #[serde(default)]
    pub default_calendar: Option<String>,
}

impl Default for Ui {
    fn default() -> Self {
        toml::from_str("").unwrap()
    }
}

fn yes() -> bool {
    true
}
fn imap_port() -> u16 {
    993
}
fn smtp_port() -> u16 {
    465
}
fn page() -> u32 {
    300
}
fn poll() -> u64 {
    60
}
fn sidebar() -> u16 {
    24
}
fn list_pct() -> u16 {
    40
}
fn reminder_minutes() -> u64 {
    10
}
fn undo_send() -> u64 {
    10
}
fn calendar_timezone() -> String {
    "America/New_York".into()
}
fn mark_read_ms() -> u64 {
    800
}
fn date_fmt() -> String {
    "%b %d".into()
}
fn download_dir() -> PathBuf {
    dirs::download_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Downloads"))
}

impl Secret {
    pub fn resolve(&self) -> Result<String> {
        if let Some(cmd) = &self.password_cmd {
            let out = Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .output()
                .with_context(|| format!("running password_cmd `{cmd}`"))?;
            if !out.status.success() {
                bail!(
                    "password_cmd `{cmd}` failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            let s = String::from_utf8(out.stdout)?;
            return Ok(s.lines().next().unwrap_or("").to_string());
        }
        if let Some(p) = &self.password {
            return Ok(p.clone());
        }
        bail!("no password or password_cmd configured")
    }
}

impl Account {
    pub fn sender_mailbox(&self) -> String {
        match &self.display_name {
            Some(n) if !n.is_empty() => format!("{n} <{}>", self.email),
            _ => self.email.clone(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_default().join("tern")
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_default().join("tern")
}

pub fn default_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn load(path: &PathBuf) -> Result<Config> {
    let text = std::fs::read_to_string(path).with_context(|| {
        format!(
            "reading {} (run `tern --init` to write an example, or `tern --demo` to look around)",
            path.display()
        )
    })?;
    let cfg: Config = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    if cfg.accounts.is_empty() {
        bail!("{} has no [[accounts]]", path.display());
    }
    Ok(cfg)
}

pub const EXAMPLE: &str = r#"# tern — ~/.config/tern/config.toml

[[accounts]]
name = "Personal"
email = "you@example.com"
display_name = "Your Name"
save_sent = true          # false for Gmail: it files sent mail itself
# signature = "-- \nYour Name"

[accounts.imap]
host = "imap.fastmail.com"
port = 993
tls = "implicit"          # implicit | starttls | none
username = "you@example.com"
password_cmd = "pass show mail/personal"   # or: password = "..."

[accounts.smtp]
host = "smtp.fastmail.com"
port = 465
tls = "implicit"          # 587 → starttls
username = "you@example.com"
password_cmd = "pass show mail/personal"

# Gmail / Outlook: sign in with OAuth instead of a password, then run `tern auth Personal`.
# Bring your own OAuth client id (Google Cloud console / Azure app registration,
# desktop app, redirect http://127.0.0.1 or http://localhost).
# [accounts.oauth]
# provider = "google"              # or "microsoft" (add tenant = "..." for work accounts)
# client_id = "…apps.googleusercontent.com"
# client_secret_cmd = "pass show oauth/google-client-secret"

# Only needed when the server lacks SPECIAL-USE folder flags.
# [accounts.folders]
# archive = "Archive"
# trash = "Trash"

# Calendars and contacts. Add one [[dav]] per server; each server's calendars and
# address books are found automatically.
# [[dav]]
# name = "Fastmail"
# url = "https://caldav.fastmail.com/"
# username = "you@example.com"
# password_cmd = "pass show mail/personal"   # or account = "Personal" to reuse its sign-in

[ui]
sidebar_width = 24
list_percent = 40
icons = true              # Nerd Font glyphs
paint_background = false  # keep the terminal's (transparent) background
mark_read_ms = 800        # dwell before a message counts as read; 0 = instantly
mouse = true              # click/scroll; drag to select and copy
images = true             # pictures in messages; remote ones load with I
threads = true            # group into conversations (T toggles)
calendar_timezone = "America/New_York"   # or est, pst, Europe/Berlin, …
collapse_threads = true   # one row per conversation (za / zR / zM)
# theme_dir = "~/.config/omarchy/current/theme"
"#;
