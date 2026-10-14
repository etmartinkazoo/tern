//! `tern doctor`: try everything tern depends on — the config, each account's password,
//! IMAP and SMTP sign-in, OAuth tokens, calendar and contact servers, and the local tools
//! — and say plainly what's wrong and what to do about it.

use crate::config::{self, Account, Config, Secret, Tls};
use std::io::IsTerminal;
use std::path::Path;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(20);

struct Out {
    color: bool,
    failed: usize,
    warned: usize,
}

impl Out {
    fn paint(&self, code: &str, s: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    fn heading(&self, s: &str) {
        println!("\n{}", self.paint("1", s));
    }
    fn ok(&self, what: &str, detail: &str) {
        println!("  {} {what:<11} {}", self.paint("32", "✓"), self.paint("2", detail));
    }
    fn warn(&mut self, what: &str, detail: &str, hint: Option<&str>) {
        self.warned += 1;
        println!("  {} {what:<11} {detail}", self.paint("33", "!"));
        if let Some(h) = hint {
            println!("    {} {}", self.paint("2", "→"), h);
        }
    }
    fn fail(&mut self, what: &str, detail: &str, hint: Option<&str>) {
        self.failed += 1;
        println!("  {} {what:<11} {}", self.paint("31", "✗"), self.paint("31", detail));
        if let Some(h) = hint {
            println!("    {} {}", self.paint("2", "→"), h);
        }
    }
}

/// Advice for the errors people actually hit.
fn hint(err: &str, port_tls: Option<(u16, &Tls)>) -> Option<String> {
    let e = err.to_lowercase();
    if e.contains("authenticationfailed") || e.contains("invalid credentials") || e.contains("authentication failed")
        || e.contains("535") || e.contains("login failed") || e.contains("401") || e.contains("unauthorized")
    {
        return Some("the server refused the sign-in: check the username and password. Fastmail, iCloud and Gmail (without OAuth) need an app password, not your normal one".into());
    }
    if e.contains("password_cmd") {
        return Some("run that command yourself: it must print the password on its first line and exit 0 (is the keyring unlocked?)".into());
    }
    if e.contains("isn't signed in") || e.contains("refresh refused") || e.contains("token expired") {
        return Some("sign in again with `tern auth <account>`".into());
    }
    if e.contains("dns") || e.contains("failed to lookup") || e.contains("name or service not known") || e.contains("no such host") {
        return Some("that host name doesn't resolve: check `host` for typos, and that you're online".into());
    }
    if e.contains("connection refused") {
        return Some("nothing is listening there: check `host` and `port`".into());
    }
    if e.contains("timed out") || e.contains("elapsed") {
        return Some("no answer in time: a firewall, a wrong port, or the server is down".into());
    }
    if e.contains("tls") || e.contains("certificate") || e.contains("handshake") || e.contains("corrupt message") || e.contains("invalidcontenttype") {
        if let Some((port, tls)) = port_tls {
            let expect = match tls {
                Tls::Implicit => "implicit TLS normally uses 993 (IMAP) or 465 (SMTP)",
                Tls::Starttls => "STARTTLS normally uses 143 (IMAP) or 587 (SMTP)",
                Tls::None => "tls = \"none\" is only for local bridges",
            };
            return Some(format!("TLS failed on port {port}: {expect}; set `tls` to match the port"));
        }
        return Some("TLS failed: check the port and `tls` setting".into());
    }
    if e.contains("connecting to") || e.contains("connect ") {
        return Some("can't reach that server: check the URL, and that you're online".into());
    }
    if e.contains("404") || e.contains("not found") {
        return Some("the server has no calendars or contacts there: use the provider's DAV URL (Fastmail: https://caldav.fastmail.com or https://carddav.fastmail.com, Google: https://www.google.com/calendar/dav)".into());
    }
    None
}

fn first_line(e: &anyhow::Error) -> String {
    let s = format!("{e:#}");
    let s = s.lines().next().unwrap_or("").to_string();
    if s.chars().count() > 160 { format!("{}…", s.chars().take(160).collect::<String>()) } else { s }
}

fn secret_source(s: &Secret) -> &'static str {
    if s.password_cmd.is_some() {
        "password_cmd"
    } else if s.password.is_some() {
        "password in config"
    } else {
        "none"
    }
}

async fn timed<T>(f: impl std::future::Future<Output = anyhow::Result<T>>) -> anyhow::Result<T> {
    tokio::time::timeout(TIMEOUT, f).await.map_err(|_| anyhow::anyhow!("timed out after {}s", TIMEOUT.as_secs()))?
}

/// Returns true when nothing failed (warnings are fine).
pub async fn run(path: &Path) -> bool {
    let mut o = Out { color: std::io::stdout().is_terminal(), failed: 0, warned: 0 };
    println!("{} {}", o.paint("1", "tern doctor"), o.paint("2", &path.display().to_string()));

    o.heading("Config");
    let cfg: Config = match config::load(&path.to_path_buf()) {
        Ok(c) => c,
        Err(e) => {
            o.fail("config", &first_line(&e), Some("`tern --init` writes an example; fix the line named above"));
            return false;
        }
    };
    o.ok("config", &format!("{} account(s), {} calendar/contact server(s)", cfg.accounts.len(), cfg.dav_sources().len()));
    let mut names = std::collections::HashSet::new();
    for a in &cfg.accounts {
        if !names.insert(a.name.to_lowercase()) {
            o.fail("accounts", &format!("two accounts are named {}", a.name), Some("names must be unique"));
        }
        if !a.email.contains('@') {
            o.fail("accounts", &format!("{}: email `{}` isn't an address", a.name, a.email), None);
        }
    }

    for a in &cfg.accounts {
        account(&mut o, a).await;
    }

    let sources = cfg.dav_sources();
    if !sources.is_empty() {
        o.heading("Calendars & contacts");
    }
    for d in &sources {
        if let Some(acct) = &d.account {
            if !cfg.accounts.iter().any(|a| &a.name == acct) {
                o.fail(&d.name, &format!("account = \"{acct}\" doesn't match any account"), None);
                continue;
            }
        }
        let src = match timed(crate::dav::resolve(d, &cfg)).await {
            Ok(s) => s,
            Err(e) => {
                let msg = first_line(&e);
                o.fail(&d.name, &format!("credentials: {msg}"), hint(&msg, None).as_deref());
                continue;
            }
        };
        let (cals, books) = tokio::task::spawn_blocking(move || crate::dav::probe(&src)).await.unwrap_or_else(|e| {
            (Err(anyhow::anyhow!("{e}")), Err(anyhow::anyhow!("{e}")))
        });
        match (cals, books) {
            (Err(c), Err(b)) => {
                let (c, b) = (first_line(&c), first_line(&b));
                o.fail(&d.name, &format!("{} — calendars: {c}", d.url), hint(&c, None).as_deref());
                if b != c {
                    o.fail("", &format!("contacts: {b}"), None);
                }
            }
            (c, b) => {
                let part = |r: &anyhow::Result<usize>, what: &str| match r {
                    Ok(n) => format!("{n} {what}"),
                    Err(_) => format!("no {what}"),
                };
                o.ok(&d.name, &format!("{} · {} · {}", d.url, part(&c, "calendar(s)"), part(&b, "address book(s)")));
            }
        }
    }
    if let Some(dc) = &cfg.ui.default_calendar {
        let cached = crate::dav::load_cache();
        if !cached.is_empty() && !cached.iter().any(|c| format!("{}/{}", c.source, c.id) == *dc) {
            o.warn("calendar", &format!("default_calendar \"{dc}\" isn't one of your calendars"), Some("pick one under Settings → Calendars"));
        }
    }

    o.heading("This machine");
    local(&mut o, &cfg);

    o.heading("Waiting");
    let queued = crate::outbox::list();
    let stuck: Vec<_> = queued.iter().filter(|q| q.last_error.is_some()).collect();
    if queued.is_empty() {
        o.ok("outbox", "empty");
    } else if stuck.is_empty() {
        o.ok("outbox", &format!("{} waiting to send", queued.len()));
    } else {
        for q in &stuck {
            o.warn(
                "outbox",
                &format!("“{}” to {} — tried {}×: {}", q.subject, q.to.join(", "), q.attempts, q.last_error.as_deref().unwrap_or("")),
                Some("open tern and press `go` to retry or edit it"),
            );
        }
    }
    let snoozed = crate::snooze::load();
    if !snoozed.is_empty() {
        let overdue = snoozed.iter().filter(|s| s.until < crate::outbox::now() - 3600).count();
        if overdue > 0 {
            o.warn("snoozed", &format!("{overdue} overdue by more than an hour"), Some("they come back while tern is running; open it"));
        } else {
            o.ok("snoozed", &format!("{} message(s)", snoozed.len()));
        }
    }
    if crate::drafts::load().is_some() {
        o.ok("draft", "an unsent message is saved — `gr` in tern resumes it");
    }

    println!();
    if o.failed == 0 && o.warned == 0 {
        println!("{}", o.paint("32", "Everything works."));
    } else {
        println!(
            "{}",
            o.paint(if o.failed > 0 { "31" } else { "33" }, &format!("{} problem(s), {} warning(s).", o.failed, o.warned))
        );
    }
    o.failed == 0
}

async fn account(o: &mut Out, a: &Account) {
    o.heading(&format!("{} ({})", a.name, a.email));
    if let Some(oa) = &a.oauth {
        if let Err(e) = crate::oauth::provider(oa) {
            return o.fail("oauth", &first_line(&e), None);
        }
        match crate::oauth::stored(&a.name) {
            None => return o.fail("oauth", "not signed in", Some(&format!("run `tern auth {}`", a.name))),
            Some(false) => o.warn("oauth", "signed in, but without a refresh token", Some(&format!("it will stop working within the hour; run `tern auth {}`", a.name))),
            Some(true) => match timed(crate::oauth::access_token(&a.name, oa)).await {
                Ok(_) => o.ok("oauth", &format!("{} token valid", oa.provider)),
                Err(e) => {
                    let m = first_line(&e);
                    return o.fail("oauth", &m, hint(&m, None).as_deref());
                }
            },
        }
    } else {
        for (what, s) in [("imap pass", &a.imap.secret), ("smtp pass", &a.smtp.secret)] {
            match s.resolve() {
                Ok(p) if p.is_empty() => o.fail(what, &format!("{} gave an empty password", secret_source(s)), None),
                Ok(_) => {}
                Err(e) => {
                    let m = first_line(&e);
                    o.fail(what, &m, hint(&m, None).as_deref());
                }
            }
        }
        if a.imap.secret.password.is_some() || a.smtp.secret.password.is_some() {
            o.warn("password", "stored in plain text in the config", Some("use password_cmd = \"pass show mail/…\" (or secret-tool, op read)"));
        }
    }

    let i = &a.imap;
    match timed(crate::imap::connect(a)).await {
        Ok(c) => {
            let mut has = vec![];
            for cap in ["IDLE", "MOVE", "UIDPLUS", "SPECIAL-USE", "CONDSTORE"] {
                if c.has(cap) {
                    has.push(cap);
                }
            }
            o.ok("imap", &format!("{}:{} · signed in · {}", i.host, i.port, if has.is_empty() { "basic".into() } else { has.join(" ") }));
            if !c.has("IDLE") {
                o.warn("push", &format!("no IDLE: new mail is checked every {}s instead of pushed", i.poll_secs), None);
            }
        }
        Err(e) => {
            let m = first_line(&e);
            o.fail("imap", &format!("{}:{} — {m}", i.host, i.port), hint(&m, Some((i.port, &i.tls))).as_deref());
        }
    }
    let s = &a.smtp;
    match timed(crate::compose::check_smtp(a)).await {
        Ok(()) => o.ok("smtp", &format!("{}:{} · signed in", s.host, s.port)),
        Err(e) => {
            let m = first_line(&e);
            o.fail("smtp", &format!("{}:{} — {m}", s.host, s.port), hint(&m, Some((s.port, &s.tls))).as_deref());
        }
    }
}

fn local(o: &mut Out, cfg: &Config) {
    // Theme.
    let dir = cfg.ui.theme_dir.clone().map(|p| crate::theme::expand(&p.to_string_lossy())).or_else(crate::theme::default_dir);
    match dir {
        Some(d) if d.exists() => {
            let t = crate::theme::Theme::load(Some(&d));
            if t.name == "fallback" {
                o.warn("theme", &format!("{} has no colors tern can read", d.display()), Some("it reads colors.toml, alacritty.toml, kitty.conf or ghostty.conf"));
            } else {
                o.ok("theme", &format!("{} ({})", t.name, d.display()));
            }
        }
        Some(d) => o.warn("theme", &format!("{} doesn't exist; using the built-in colors", d.display()), None),
        None => o.ok("theme", "built-in colors (no Omarchy theme found)"),
    }
    // Time zone.
    match crate::calendar::parse_zone(&cfg.ui.calendar_timezone) {
        Some(z) => o.ok("time zone", &z.to_string()),
        None => o.fail("time zone", &format!("calendar_timezone = \"{}\" isn't a zone", cfg.ui.calendar_timezone), Some("use an IANA name like America/New_York, or est/cst/mst/pst")),
    }
    // Editor (only used for `E`, the full-screen editor).
    let editor = cfg.ui.editor.clone().or_else(|| std::env::var("VISUAL").ok()).or_else(|| std::env::var("EDITOR").ok()).filter(|e| !e.trim().is_empty());
    match editor {
        Some(e) => {
            let bin = e.split_whitespace().next().unwrap_or("").to_string();
            if crate::on_path(&bin) || Path::new(&bin).is_file() {
                o.ok("editor", &e);
            } else {
                o.warn("editor", &format!("`{bin}` isn't on PATH"), Some("set `editor` under [ui]"));
            }
        }
        None if ["nvim", "vim", "vi", "nano"].iter().any(|e| crate::on_path(e)) => o.ok("editor", "found one on PATH"),
        None => o.warn("editor", "none found", Some("set $EDITOR; the built-in composer still works")),
    }
    for (bin, why) in [
        ("notify-send", "desktop notifications"),
        ("wl-copy", "copying addresses and links"),
        ("xdg-open", "opening HTML mail, links and OAuth sign-in"),
    ] {
        if crate::on_path(bin) {
            o.ok(bin, why);
        } else if bin == "notify-send" && !cfg.ui.notifications {
            o.ok(bin, "not installed (notifications are off)");
        } else {
            o.warn(bin, &format!("not installed — needed for {why}"), None);
        }
    }
    if let Some(z) = &cfg.ui.zoom_link {
        if !z.starts_with("https://") {
            o.warn("zoom", &format!("zoom_link `{z}` isn't an https link"), None);
        } else {
            o.ok("zoom", z);
        }
    }
    // Writable data and cache.
    for (what, d) in [("cache", config::cache_dir()), ("data", dirs::data_dir().unwrap_or_default().join("tern"))] {
        let probe = d.join(".doctor");
        match std::fs::create_dir_all(&d).and_then(|_| std::fs::write(&probe, b"")) {
            Ok(()) => {
                let _ = std::fs::remove_file(probe);
                o.ok(what, &d.display().to_string());
            }
            Err(e) => o.fail(what, &format!("{} isn't writable: {e}", d.display()), None),
        }
    }
}
