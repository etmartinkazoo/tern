//! Plugins: programs outside tern that act on mail.
//!
//! Each lives in `~/.config/tern/plugins/<name>/` with a `plugin.toml`:
//!
//! ```toml
//! name = "todo"
//! description = "Turn email into todos"
//! run = ["./run"]                 # relative to the plugin's folder
//! timeout_secs = 60
//!
//! [[actions]]
//! id = "create"
//! title = "Send to Runwell as a todo"
//! key = "t"                       # ;t runs it (every action is also under :)
//! on = "message"                  # message (or the marked ones) | marked (needs marks)
//! accounts = ["Brem", "Lumin"]    # only offer it on these accounts (empty = all)
//! attachments = false             # write attachments to files for the plugin
//! fields = [{ name = "title", default = "{subject}" }, { name = "due", label = "due (optional)" }]
//! ```
//!
//! tern runs `run` in the plugin's folder with one JSON object on stdin (the action, the
//! filled-in fields, the account and the messages) and reads JSON lines from stdout:
//! `{"status": "…"}`, `{"done": "…", "url": "…"}`, `{"error": "…"}`, `{"then": ["archive"]}`
//! (archive | read | flag), `{"open": "https://…"}`. Plain lines count as status. The plugin
//! runs in the background; tern never waits on it, and kills it after `timeout_secs`.

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Deserialize)]
pub struct Field {
    pub name: String,
    #[serde(default)]
    pub label: String,
    /// Pre-filled value; {subject} {from} {from_name} {from_addr} {date} {account} expand.
    #[serde(default)]
    pub default: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActionDef {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default = "default_on")]
    pub on: String,
    #[serde(default)]
    pub accounts: Vec<String>,
    #[serde(default)]
    pub attachments: bool,
    #[serde(default)]
    pub fields: Vec<Field>,
}

fn default_on() -> String {
    "message".into()
}

#[derive(Debug, Deserialize)]
struct Manifest {
    name: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default = "default_run")]
    run: Vec<String>,
    #[serde(default = "default_timeout")]
    timeout_secs: u64,
    #[serde(default)]
    actions: Vec<ActionDef>,
}

fn default_run() -> Vec<String> {
    vec!["./run".into()]
}

fn default_timeout() -> u64 {
    60
}

#[derive(Debug, Clone)]
pub struct Plugin {
    pub name: String,
    pub description: String,
    pub dir: PathBuf,
    pub run: Vec<String>,
    pub timeout: Duration,
    pub actions: Vec<ActionDef>,
}

impl ActionDef {
    pub fn key_char(&self) -> Option<char> {
        self.key.as_deref().and_then(|k| k.chars().next())
    }

    pub fn offered_for(&self, account: &str) -> bool {
        self.accounts.is_empty() || self.accounts.iter().any(|a| a.eq_ignore_ascii_case(account))
    }
}

pub fn dir() -> PathBuf {
    crate::config::config_dir().join("plugins")
}

/// Every plugin with a readable manifest, by name. Broken ones come back as errors
/// (for `tern doctor` and the status line) rather than stopping the rest.
pub fn load() -> (Vec<Plugin>, Vec<String>) {
    let mut plugins = Vec::new();
    let mut errors = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir()) else { return (plugins, errors) };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.join("plugin.toml").is_file()).collect();
    dirs.sort();
    for d in dirs {
        let folder = d.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match std::fs::read_to_string(d.join("plugin.toml")).map_err(|e| e.to_string()).and_then(|t| toml::from_str::<Manifest>(&t).map_err(|e| e.to_string())) {
            Ok(m) if m.run.is_empty() => errors.push(format!("{folder}: `run` is empty")),
            Ok(m) => plugins.push(Plugin {
                name: m.name.unwrap_or(folder),
                description: m.description,
                dir: d,
                run: m.run,
                timeout: Duration::from_secs(m.timeout_secs.clamp(1, 3600)),
                actions: m.actions,
            }),
            Err(e) => errors.push(format!("{folder}/plugin.toml: {}", e.lines().next().unwrap_or(""))),
        }
    }
    (plugins, errors)
}

/// Fill {placeholders} in a field's default from the message.
pub fn expand(template: &str, vars: &[(&str, String)]) -> String {
    let mut out = template.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

/// What a running plugin reports back to the app.
#[derive(Debug)]
pub enum Event {
    Status(String),
    Done { text: String, url: Option<String> },
    Error(String),
    Then(Vec<String>),
    Open(String),
    /// The plugin exited (after its other events).
    Finished,
}

pub type Inbox = Arc<Mutex<VecDeque<(u64, Event)>>>;

/// Start a plugin action in the background. `payload` goes to its stdin; events land in
/// `inbox` tagged with `run_id`. `files` is a temporary folder to delete once it exits.
pub fn spawn(plugin: &Plugin, account: &str, email: &str, payload: Value, run_id: u64, inbox: Inbox, files: Option<PathBuf>) {
    let plugin = plugin.clone();
    let (account, email) = (account.to_string(), email.to_string());
    std::thread::spawn(move || {
        let push = |e: Event| inbox.lock().unwrap().push_back((run_id, e));
        let program = resolve(&plugin.dir, &plugin.run[0]);
        let mut child = match Command::new(&program)
            .args(&plugin.run[1..])
            .current_dir(&plugin.dir)
            .env("TERN_PLUGIN_DIR", &plugin.dir)
            .env("TERN_ACCOUNT", &account)
            .env("TERN_ACCOUNT_EMAIL", &email)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                push(Event::Error(format!("{}: can't run {}: {e}", plugin.name, program.display())));
                push(Event::Finished);
                return;
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(payload.to_string().as_bytes());
            let _ = stdin.write_all(b"\n");
        }
        // stderr: keep the last line, for when it fails without saying why.
        let last_err = Arc::new(Mutex::new(String::new()));
        if let Some(err) = child.stderr.take() {
            let last = last_err.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    if !line.trim().is_empty() {
                        *last.lock().unwrap() = line;
                    }
                }
            });
        }
        // Kill it if it overstays.
        let pid = child.id();
        let deadline = Instant::now() + plugin.timeout;
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let finished = finished.clone();
            let name = plugin.name.clone();
            let inbox = inbox.clone();
            std::thread::spawn(move || {
                while Instant::now() < deadline {
                    if finished.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                if !finished.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = Command::new("kill").arg(pid.to_string()).status();
                    inbox.lock().unwrap().push_back((run_id, Event::Error(format!("{name}: took too long, stopped"))));
                }
            });
        }
        let mut said_error = false;
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                for e in parse_line(&line) {
                    said_error |= matches!(e, Event::Error(_));
                    push(e);
                }
            }
        }
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        finished.store(true, std::sync::atomic::Ordering::Relaxed);
        if !ok && !said_error {
            let why = last_err.lock().unwrap().clone();
            push(Event::Error(format!("{} failed{}", plugin.name, if why.is_empty() { String::new() } else { format!(": {why}") })));
        }
        if let Some(d) = files {
            let _ = std::fs::remove_dir_all(d);
        }
        push(Event::Finished);
    });
}

fn resolve(dir: &Path, program: &str) -> PathBuf {
    if program.starts_with("./") || program.starts_with("../") {
        dir.join(program)
    } else {
        PathBuf::from(crate::theme::expand(program))
    }
}

fn parse_line(line: &str) -> Vec<Event> {
    let line = line.trim();
    if line.is_empty() {
        return vec![];
    }
    let Ok(Value::Object(m)) = serde_json::from_str::<Value>(line) else {
        return vec![Event::Status(line.chars().take(200).collect())];
    };
    let s = |k: &str| m.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let mut out = Vec::new();
    if let Some(t) = s("status") {
        out.push(Event::Status(t));
    }
    if let Some(t) = s("error") {
        out.push(Event::Error(t));
    }
    if let Some(t) = s("done") {
        out.push(Event::Done { text: t, url: s("url") });
    }
    if let Some(u) = s("open") {
        out.push(Event::Open(u));
    }
    if let Some(Value::Array(ops)) = m.get("then") {
        out.push(Event::Then(ops.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()));
    }
    out
}

/// One message as a plugin sees it.
pub fn message_json(b: &crate::model::Body, folder: &str, attachments: &[(String, PathBuf)]) -> Value {
    json!({
        "subject": b.subject,
        "from": b.from,
        "reply_to": b.reply_to,
        "to": b.to,
        "cc": b.cc,
        "date": b.date,
        "message_id": b.message_id,
        "folder": folder,
        "text": b.text,
        "has_html": b.html.is_some(),
        "attachments": attachments.iter().map(|(n, p)| json!({"name": n, "path": p})).collect::<Vec<_>>(),
    })
}
