mod accounts;
mod app;
mod auth;
mod calendar;
mod calview;
mod compose;
mod composer;
mod config;
mod contacts;
mod dav;
mod demo;
mod imap;
mod model;
mod oauth;
mod keys;
mod parse;
mod settings;
mod theme;
mod thread;
mod ui;
mod when;

use anyhow::{Context, Result};
use app::{AcctState, Action, App};
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::stdout;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const USAGE: &str = "tern — a dense, vim-driven mail client that wears your Omarchy theme

usage:
  tern                     open your mailboxes
  tern --demo              explore with a fake mailbox
  tern --config <path>     use another config file
  tern --init              write an example config to ~/.config/tern/config.toml
  tern query <text>        search contacts (mutt query format, for editor completion)
  tern contacts sync       pull contacts from every [[dav]] source now
  tern auth <account>      sign in with OAuth (Google, Microsoft) in the browser
";

const DEMO_CONFIG: &str = r#"
[[accounts]]
name = "Demo"
email = "ted@example.com"
display_name = "Ted Martin"
[accounts.imap]
host = "demo"
username = "demo"
password = "demo"
[accounts.smtp]
host = "demo"
username = "demo"
password = "demo"
"#;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut demo = false;
    let mut cfg_path = config::default_path();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("tern {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--demo" => demo = true,
            "--config" | "-c" => {
                i += 1;
                cfg_path = PathBuf::from(args.get(i).context("--config needs a path")?);
            }
            "--init" => return init(&cfg_path),
            "query" => {
                let q = args[i + 1..].join(" ");
                let all = contacts::load_cache();
                let hits = contacts::query(&all, &q);
                println!("tern: {} matches", hits.len());
                for (c, e) in hits {
                    println!("{}\t{}\t{}", c.emails[e], c.name, c.org);
                }
                return Ok(());
            }
            "auth" => {
                let cfg = config::load(&cfg_path)?;
                let name = args.get(i + 1).context("usage: tern auth <account name>")?;
                let acct = cfg
                    .accounts
                    .iter()
                    .find(|a| a.name.eq_ignore_ascii_case(name))
                    .with_context(|| format!("no account named {name}"))?;
                let o = acct.oauth.as_ref().with_context(|| format!("{name} has no [accounts.oauth] section"))?;
                let (name, o) = (acct.name.clone(), o.clone());
                return tokio::task::spawn_blocking(move || oauth::authorize(&name, &o)).await?;
            }
            "contacts" if args.get(i + 1).map(|s| s.as_str()) == Some("sync") => {
                let cfg = config::load(&cfg_path)?;
                let (c, errors) = dav::refresh_contacts(cfg).await?;
                println!("{} contacts synced", c.len());
                for e in errors {
                    eprintln!("warning: {e}");
                }
                return Ok(());
            }
            other => {
                eprint!("unknown argument `{other}`\n\n{USAGE}");
                std::process::exit(2);
            }
        }
        i += 1;
    }

    let cfg = if demo { toml::from_str(DEMO_CONFIG)? } else { config::load(&cfg_path)? };
    run(cfg, demo, cfg_path).await
}

fn init(path: &PathBuf) -> Result<()> {
    if path.exists() {
        println!("{} already exists — leaving it alone", path.display());
        return Ok(());
    }
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, config::EXAMPLE)?;
    println!("wrote {} — fill in your servers, then run `tern`", path.display());
    Ok(())
}

fn setup_terminal() -> Result<()> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, cursor::Hide, EnableMouseCapture)?;
    Ok(())
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(stdout(), DisableMouseCapture, LeaveAlternateScreen, cursor::Show);
}

/// Reads terminal input on its own thread. `paused` hands the terminal to $EDITOR without
/// this thread stealing its keystrokes.
fn spawn_input(tx: mpsc::UnboundedSender<CEvent>, paused: Arc<AtomicBool>) {
    std::thread::spawn(move || loop {
        if paused.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(30));
            continue;
        }
        if let Ok(true) = event::poll(Duration::from_millis(50)) {
            if paused.load(Ordering::SeqCst) {
                continue;
            }
            match event::read() {
                Ok(ev) => {
                    if tx.send(ev).is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });
}

async fn run(cfg: config::Config, demo: bool, cfg_path: PathBuf) -> Result<()> {
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    let mut accts = Vec::new();
    for (i, a) in cfg.accounts.iter().enumerate() {
        let (tx, rx) = mpsc::unbounded_channel();
        if demo {
            demo::spawn(i, rx, ev_tx.clone());
        } else {
            imap::spawn(i, a.clone(), rx, ev_tx.clone());
        }
        accts.push(AcctState { cfg: a.clone(), folders: vec![], tx, connected: false });
    }

    let mut app = App::new(cfg, accts, ev_tx.clone());
    app.demo = demo;
    app.cfg_path = (!demo).then_some(cfg_path);
    if demo {
        app.contacts = vec![
            contacts::Contact { name: "Maya Okafor".into(), emails: vec!["maya@northwind.io".into()], org: "Northwind".into() },
            contacts::Contact { name: "Jonas Berg".into(), emails: vec!["jonas@berg.dev".into()], org: String::new() },
            contacts::Contact { name: "Lena Fischer".into(), emails: vec!["lena@kleinwerk.de".into()], org: "Kleinwerk".into() },
        ];
    }
    app.start();

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        hook(info);
    }));
    setup_terminal()?;
    if !app.cfg.ui.mouse {
        let _ = execute!(stdout(), DisableMouseCapture);
    }
    let mut term = Terminal::new(CrosstermBackend::new(stdout()))?;
    term.clear()?;

    let (in_tx, mut in_rx) = mpsc::unbounded_channel();
    let paused = Arc::new(AtomicBool::new(false));
    spawn_input(in_tx, paused.clone());
    let mut tick = tokio::time::interval(Duration::from_millis(100));

    let result: Result<()> = async {
        loop {
            term.draw(|f| ui::draw(f, &mut app))?;
            tokio::select! {
                Some(ev) = in_rx.recv() => {
                    handle_input(&mut app, ev);
                    while let Ok(ev) = in_rx.try_recv() {
                        handle_input(&mut app, ev);
                    }
                }
                Some(ev) = ev_rx.recv() => {
                    app.on_event(ev);
                    while let Ok(ev) = ev_rx.try_recv() {
                        app.on_event(ev);
                    }
                }
                _ = tick.tick() => app.on_tick(),
            }
            for action in std::mem::take(&mut app.actions) {
                match action {
                    Action::Quit => return Ok(()),
                    Action::Mouse(on) => {
                        let _ = if on { execute!(stdout(), EnableMouseCapture) } else { execute!(stdout(), DisableMouseCapture) };
                    }
                    Action::Open(target) => {
                        let _ = std::process::Command::new("xdg-open")
                            .arg(&target)
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .spawn();
                    }
                    Action::Edit(draft) => {
                        let from = app.accts[app.current.as_ref().map(|c| c.0).unwrap_or(0)].cfg.sender_mailbox();
                        paused.store(true, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(80)).await;
                        restore_terminal();
                        let text = edit(&app, &draft, &from);
                        setup_terminal()?;
                        if !app.cfg.ui.mouse {
                            let _ = execute!(stdout(), DisableMouseCapture);
                        }
                        term.clear()?;
                        paused.store(false, Ordering::SeqCst);
                        match text {
                            Ok(t) => app.after_edit(draft, t),
                            Err(e) => app.after_edit_failed(format!("{e:#}")),
                        }
                    }
                }
            }
        }
    }
    .await;

    restore_terminal();
    result
}

fn handle_input(app: &mut App, ev: CEvent) {
    match ev {
        CEvent::Key(k) if k.kind == KeyEventKind::Press => app.on_key(k),
        CEvent::Mouse(m) => app.on_mouse(m),
        _ => {}
    }
}

/// Write the draft to a file, run the editor on it, and read it back.
fn edit(app: &App, d: &compose::Draft, from: &str) -> Result<Option<String>> {
    let dir = config::cache_dir().join("drafts");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("tern-{}.eml", uuid::Uuid::new_v4()));
    let text = d.to_file_text(from);
    std::fs::write(&path, &text)?;

    let editor = app
        .cfg
        .ui
        .editor
        .clone()
        .or_else(|| std::env::var("VISUAL").ok())
        .or_else(|| std::env::var("EDITOR").ok())
        .filter(|e| !e.trim().is_empty())
        .or_else(|| ["nvim", "vim", "vi", "nano"].iter().find(|e| on_path(e)).map(|e| e.to_string()))
        .context("no editor found — set $EDITOR or `editor` under [ui] in config.toml")?;
    // Land the cursor where typing starts: Subject for new mail, the body for replies.
    let header_lines = text.split("\n\n").next().unwrap_or("").lines().count();
    let line = if d.subject.is_empty() && !d.to.is_empty() {
        5
    } else if d.to.is_empty() {
        2
    } else {
        header_lines + 2
    };
    let jump = if editor.contains("vim") || editor.contains("vi") || editor.contains("nano") || editor.contains("kak") {
        format!(" +{line}")
    } else {
        String::new()
    };
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor}{jump} \"$1\"", ))
        .arg("sh")
        .arg(&path)
        .status()
        .with_context(|| format!("running editor `{editor}`"))?;
    let out = std::fs::read_to_string(&path)?;
    let _ = std::fs::remove_file(&path);
    if status.code() == Some(127) {
        anyhow::bail!("editor `{editor}` not found — set $EDITOR or `editor` under [ui] in config.toml");
    }
    // Some editors exit non-zero after a perfectly good save; trust the file. A failed exit
    // with nothing changed (vim's :cq) is an abort.
    if out.trim().is_empty() || (!status.success() && out == text) {
        return Ok(None);
    }
    Ok(Some(out))
}

fn on_path(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
        .unwrap_or(false)
}
