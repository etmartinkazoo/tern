//! Application state and the vim-style keymap.

use crate::compose::{self, Draft, Kind};
use crate::composer::{Composer, Outcome, VMode};
use crate::config::{Account, Config};
use crate::contacts::{self, Contact};
use crate::model::{Body, Cmd, Envelope, Event, Folder, Role};
use crate::theme::{Theme, Watcher};
use crate::accounts;
use crate::settings::{self, GeneralRow, Settings};
use crate::thread::{self, Row, RowKind, Thread};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Sidebar,
    List,
    Body,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptKind {
    Filter,
    CalFilter,
    PeopleFilter,
    QuickAdd,
    ConfirmDelete,
    Move,
    Attach,
}

#[derive(Debug)]
pub struct Prompt {
    pub kind: PromptKind,
    pub input: String,
    pub completions: Vec<String>,
    pub sel: Option<usize>,
}

pub enum Mode {
    Normal,
    Prompt(Prompt),
    /// The outbox panel: queued and failed messages, with the selected row.
    Outbox(usize),
    /// The settings view (with the full key list), over the whole window.
    Settings(Box<Settings>),
    /// Writing a message in the right-hand pane.
    Compose(Box<Composer>),
}

/// Things only the main loop can do (they need the terminal or the runtime).
pub enum Action {
    Quit,
    Edit(Draft),
    Open(String),
    Mouse(bool),
}

/// Where things were drawn last frame, for mouse clicks.
#[derive(Default, Clone)]
pub struct Hits {
    pub side: ratatui::layout::Rect,
    pub list: ratatui::layout::Rect,
    pub body: ratatui::layout::Rect,
    pub tabs: Vec<(ratatui::layout::Rect, usize)>,
    /// (screen row, General-tab row index)
    pub settings_rows: Vec<(u16, usize)>,
    pub settings_body: ratatui::layout::Rect,
    pub side_offset: usize,
    pub status: ratatui::layout::Rect,
}

pub struct AcctState {
    pub cfg: Account,
    pub folders: Vec<Folder>,
    pub tx: UnboundedSender<Cmd>,
    pub connected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DavRow {
    Source(usize),
    Calendar(usize),
    Add,
}

/// The account index of the unified inbox ("All inboxes").
pub const UNIFIED: usize = usize::MAX;

pub struct SideRow {
    pub acct: usize,
    pub folder: Option<usize>,
}

pub struct App {
    pub cfg: Config,
    pub theme: Theme,
    watcher: Watcher,
    pub accts: Vec<AcctState>,
    pub events_tx: UnboundedSender<Event>,

    pub focus: Focus,
    pub mode: Mode,
    pub zoom: bool,

    pub side_cursor: usize,
    /// The open folder: (account, IMAP name).
    pub current: Option<(usize, String)>,
    pub loading: bool,
    pub total: u32,

    /// Everything loaded for the open folder; `view` indexes into it after filtering.
    pub all: Vec<Envelope>,
    pub view: Vec<usize>,
    pub cursor: usize,
    pub list_offset: usize,
    pub list_height: usize,
    pub marked: HashSet<u32>,
    pub filter: String,

    pub body: Option<Body>,
    pub body_scroll: usize,
    pub body_height: usize,
    pub body_lines: usize,
    want_body: Option<(u32, Instant)>,
    /// The message on screen and since when, for marking it read after a short dwell.
    shown: Option<(u32, Instant)>,
    requested: Option<u32>,
    cache: HashMap<(usize, String, u32), Body>,
    cache_order: VecDeque<(usize, String, u32)>,

    pub contacts: Vec<Contact>,
    pub status: String,
    pub error: bool,
    status_at: Instant,
    pending: Option<char>,
    count: usize,
    last_poll: Instant,
    last_theme_check: Instant,
    pub actions: Vec<Action>,
    /// A draft set aside while the attach prompt is open.
    parked: Option<Box<Composer>>,
    /// UIDs the server matched for the current filter (body text included).
    server_hits: HashSet<u32>,
    /// Conversations in the open folder, and the list rows drawn from them.
    pub threads: Vec<Thread>,
    pub rows: Vec<Row>,
    pub threaded: bool,
    /// Per-conversation fold state, keyed by root UID; unlisted ones use `fold_default`.
    folds: HashMap<u32, bool>,
    fold_default: bool,
    /// Root UID of the conversation shown in the message pane, when a folded thread is
    /// selected, and which of its messages are shown open.
    pub conv: Option<u32>,
    pub conv_open: HashSet<u32>,
    /// Scroll the conversation to its first open message on the next draw.
    pub conv_jump: bool,
    pub screen: crate::calview::View,
    pub cal: crate::calview::CalState,
    pub people: crate::people::PeopleState,
    /// The settings view, set aside while an account form is open over it.
    pub parked_settings: Option<Box<Settings>>,
    /// Folders already loaded once (mail arriving after that is "new"), meetings
    /// already reminded of, and when reminders were last checked.
    loaded_once: HashSet<(usize, String)>,
    reminded: HashSet<(String, i64)>,
    last_reminder_check: Instant,
    /// Unified inbox bookkeeping: view ids ⇄ (account, uid), per-account totals.
    ids: HashMap<(usize, u32), u32>,
    rev: HashMap<u32, (usize, u32)>,
    next_id: u32,
    unified_totals: HashMap<usize, u32>,
    /// The last message sent, still undoable: (outbox id, its composer, when).
    undo: Option<(String, Box<Composer>, Instant)>,
    /// Outbox messages currently being sent, and how many are queued.
    sending: HashSet<String>,
    pub outbox_count: usize,
    last_outbox: Instant,
    last_autosave: Instant,
    /// Accounts folded shut in the sidebar, by name; remembered between runs.
    pub collapsed: HashSet<String>,
    /// Where settings are saved; None in demo mode.
    pub cfg_path: Option<PathBuf>,
    pub hits: Hits,
    /// Messages being shown as HTML instead of text, and the last HTML layout.
    pub html_view: HashSet<u32>,
    #[allow(clippy::type_complexity)]
    pub html_cache: Option<((u32, u16, ratatui::style::Color), Vec<ratatui::text::Line<'static>>)>,
    /// For telling a double click from two single ones.
    last_click: Option<(Instant, u16, u16)>,
    /// Demo mode never talks to an SMTP server.
    pub demo: bool,
}

fn ctrl(k: &KeyEvent) -> bool {
    k.modifiers.contains(KeyModifiers::CONTROL)
}

impl App {
    pub fn new(cfg: Config, accts: Vec<AcctState>, events_tx: UnboundedSender<Event>) -> Self {
        let theme_dir = cfg.ui.theme_dir.as_ref().map(|p| crate::theme::expand(&p.to_string_lossy()));
        App {
            theme: Theme::load(theme_dir.as_deref()),
            watcher: Watcher::new(theme_dir),
            cfg,
            accts,
            events_tx,
            focus: Focus::List,
            mode: Mode::Normal,
            zoom: false,
            side_cursor: 0,
            current: None,
            loading: true,
            total: 0,
            all: vec![],
            view: vec![],
            cursor: 0,
            list_offset: 0,
            list_height: 20,
            marked: HashSet::new(),
            filter: String::new(),
            body: None,
            body_scroll: 0,
            body_height: 20,
            body_lines: 0,
            want_body: None,
            shown: None,
            requested: None,
            cache: HashMap::new(),
            cache_order: VecDeque::new(),
            contacts: contacts::load_cache(),
            status: "connecting…".into(),
            error: false,
            status_at: Instant::now(),
            pending: None,
            count: 0,
            last_poll: Instant::now(),
            last_theme_check: Instant::now(),
            actions: vec![],
            parked: None,
            server_hits: HashSet::new(),
            demo: false,
            cfg_path: None,
            parked_settings: None,
            people: Default::default(),
            collapsed: load_ui_state(),
            undo: None,
            loaded_once: HashSet::new(),
            reminded: HashSet::new(),
            last_reminder_check: Instant::now(),
            ids: HashMap::new(),
            rev: HashMap::new(),
            next_id: 0,
            unified_totals: HashMap::new(),
            sending: HashSet::new(),
            outbox_count: crate::outbox::list().len(),
            last_outbox: Instant::now(),
            last_autosave: Instant::now(),
            screen: crate::calview::View::Mail,
            cal: crate::calview::CalState::new(),
            hits: Hits::default(),
            html_view: HashSet::new(),
            html_cache: None,
            last_click: None,
            threads: vec![],
            rows: vec![],
            threaded: true,
            folds: HashMap::new(),
            fold_default: true,
            conv: None,
            conv_open: HashSet::new(),
            conv_jump: false,
        }
    }

    pub fn start(&mut self) {
        if let Some(s) = crate::drafts::load() {
            let when = chrono::DateTime::from_timestamp(s.saved_at, 0)
                .map(|d| d.with_timezone(&chrono::Local).format("%a %H:%M").to_string())
                .unwrap_or_default();
            self.say(format!("unsent message “{}” from {when} · gr to pick it up", s.draft.subject));
        }
        self.apply_calendar_config();
        self.apply_timezone();
        self.threaded = self.cfg.ui.threads;
        self.fold_default = self.cfg.ui.collapse_threads;
        for a in &self.accts {
            let _ = a.tx.send(Cmd::Folders);
        }
        if self.accts.len() > 1 {
            self.open_folder(UNIFIED, "INBOX".into());
        } else if !self.accts.is_empty() {
            self.open_folder(0, "INBOX".into());
        }
        // Calendars sync in the background from the start, for reminders and the
        // mail view's agenda.
        if !self.cfg.dav_sources().is_empty() || self.demo {
            self.sync_calendars();
        }
        let stale = contacts::cache_age().is_none_or(|a| a > Duration::from_secs(3600));
        if stale {
            self.sync_contacts();
        }
    }

    pub fn refresh_contacts(&mut self) {
        self.sync_contacts();
    }

    /// A new message to `to`, from the account in view, with its signature.
    pub fn write_to(&mut self, to: &str) {
        let body = self.accts.get(self.acct_idx()).and_then(|a| a.cfg.signature.as_deref()).map(|s| format!("\n\n{s}")).unwrap_or_default();
        self.compose("New message", Draft { to: to.to_string(), body, ..Default::default() });
        if let Mode::Compose(c) = &mut self.mode {
            c.focus = crate::composer::SUBJECT;
        }
    }

    pub fn copy_text(&mut self, text: &str) {
        let ok = std::process::Command::new("wl-copy")
            .arg(text)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            self.say(format!("copied {text}"));
        } else {
            self.say_error("wl-copy not available");
        }
    }

    fn sync_contacts(&mut self) {
        if self.cfg.dav_sources().is_empty() {
            return;
        }
        let cfg = self.cfg.clone();
        let tx = self.events_tx.clone();
        tokio::spawn(async move {
            match crate::dav::refresh_contacts(cfg).await {
                Ok((c, errors)) => {
                    let _ = tx.send(Event::Contacts(c));
                    if !errors.is_empty() {
                        let _ = tx.send(Event::Error(usize::MAX, format!("contacts: {}", errors.join("; "))));
                    }
                }
                Err(e) => {
                    let _ = tx.send(Event::Error(usize::MAX, format!("contacts: {e:#}")));
                }
            }
        });
    }

    /// Prefix with the account name when there is more than one.
    fn tag(&self, acct: usize, s: String) -> String {
        match self.accts.get(acct) {
            Some(a) if self.accts.len() > 1 => format!("{}: {s}", a.cfg.name),
            _ => s,
        }
    }

    pub fn say(&mut self, s: impl Into<String>) {
        self.status = s.into();
        self.error = false;
        self.status_at = Instant::now();
    }

    pub fn say_error(&mut self, s: impl Into<String>) {
        self.fail(s)
    }

    fn fail(&mut self, s: impl Into<String>) {
        self.status = s.into();
        self.error = true;
        self.status_at = Instant::now();
    }

    // ---------------------------------------------------------------- selection helpers

    pub fn selected(&self) -> Option<&Envelope> {
        self.view.get(self.cursor).and_then(|&i| self.all.get(i))
    }


    /// The marked messages, or what is under the cursor: a folded conversation counts as
    /// all of its messages.
    fn targets(&self) -> Vec<u32> {
        if self.marked.is_empty() {
            self.row_uids(self.cursor)
        } else {
            self.all.iter().map(|e| e.uid).filter(|u| self.marked.contains(u)).collect()
        }
    }

    fn row_uids(&self, row: usize) -> Vec<u32> {
        match self.rows.get(row) {
            Some(r) if r.kind == RowKind::Collapsed => {
                self.threads[r.thread].members.iter().map(|&i| self.all[i].uid).collect()
            }
            Some(r) => vec![self.all[r.msg].uid],
            None => vec![],
        }
    }

    pub fn conv_thread(&self) -> Option<&Thread> {
        let root = self.conv?;
        self.threads.iter().find(|t| t.root == root)
    }

    pub fn cached(&self, uid: u32) -> Option<&Body> {
        let k = self.key(uid)?;
        self.cache.get(&k)
    }

    /// Whether the open list is the unified inbox (every account's INBOX).
    pub fn unified(&self) -> bool {
        self.current.as_ref().is_some_and(|c| c.0 == UNIFIED)
    }

    /// The id a message has in the list. In the unified inbox UIDs from different
    /// accounts can collide, so each (account, uid) gets its own; elsewhere it's the UID.
    fn view_id(&mut self, acct: usize, uid: u32) -> u32 {
        if !self.unified() {
            return uid;
        }
        if let Some(&v) = self.ids.get(&(acct, uid)) {
            return v;
        }
        self.next_id += 1;
        let v = self.next_id;
        self.ids.insert((acct, uid), v);
        self.rev.insert(v, (acct, uid));
        v
    }

    /// Where a listed message really lives: (account, folder, server uid).
    fn real(&self, view: u32) -> Option<(usize, String, u32)> {
        let (a, f) = self.current.clone()?;
        if a == UNIFIED {
            let (acct, uid) = *self.rev.get(&view)?;
            Some((acct, "INBOX".into(), uid))
        } else {
            Some((a, f, view))
        }
    }

    /// Group listed messages by the account and folder their commands go to.
    fn route(&self, uids: &[u32]) -> Vec<(usize, String, Vec<u32>)> {
        let mut out: Vec<(usize, String, Vec<u32>)> = Vec::new();
        for &v in uids {
            let Some((a, f, u)) = self.real(v) else { continue };
            match out.iter_mut().find(|(x, y, _)| *x == a && *y == f) {
                Some(g) => g.2.push(u),
                None => out.push((a, f, vec![u])),
            }
        }
        out
    }


    /// The row showing `uid`, or the folded conversation that contains it.
    fn row_of(&self, uid: u32) -> Option<usize> {
        self.rows.iter().position(|r| self.all[r.msg].uid == uid).or_else(|| {
            self.rows.iter().position(|r| {
                r.kind == RowKind::Collapsed && self.threads[r.thread].members.iter().any(|&i| self.all[i].uid == uid)
            })
        })
    }

    fn is_folded(&self, root: u32) -> bool {
        self.folds.get(&root).copied().unwrap_or(self.fold_default)
    }

    fn me(&self) -> String {
        self.accts.get(self.acct_idx()).map(|a| a.cfg.email.clone()).unwrap_or_default()
    }

    pub fn current_folder(&self) -> Option<&Folder> {
        let (a, name) = self.current.as_ref()?;
        self.accts.get(*a)?.folders.iter().find(|f| &f.name == name)
    }

    fn folder_for(&self, acct: usize, role: Role) -> Option<String> {
        self.accts[acct].folders.iter().find(|f| f.role == role).map(|f| f.name.clone())
    }

    pub fn side_rows(&self) -> Vec<SideRow> {
        let mut rows = Vec::new();
        let multi = self.accts.len() > 1;
        if multi {
            rows.push(SideRow { acct: UNIFIED, folder: None });
        }
        for (ai, a) in self.accts.iter().enumerate() {
            if multi {
                rows.push(SideRow { acct: ai, folder: None });
                if self.collapsed.contains(&a.cfg.name) {
                    continue;
                }
            }
            for fi in 0..a.folders.len() {
                rows.push(SideRow { acct: ai, folder: Some(fi) });
            }
        }
        rows
    }

    fn send(&self, acct: usize, cmd: Cmd) {
        let _ = self.accts[acct].tx.send(cmd);
    }

    // ---------------------------------------------------------------- folder + list

    fn open_folder(&mut self, acct: usize, name: String) {
        if self.current.as_ref() == Some(&(acct, name.clone())) && !self.all.is_empty() {
            return;
        }
        self.current = Some((acct, name.clone()));
        self.all.clear();
        self.view.clear();
        // Threads and rows index into `all`; they must go with it.
        self.threads.clear();
        self.rows.clear();
        self.conv = None;
        self.conv_open.clear();
        self.marked.clear();
        self.filter.clear();
        self.cursor = 0;
        self.list_offset = 0;
        self.body = None;
        self.requested = None;
        self.loading = true;
        self.ids.clear();
        self.rev.clear();
        if acct == UNIFIED {
            for a in 0..self.accts.len() {
                self.send(a, Cmd::Select { folder: "INBOX".into() });
            }
            self.side_cursor = 0;
            return;
        }
        self.send(acct, Cmd::Select { folder: name });
        // Keep the sidebar cursor on the folder we just opened.
        if let Some(i) = self.side_rows().iter().position(|r| {
            r.acct == acct
                && r.folder.and_then(|f| self.accts[acct].folders.get(f)).map(|f| &f.name)
                    == self.current.as_ref().map(|c| &c.1)
        }) {
            self.side_cursor = i;
        }
    }

    fn goto_role(&mut self, role: Role) {
        let acct = self.acct_idx();
        let name = if role == Role::Inbox {
            Some("INBOX".to_string())
        } else {
            self.folder_for(acct, role)
        };
        match name {
            Some(n) => {
                self.open_folder(acct, n);
                self.focus = Focus::List;
            }
            None => self.fail(format!("no {role:?} folder on this account")),
        }
    }

    fn refilter(&mut self) {
        let keep = self.selected().map(|e| e.uid);
        self.refilter_to(&keep.into_iter().collect::<Vec<_>>());
    }

    /// Rebuild the list and put the cursor on the first of `prefer` still in it. Callers
    /// that change `all` must capture this beforehand: afterwards the old rows point at
    /// the wrong messages.
    fn refilter_to(&mut self, prefer: &[u32]) {
        self.rebuild();
        self.cursor = prefer
            .iter()
            .find_map(|&u| self.row_of(u))
            .unwrap_or(0)
            .min(self.rows.len().saturating_sub(1));
        self.on_cursor_moved();
    }

    /// Where the cursor should go if the rows change: the highlighted message, then the
    /// rows below it, then the ones above (nearest first).
    fn cursor_candidates(&self) -> Vec<u32> {
        let uid = |r: usize| self.all[self.rows[r].msg].uid;
        let n = self.rows.len();
        let c = self.cursor.min(n.saturating_sub(1));
        (c..n).chain((0..c).rev()).map(uid).collect()
    }

    /// Filter, thread and lay out the list rows.
    fn rebuild(&mut self) {
        let q = self.filter.to_lowercase();
        let list: Vec<usize> = (0..self.all.len())
            .filter(|&i| {
                if q.is_empty() {
                    return true;
                }
                let e = &self.all[i];
                if self.server_hits.contains(&e.uid) {
                    return true;
                }
                e.subject.to_lowercase().contains(&q)
                    || e.from_name.to_lowercase().contains(&q)
                    || e.from_addr.to_lowercase().contains(&q)
                    || e.to.to_lowercase().contains(&q)
            })
            .collect();
        if self.threaded {
            self.threads = thread::build(&self.all, &list, &self.me());
            let folded: HashSet<u32> = self.threads.iter().map(|t| t.root).filter(|&r| self.is_folded(r)).collect();
            self.rows = thread::rows(&self.threads, |r| folded.contains(&r));
        } else {
            self.threads.clear();
            self.rows = list.iter().map(|&i| Row { msg: i, thread: usize::MAX, member: 0, kind: RowKind::Single }).collect();
        }
        self.view = self.rows.iter().map(|r| r.msg).collect();
    }

    fn set_fold(&mut self, fold: Option<bool>) {
        let Some(r) = self.rows.get(self.cursor) else { return };
        if r.thread == usize::MAX || r.kind == RowKind::Single {
            return;
        }
        let root = self.threads[r.thread].root;
        let now = self.is_folded(root);
        let want = fold.unwrap_or(!now);
        self.folds.insert(root, want);
        self.rebuild();
        // Land on the conversation's first row either way.
        let pos = self.rows.iter().position(|r| r.thread != usize::MAX && self.threads[r.thread].root == root).unwrap_or(0);
        self.cursor = pos;
        self.on_cursor_moved();
    }

    fn fold_all(&mut self, fold: bool) {
        let keep = self.selected().map(|e| e.uid);
        self.folds.clear();
        self.fold_default = fold;
        self.rebuild();
        self.cursor = keep.and_then(|u| self.row_of(u)).unwrap_or(0).min(self.rows.len().saturating_sub(1));
        self.on_cursor_moved();
    }

    fn move_cursor(&mut self, delta: isize) {
        if self.view.is_empty() {
            return;
        }
        let max = self.view.len() as isize - 1;
        self.cursor = (self.cursor as isize + delta).clamp(0, max) as usize;
        self.on_cursor_moved();
    }

    fn set_cursor(&mut self, i: usize) {
        self.cursor = i.min(self.view.len().saturating_sub(1));
        self.on_cursor_moved();
    }

    fn on_cursor_moved(&mut self) {
        // Keep a little context above/below the cursor, vim's scrolloff.
        let h = self.list_height.max(1);
        let off = 3.min(h / 3);
        if self.cursor < self.list_offset + off {
            self.list_offset = self.cursor.saturating_sub(off);
        } else if self.cursor + off >= self.list_offset + h {
            self.list_offset = (self.cursor + off + 1).saturating_sub(h);
        }
        let Some(uid) = self.selected().map(|e| e.uid) else {
            self.body = None;
            self.conv = None;
            return;
        };
        let conv = self.rows.get(self.cursor).filter(|r| r.kind == RowKind::Collapsed).map(|r| self.threads[r.thread].root);
        if conv != self.conv {
            self.conv = conv;
            self.body_scroll = 0;
            if let Some(t) = self.conv_thread() {
                // Open what you haven't read, plus the latest; fold the rest to one line.
                let open: HashSet<u32> =
                    t.chrono.iter().filter(|&&i| !self.all[i].seen || i == t.latest).map(|&i| self.all[i].uid).collect();
                let missing = t.chrono.iter().any(|&i| self.cached(self.all[i].uid).is_none());
                self.conv_open = open;
                self.conv_jump = true;
                if missing {
                    self.want_body = Some((uid, Instant::now()));
                }
            }
        }
        if self.body.as_ref().map(|b| b.uid) == Some(uid) {
            return;
        }
        self.body_scroll = 0;
        let key = self.key(uid);
        if let Some(b) = key.and_then(|k| self.cache.get(&k)).cloned() {
            self.body = Some(b);
            self.shown = Some((uid, Instant::now()));
        } else {
            self.want_body = Some((uid, Instant::now()));
        }
    }

    fn key(&self, uid: u32) -> Option<(usize, String, u32)> {
        self.real(uid)
    }

    fn mark_seen(&mut self) {
        if let Some(t) = self.conv_thread() {
            let uids: Vec<u32> = t.members.iter().map(|&i| self.all[i].uid).collect();
            return self.set_seen(uids, true);
        }
        let Some(uid) = self.selected().filter(|e| !e.seen).map(|e| e.uid) else { return };
        self.set_seen(vec![uid], true);
    }

    // ---------------------------------------------------------------- events

    pub fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Status(a, s) => {
                let s = self.tag(a, s);
                self.say(s)
            }
            Event::Error(a, s) => {
                self.loading = false;
                let s = self.tag(a, s);
                self.fail(s)
            }
            Event::Connected(a) => {
                self.accts[a].connected = true;
                self.say(format!("connected · {}", self.accts[a].cfg.name));
            }
            Event::Folders(a, folders) => {
                self.accts[a].folders = folders;
                let cur = self.current.clone().filter(|c| c.0 != UNIFIED);
                if let Some((ca, cf)) = cur {
                    if ca == a {
                        if let Some(i) = self.side_rows().iter().position(|r| {
                            r.acct == a && r.folder.map(|f| &self.accts[a].folders[f].name) == Some(&cf)
                        }) {
                            self.side_cursor = i;
                        }
                    }
                }
            }
            Event::Envelopes { acct, folder, items, total } => {
                let merge = self.unified() && folder == "INBOX";
                if self.current.as_ref() != Some(&(acct, folder.clone())) && !merge {
                    return;
                }
                let keep = self.cursor_candidates();
                let fresh = self.all.is_empty();
                let mut items = items;
                for e in items.iter_mut() {
                    e.acct = acct;
                    e.uid = self.view_id(acct, e.uid);
                }
                let before: HashSet<u32> = self.all.iter().map(|e| e.uid).collect();
                let new = items.iter().filter(|e| !e.seen && !before.contains(&e.uid)).count();
                // Notify only for mail that arrived after this folder was first loaded.
                let first_load = self.loaded_once.insert((acct, folder.clone()));
                if !first_load && self.cfg.ui.notifications && folder.eq_ignore_ascii_case("INBOX") {
                    let fresh_mail: Vec<&Envelope> = items.iter().filter(|e| !e.seen && !before.contains(&e.uid)).collect();
                    match fresh_mail.as_slice() {
                        [] => {}
                        [e] => crate::notify::send(&format!("{} · {}", e.sender(), self.accts[acct].cfg.name), &e.subject, "mail-unread"),
                        many => crate::notify::send(
                            &format!("{} new messages · {}", many.len(), self.accts[acct].cfg.name),
                            &many.iter().take(4).map(|e| format!("{}: {}", e.sender(), e.subject)).collect::<Vec<_>>().join("\n"),
                            "mail-unread",
                        ),
                    }
                }
                if merge {
                    // Replace this account's part of the unified list, newest first overall.
                    self.all.retain(|e| e.acct != acct);
                    self.all.extend(items);
                    self.all.sort_by(|a, b| b.date.cmp(&a.date).then(b.uid.cmp(&a.uid)));
                    self.unified_totals.insert(acct, total);
                    self.total = self.unified_totals.values().sum();
                } else {
                    self.all = items;
                    self.total = total;
                }
                self.loading = false;
                self.marked.retain(|u| self.all.iter().any(|e| e.uid == *u));
                self.refilter_to(&keep);
                if fresh {
                    self.say(format!("{} messages", total));
                } else if new > 0 {
                    self.say(format!("{new} new"));
                }
            }
            Event::Body { acct, folder, mut body } => {
                let key = (acct, folder, body.uid);
                // The list knows this message by its view id.
                if self.unified() {
                    match self.ids.get(&(acct, body.uid)) {
                        Some(&v) => body.uid = v,
                        None => body.uid = 0,
                    }
                }
                if self.cache_order.len() >= 256 {
                    if let Some(old) = self.cache_order.pop_front() {
                        self.cache.remove(&old);
                    }
                }
                self.cache_order.push_back(key.clone());
                let mut cached = (*body).clone();
                cached.uid = key.2;
                self.cache.insert(key.clone(), cached);
                let view_uid = body.uid;
                if self.selected().map(|e| e.uid) == Some(body.uid) && self.real(body.uid).as_ref() == Some(&key) {
                    self.shown = Some((body.uid, Instant::now()));
                    self.body = Some(*body);
                }
                if self.requested == Some(view_uid) {
                    self.requested = None;
                }
            }
            Event::Changed { acct, folder } => {
                if self.current.as_ref() == Some(&(acct, folder.clone())) || (self.unified() && folder == "INBOX") {
                    self.send(acct, Cmd::Poll { folder });
                    self.last_poll = Instant::now();
                } else {
                    self.send(acct, Cmd::Folders);
                }
            }
            Event::Removed { acct, folder, uids } => {
                if self.current.as_ref() == Some(&(acct, folder.clone())) {
                    self.remove_local(&uids);
                } else if self.unified() && folder == "INBOX" {
                    let views: Vec<u32> = uids.iter().filter_map(|u| self.ids.get(&(acct, *u)).copied()).collect();
                    self.remove_local(&views);
                }
            }
            Event::SearchResult { acct, folder, items, query } => {
                let merge = self.unified() && folder == "INBOX";
                if (self.current.as_ref() != Some(&(acct, folder)) && !merge) || self.filter != query {
                    return;
                }
                let keep = self.cursor_candidates();
                let mut items = items;
                for e in items.iter_mut() {
                    e.acct = acct;
                    e.uid = self.view_id(acct, e.uid);
                }
                let have: HashSet<u32> = self.all.iter().map(|e| e.uid).collect();
                let uids: Vec<u32> = items.iter().map(|e| e.uid).collect();
                let mut added = 0;
                for e in items {
                    if !have.contains(&e.uid) {
                        self.all.push(e);
                        added += 1;
                    }
                }
                self.all.sort_by(|a, b| b.date.cmp(&a.date).then(b.uid.cmp(&a.uid)));
                // The server matches body text too: show everything it found.
                self.server_hits = self.all.iter().map(|e| e.uid).filter(|u| !have.contains(u)).collect();
                self.server_hits.extend(uids);
                self.refilter_to(&keep);
                self.say(format!("server search: {} results ({added} older)", self.view.len()));
            }
            Event::Contacts(c) => {
                let n = c.len();
                self.contacts = c;
                self.say(format!("{n} contacts synced"));
            }
            Event::Calendars(c, errors) => self.on_calendars(c, errors),
            Event::CallLink(r, target) => self.on_call_link(r, target),
            Event::CalWrite(r) => {
                match r {
                    Ok(s) => self.say(s),
                    Err(e) => self.say_error(e),
                }
                // Either way, the server is the truth: resync.
                self.cal.syncing = false;
                self.sync_calendars();
            }
            Event::OutboxResult(id, r) => self.on_outbox_result(id, r),
            Event::Note(s) => self.say(s),
        }
    }

    fn remove_local(&mut self, uids: &[u32]) {
        let gone: HashSet<u32> = uids.iter().copied().collect();
        let candidates: Vec<u32> = self.cursor_candidates().into_iter().filter(|u| !gone.contains(u)).collect();
        // Called twice per move (optimistically, then on the server's confirmation), so
        // only count what is actually still here.
        let present = self.all.iter().filter(|e| gone.contains(&e.uid)).count() as u32;
        let unread: Vec<u32> = self.all.iter().filter(|e| gone.contains(&e.uid) && !e.seen).map(|e| e.uid).collect();
        for (a, f, u) in self.route(&unread) {
            if let Some(fo) = self.accts.get_mut(a).and_then(|s| s.folders.iter_mut().find(|x| x.name == f)) {
                fo.unread = fo.unread.saturating_sub(u.len() as u32);
            }
        }
        self.all.retain(|e| !gone.contains(&e.uid));
        self.marked.retain(|u| !gone.contains(u));
        self.total = self.total.saturating_sub(present);
        self.refilter_to(&candidates);
    }

    pub fn on_tick(&mut self) {
        self.autosave_draft();
        if self.last_outbox.elapsed() >= Duration::from_secs(1) {
            self.last_outbox = Instant::now();
            self.pump_outbox();
        }
        if let Some((uid, at)) = self.shown {
            let dwell = Duration::from_millis(self.cfg.ui.mark_read_ms);
            if self.selected().map(|e| e.uid) != Some(uid) {
                self.shown = None;
            } else if at.elapsed() >= dwell {
                self.shown = None;
                self.mark_seen();
            }
        }
        if let Some((uid, at)) = self.want_body {
            if at.elapsed() > Duration::from_millis(60) {
                self.want_body = None;
                if self.selected().map(|e| e.uid) == Some(uid) {
                    let missing: Option<Vec<u32>> = self.conv_thread().map(|t| {
                        t.chrono.iter().rev().take(40).map(|&i| self.all[i].uid).filter(|&u| self.cached(u).is_none()).collect()
                    });
                    match missing {
                        Some(uids) if !uids.is_empty() => {
                            self.requested = Some(uid);
                            for (a, f, real) in self.route(&uids) {
                                self.send(a, Cmd::Bodies { folder: f, uids: real });
                            }
                        }
                        Some(_) => {}
                        None if self.requested != Some(uid) => {
                            self.requested = Some(uid);
                            if let Some((a, f, real)) = self.real(uid) {
                                self.send(a, Cmd::Body { folder: f, uid: real });
                            }
                        }
                        None => {}
                    }
                }
            }
        }
        if self.last_theme_check.elapsed() > Duration::from_secs(2) {
            self.last_theme_check = Instant::now();
            if self.watcher.changed() {
                let dir = self.cfg.ui.theme_dir.as_ref().map(|p| crate::theme::expand(&p.to_string_lossy()));
                self.theme = Theme::load(dir.as_deref());
                self.say(format!("theme · {}", self.theme.name));
            }
        }
        let poll = self.accts.first().map(|a| a.cfg.imap.poll_secs).unwrap_or(60).max(15);
        if self.last_poll.elapsed() > Duration::from_secs(poll) {
            self.last_poll = Instant::now();
            self.poll();
        }
        if self.cal.synced_at.is_some_and(|t| t.elapsed() > Duration::from_secs(900)) {
            self.sync_calendars();
        }
        if self.last_reminder_check.elapsed() >= Duration::from_secs(20) {
            self.last_reminder_check = Instant::now();
            self.remind();
        }
        if !self.error && self.status_at.elapsed() > Duration::from_secs(6) && !self.status.is_empty() {
            self.status.clear();
        }
    }

    /// Notify for meetings about to start: at the event's own alarm, else
    /// `reminder_minutes` before. Each occurrence once.
    fn remind(&mut self) {
        if !self.cfg.ui.notifications || self.cal.calendars.is_empty() {
            return;
        }
        if self.cal.occ.is_empty() {
            self.cal.expand();
        }
        let now = crate::calendar::now();
        let default = self.cfg.ui.reminder_minutes as i64 * 60;
        let mut due = Vec::new();
        for o in &self.cal.occ {
            if o.all_day || o.start <= now {
                continue;
            }
            let lead = match o.alarm {
                Some(a) => a,
                None if default > 0 => default,
                None => continue,
            };
            let left = (o.start - now).num_seconds();
            let key = (o.uid.clone(), o.start.timestamp());
            if left <= lead && !self.reminded.contains(&key) {
                due.push((key, o.clone(), left));
            }
        }
        for (key, o, left) in due {
            self.reminded.insert(key);
            let mins = (left + 59) / 60;
            let mut body = format!("in {mins} min · {}", o.start.format("%H:%M"));
            if !o.location.is_empty() {
                body.push_str(&format!("\n{}", o.location));
            }
            crate::notify::send(&o.summary, &body, "x-office-calendar");
        }
    }

    fn poll(&mut self) {
        let unified = self.unified();
        if let Some((a, f)) = self.current.clone().filter(|_| !unified) {
            self.send(a, Cmd::Poll { folder: f });
        }
        for (i, a) in self.accts.iter().enumerate() {
            if unified {
                let _ = a.tx.send(Cmd::Poll { folder: "INBOX".into() });
            } else if self.current.as_ref().map(|c| c.0) != Some(i) {
                let _ = a.tx.send(Cmd::Folders);
            }
        }
    }

    // ---------------------------------------------------------------- keys

    pub fn on_key(&mut self, k: KeyEvent) {
        match &mut self.mode {
            Mode::Settings(_) => return self.settings_key(k),
            Mode::Outbox(_) => return self.outbox_key(k),
            Mode::Prompt(_) => return self.prompt_key(k),
            Mode::Compose(_) => return self.compose_key(k),
            Mode::Normal => {}
        }
        if ctrl(&k) && k.code == KeyCode::Char('z') {
            return self.undo_send();
        }
        if self.screen == crate::calview::View::People {
            if self.people_key(k) {
                return;
            }
            let global = matches!(k.code, KeyCode::Char('q') | KeyCode::Char(',') | KeyCode::Char('?'))
                || (ctrl(&k) && k.code == KeyCode::Char('c'));
            if !global {
                return;
            }
        }
        if self.screen == crate::calview::View::Calendar {
            if self.calendar_key(k) {
                return;
            }
            // Only the app-wide keys fall through to the mail keymap.
            let global = matches!(k.code, KeyCode::Char('q') | KeyCode::Char(',') | KeyCode::Char('?'))
                || (ctrl(&k) && k.code == KeyCode::Char('c'));
            if !global {
                return;
            }
        }

        if let KeyCode::Char(c) = k.code {
            if c.is_ascii_digit() && !ctrl(&k) && self.pending.is_none() && (c != '0' || self.count > 0) {
                self.count = self.count.saturating_mul(10).saturating_add(c as usize - '0' as usize).min(9999);
                return;
            }
        }
        let n = self.count.max(1);
        let had_count = self.count > 0;
        self.count = 0;

        if let Some(p) = self.pending.take() {
            if p == 'g' {
                match k.code {
                    KeyCode::Char('g') => self.top(),
                    KeyCode::Char('i') => self.goto_role(Role::Inbox),
                    KeyCode::Char('u') if self.accts.len() > 1 => {
                        self.open_folder(UNIFIED, "INBOX".into());
                        self.focus = Focus::List;
                    }
                    KeyCode::Char('s') => self.goto_role(Role::Sent),
                    KeyCode::Char('d') => self.goto_role(Role::Drafts),
                    KeyCode::Char('a') => self.goto_role(Role::Archive),
                    KeyCode::Char('t') => self.goto_role(Role::Trash),
                    KeyCode::Char('j') => self.goto_role(Role::Junk),
                    KeyCode::Char('f') => self.goto_role(Role::Flagged),
                    KeyCode::Char('c') => self.sync_contacts(),
                    KeyCode::Char('o') => self.mode = Mode::Outbox(0),
                    KeyCode::Char('r') => self.resume_draft(),
                    _ => {}
                }
            } else if p == 'z' {
                match k.code {
                    KeyCode::Char('z') => self.zoom = !self.zoom,
                    KeyCode::Char('a') if self.focus == Focus::Sidebar => {
                        if let Some(a) = self.side_rows().get(self.side_cursor).map(|r| r.acct) {
                            self.toggle_account(a);
                        }
                    }
                    KeyCode::Char('a') => self.set_fold(None),
                    KeyCode::Char('o') => self.set_fold(Some(false)),
                    KeyCode::Char('c') => self.set_fold(Some(true)),
                    KeyCode::Char('M') => self.fold_all(true),
                    KeyCode::Char('R') => self.fold_all(false),
                    _ => {}
                }
            }
            return;
        }

        self.error = false;
        let half = (self.pane_height() / 2).max(1) as isize;
        match k.code {
            KeyCode::Char('q') if !ctrl(&k) => self.actions.push(Action::Quit),
            KeyCode::Char('c') if ctrl(&k) => self.actions.push(Action::Quit),
            KeyCode::Esc => {
                if !self.filter.is_empty() {
                    self.filter.clear();
                    self.server_hits.clear();
                    self.refilter();
                } else if !self.marked.is_empty() {
                    self.marked.clear();
                } else if self.zoom {
                    self.zoom = false;
                }
                self.status.clear();
            }
            KeyCode::Char('?') => self.open_settings(settings::KEYS),
            KeyCode::Char(',') => self.open_settings(settings::GENERAL),
            KeyCode::Char('C') => self.toggle_view(),
            KeyCode::Char('P') => self.toggle_people(),
            KeyCode::Char('g') => self.pending = Some('g'),

            KeyCode::Char('j') | KeyCode::Down => self.down(n as isize),
            KeyCode::Char('k') | KeyCode::Up => self.down(-(n as isize)),
            KeyCode::Char('d') if ctrl(&k) => self.down(half),
            KeyCode::Char('u') if ctrl(&k) => self.down(-half),
            KeyCode::Char('f') if ctrl(&k) => self.down(half * 2),
            KeyCode::Char('b') if ctrl(&k) => self.down(-half * 2),
            KeyCode::PageDown => self.down(half * 2),
            KeyCode::PageUp => self.down(-half * 2),
            KeyCode::Char('G') | KeyCode::End => {
                if had_count {
                    self.goto_line(n - 1)
                } else {
                    self.bottom()
                }
            }
            KeyCode::Home => self.top(),
            KeyCode::Char('J') => self.move_cursor(n as isize),
            KeyCode::Char('K') => self.move_cursor(-(n as isize)),
            KeyCode::Char(' ') if self.focus == Focus::Sidebar => {
                if let Some(a) = self.side_rows().get(self.side_cursor).map(|r| r.acct) {
                    if self.accts.len() > 1 {
                        self.toggle_account(a);
                    }
                }
            }
            KeyCode::Char(' ') => self.page_or_next(),
            KeyCode::Char('h') | KeyCode::Left => self.left(),
            KeyCode::Char('l') | KeyCode::Right => self.right(),
            KeyCode::Enter => self.right(),
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::List,
                    Focus::List => Focus::Body,
                    Focus::Body => Focus::Sidebar,
                }
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Body,
                    Focus::List => Focus::Sidebar,
                    Focus::Body => Focus::List,
                }
            }
            KeyCode::Char('z') => self.pending = Some('z'),
            KeyCode::Char('/') => {
                self.mode = Mode::Prompt(Prompt {
                    kind: PromptKind::Filter,
                    input: self.filter.clone(),
                    completions: vec![],
                    sel: None,
                })
            }

            KeyCode::Char('x') => {
                let uids = self.row_uids(self.cursor);
                if !uids.is_empty() {
                    if uids.iter().all(|u| self.marked.contains(u)) {
                        uids.iter().for_each(|u| {
                            self.marked.remove(u);
                        });
                    } else {
                        self.marked.extend(uids);
                    }
                    self.move_cursor(1);
                }
            }
            KeyCode::Char('X') => {
                if self.marked.is_empty() {
                    self.marked = (0..self.rows.len()).flat_map(|r| self.row_uids(r)).collect();
                } else {
                    self.marked.clear();
                }
            }
            KeyCode::Char('T') => {
                self.threaded = !self.threaded;
                self.refilter();
                self.say(if self.threaded { "threads on" } else { "threads off" });
            }
            KeyCode::Char('s') => self.toggle_flag(),
            KeyCode::Char('u') => self.toggle_seen(),
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::Char('#') => self.trash(),
            KeyCode::Char('e') => self.archive(),
            KeyCode::Char('!') => self.move_to_role(Role::Junk),
            KeyCode::Char('M') => {
                let completions = self.folder_completions("");
                self.mode = Mode::Prompt(Prompt {
                    kind: PromptKind::Move,
                    input: String::new(),
                    sel: (!completions.is_empty()).then_some(0),
                    completions,
                })
            }
            KeyCode::Char('$') => {
                self.say("syncing…");
                self.poll();
                self.last_poll = Instant::now();
            }
            KeyCode::Char('r') if ctrl(&k) => {
                self.say("syncing…");
                self.poll();
            }

            KeyCode::Char('m') | KeyCode::Char('c') => {
                let body = self.accts[self.acct_idx()]
                    .cfg
                    .signature
                    .as_deref()
                    .map(|s| format!("\n\n{s}"))
                    .unwrap_or_default();
                self.compose("New message", Draft { body, ..Default::default() });
            }
            KeyCode::Char('r') => self.respond(Kind::Reply),
            KeyCode::Char('R') => self.respond(Kind::ReplyAll),
            KeyCode::Char('f') => self.respond(Kind::Forward),

            KeyCode::Char('o') => self.open_html(),
            KeyCode::Char('H') => self.toggle_html(),
            KeyCode::Char('a') => self.answer_invite("ACCEPTED"),
            KeyCode::Char('t') => self.answer_invite("TENTATIVE"),
            KeyCode::Char('n') => self.answer_invite("DECLINED"),
            KeyCode::Char('S') => self.save_attachments(),
            KeyCode::Char('U') => self.unsubscribe(),
            KeyCode::Char('A') => self.add_contact(),
            KeyCode::Char('y') => self.yank_sender(),
            _ => {}
        }
    }

    pub fn pane_height(&self) -> usize {
        match self.focus {
            Focus::Body => self.body_height,
            _ => self.list_height,
        }
    }

    fn scroll_body(&mut self, delta: isize) {
        let max = self.body_lines.saturating_sub(self.body_height.saturating_sub(2)) as isize;
        self.body_scroll = (self.body_scroll as isize + delta).clamp(0, max.max(0)) as usize;
    }

    fn down(&mut self, n: isize) {
        match self.focus {
            Focus::Sidebar => {
                let rows = self.side_rows().len() as isize;
                if rows > 0 {
                    self.side_cursor = (self.side_cursor as isize + n).clamp(0, rows - 1) as usize;
                }
            }
            Focus::List => self.move_cursor(n),
            Focus::Body => self.scroll_body(n),
        }
    }

    fn top(&mut self) {
        match self.focus {
            Focus::Sidebar => self.side_cursor = 0,
            Focus::List => self.set_cursor(0),
            Focus::Body => self.body_scroll = 0,
        }
    }

    fn bottom(&mut self) {
        match self.focus {
            Focus::Sidebar => self.side_cursor = self.side_rows().len().saturating_sub(1),
            Focus::List => self.set_cursor(self.view.len().saturating_sub(1)),
            Focus::Body => self.scroll_body(isize::MAX / 2),
        }
    }

    fn goto_line(&mut self, i: usize) {
        match self.focus {
            Focus::List => self.set_cursor(i),
            Focus::Body => self.body_scroll = i.min(self.body_lines),
            Focus::Sidebar => self.side_cursor = i.min(self.side_rows().len().saturating_sub(1)),
        }
    }

    /// Space reads on: page the body, and at its end move to the next message.
    fn page_or_next(&mut self) {
        let page = self.body_height.saturating_sub(3).max(1);
        let max = self.body_lines.saturating_sub(self.body_height.saturating_sub(2));
        if self.body_scroll >= max {
            self.move_cursor(1);
        } else {
            self.scroll_body(page as isize);
        }
    }

    /// Fold or unfold an account's folders in the sidebar (the accordion). The cursor
    /// lands on the account's heading so it stays put on screen.
    fn toggle_account(&mut self, acct: usize) {
        if acct == UNIFIED {
            return self.open_folder(UNIFIED, "INBOX".into());
        }
        let name = self.accts[acct].cfg.name.clone();
        if !self.collapsed.remove(&name) {
            self.collapsed.insert(name);
        }
        if let Some(i) = self.side_rows().iter().position(|r| r.acct == acct && r.folder.is_none()) {
            self.side_cursor = i;
        }
        save_ui_state(&self.collapsed);
    }

    fn left(&mut self) {
        // h on a folder folds its account, like closing a directory in a file tree.
        if self.focus == Focus::Sidebar && self.accts.len() > 1 {
            if let Some(r) = self.side_rows().get(self.side_cursor) {
                if r.folder.is_some() {
                    let a = r.acct;
                    return self.toggle_account(a);
                }
            }
        }
        self.focus = match self.focus {
            Focus::Body if self.zoom => {
                self.zoom = false;
                Focus::List
            }
            Focus::Body => Focus::List,
            _ => Focus::Sidebar,
        };
    }

    fn right(&mut self) {
        match self.focus {
            Focus::Sidebar => {
                let rows = self.side_rows();
                if let Some(r) = rows.get(self.side_cursor) {
                    if r.folder.is_none() {
                        let a = r.acct;
                        return self.toggle_account(a);
                    }
                    if let Some(fi) = r.folder {
                        let f = &self.accts[r.acct].folders[fi];
                        if f.selectable {
                            let (a, n) = (r.acct, f.name.clone());
                            self.open_folder(a, n);
                            self.focus = Focus::List;
                        }
                    }
                }
            }
            Focus::List => {
                if self.selected().is_some() && !self.edit_server_draft() {
                    self.focus = Focus::Body
                }
            }
            Focus::Body => {}
        }
    }

    // ---------------------------------------------------------------- message actions

    fn toggle_flag(&mut self) {
        // On a folded conversation, flag its latest message, as Gmail does.
        let uids = if self.marked.is_empty() { self.selected().map(|e| vec![e.uid]).unwrap_or_default() } else { self.targets() };
        let Some(first) = self.all.iter().find(|e| uids.contains(&e.uid)) else { return };
        let on = !first.flagged;
        for e in self.all.iter_mut().filter(|e| uids.contains(&e.uid)) {
            e.flagged = on;
        }
        for t in self.threads.iter_mut() {
            t.flagged = t.members.iter().any(|&i| self.all[i].flagged);
        }
        for (a, f, real) in self.route(&uids) {
            self.send(a, Cmd::SetFlag { folder: f, uids: real, flag: "\\Flagged", on });
        }
    }

    fn toggle_seen(&mut self) {
        let uids = self.targets();
        if uids.is_empty() {
            return;
        }
        // Anything unread among them (a conversation, say) means "mark read".
        let on = !self.all.iter().filter(|e| uids.contains(&e.uid)).all(|e| e.seen);
        self.set_seen(uids, on);
        // Stop the open message from marking itself read again.
        self.shown = None;
    }

    fn set_seen(&mut self, uids: Vec<u32>, on: bool) {
        let mut changed = Vec::new();
        for e in self.all.iter_mut().filter(|e| uids.contains(&e.uid) && e.seen != on) {
            e.seen = on;
            changed.push(e.uid);
        }
        if changed.is_empty() {
            return;
        }
        for t in self.threads.iter_mut() {
            t.unread = t.members.iter().filter(|&&i| !self.all[i].seen).count();
        }
        for (a, f, real) in self.route(&changed) {
            let delta = if on { -(real.len() as i64) } else { real.len() as i64 };
            if let Some(fo) = self.accts.get_mut(a).and_then(|s| s.folders.iter_mut().find(|x| x.name == f)) {
                fo.unread = (fo.unread as i64 + delta).max(0) as u32;
            }
            self.send(a, Cmd::SetFlag { folder: f, uids: real, flag: "\\Seen", on });
        }
    }

    /// Move the targets; `dest` names the folder per account (a role like Archive is a
    /// different folder on each).
    fn move_each(&mut self, dest: impl Fn(&App, usize) -> Option<String>, verb: &str) {
        let uids = self.targets();
        if uids.is_empty() {
            return;
        }
        let groups = self.route(&uids);
        let mut moved = 0;
        let mut missing = Vec::new();
        for (a, f, real) in &groups {
            match dest(self, *a) {
                Some(to) if &to != f => {
                    moved += real.len();
                    self.send(*a, Cmd::Move { folder: f.clone(), uids: real.clone(), to });
                }
                Some(_) => {}
                None => missing.push(self.accts[*a].cfg.name.clone()),
            }
        }
        if !missing.is_empty() {
            return self.fail(format!("no folder to {verb} to on {} — set it under [accounts.folders]", missing.join(", ")));
        }
        // Optimistic: drop from the list now; the server confirms with Removed.
        self.remove_local(&uids);
        self.say(format!("{verb} {moved}"));
    }

    fn move_to(&mut self, to: String, verb: &str) {
        if self.unified() && self.route(&self.targets()).len() > 1 {
            return self.fail("those are in different accounts — move them from each account's inbox");
        }
        self.move_each(move |_, _| Some(to.clone()), verb);
    }

    fn move_to_role(&mut self, role: Role) {
        let verb = format!("{role:?}").to_lowercase();
        self.move_each(move |app, a| app.folder_for(a, role), &verb);
    }

    fn trash(&mut self) {
        let in_trash = self.current_folder().map(|x| x.role == Role::Trash).unwrap_or(false);
        let uids = self.targets();
        if uids.is_empty() {
            return;
        }
        // Accounts with a Trash folder move there; inside Trash (or without one) it's for good.
        let groups = self.route(&uids);
        let mut purged = 0;
        for (a, f, real) in &groups {
            if in_trash || self.folder_for(*a, Role::Trash).is_none() {
                purged += real.len();
                self.send(*a, Cmd::Expunge { folder: f.clone(), uids: real.clone() });
            } else if let Some(to) = self.folder_for(*a, Role::Trash) {
                self.send(*a, Cmd::Move { folder: f.clone(), uids: real.clone(), to });
            }
        }
        self.remove_local(&uids);
        if purged > 0 {
            self.say(format!("deleted {purged} permanently"));
        } else {
            self.say(format!("trash {}", uids.len()));
        }
    }

    fn archive(&mut self) {
        self.move_each(|app, a| app.folder_for(a, Role::Archive).or_else(|| app.folder_for(a, Role::All)), "archive");
    }

    /// For drawing: the account a message on screen belongs to.
    pub fn open_acct(&self) -> usize {
        self.acct_idx()
    }

    /// The account the composer on screen sends from.
    pub fn compose_acct(&self) -> usize {
        match &self.mode {
            Mode::Compose(c) => c.from_acct.unwrap_or_else(|| self.acct_idx()),
            _ => self.acct_idx(),
        }
    }

    /// The open account (in the unified inbox: the selected message's, else the first).
    fn acct_idx(&self) -> usize {
        match self.current.as_ref() {
            Some((a, _)) if *a == UNIFIED => self.selected().and_then(|e| self.rev.get(&e.uid)).map(|x| x.0).unwrap_or(0),
            Some((a, _)) => *a,
            None => 0,
        }
    }

    fn respond(&mut self, kind: Kind) {
        let Some(b) = self.body.as_ref() else {
            self.fail("message still loading");
            return;
        };
        // Reply from the account (and as the message) it actually came to.
        let Some((a, folder, real_uid)) = self.real(b.uid) else { return };
        let mut b = b.clone();
        b.uid = real_uid;
        let d = compose::respond(kind, &b, &self.accts[a].cfg, a, &folder);
        let title = match kind {
            Kind::Reply => "Reply",
            Kind::ReplyAll => "Reply all",
            Kind::Forward => "Forward",
        };
        self.compose(title, d);
    }

    /// a / t / n on a meeting invitation: reply to the organizer, and put the event in
    /// (or take it out of) your calendar.
    fn answer_invite(&mut self, partstat: &'static str) {
        let Some(b) = self.body.clone() else { return };
        let Some(ics) = b.invite.clone() else {
            return self.say("not a meeting invitation");
        };
        let Some(inv) = crate::calendar::invite(&ics) else {
            return self.say_error("couldn't read the invitation");
        };
        if inv.method != "REQUEST" {
            return self.say(format!("this is a {} — nothing to answer", inv.method.to_lowercase()));
        }
        let a = self.acct_idx();
        let acct = self.accts[a].cfg.clone();
        let me = acct.email.clone();
        let my_name = acct.display_name.clone().unwrap_or_default();
        let (word, verb) = match partstat {
            "ACCEPTED" => ("Accepted", "accepted"),
            "TENTATIVE" => ("Tentative", "tentatively accepted"),
            _ => ("Declined", "declined"),
        };
        let when = inv.start.format("%a %e %b %Y, %H:%M").to_string();
        let who = if my_name.is_empty() { me.clone() } else { my_name.clone() };
        let text = format!("{who} has {verb} this invitation:\n\n{}\n{when}\n", inv.summary);
        let reply = match crate::calendar::reply_ics(&ics, &me, &my_name, partstat) {
            Ok(r) => r,
            Err(e) => return self.say_error(format!("{e:#}")),
        };
        let organizer = if inv.organizer_name.is_empty() {
            inv.organizer.clone()
        } else {
            format!("\"{}\" <{}>", inv.organizer_name.replace('"', ""), inv.organizer)
        };
        let msg = match crate::compose::invite_reply(&acct, &organizer, &format!("{word}: {}", inv.summary), &text, reply) {
            Ok(m) => m,
            Err(e) => return self.say_error(format!("{e:#}")),
        };

        // The calendar side: an existing copy (same UID) is updated, or removed on decline.
        let existing = self.cal.calendars.iter().enumerate().find_map(|(ci, c)| {
            c.items.iter().find(|(_, v)| v.contains(&format!("UID:{}", inv.uid))).map(|(id, _)| (ci, id.clone()))
        });
        let cal_job = match (partstat, existing) {
            ("DECLINED", Some((ci, id))) => {
                let c = &self.cal.calendars[ci];
                Some((c.source.clone(), c.target(), id.clone(), None, c.etags.get(&id).cloned()))
            }
            ("DECLINED", None) => None,
            (_, found) => {
                let copy = match crate::calendar::calendar_copy(&ics, &me, &my_name, partstat) {
                    Ok(c) => c,
                    Err(e) => return self.say_error(format!("{e:#}")),
                };
                let (ci, id) = match found {
                    Some(x) => x,
                    None => match self.cal.target() {
                        Some(ci) => {
                            let safe: String = inv.uid.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
                            (ci, format!("{safe}.ics"))
                        }
                        None => {
                            self.say("no calendar to file it in — add a [[dav]] source");
                            (usize::MAX, String::new())
                        }
                    },
                };
                (ci != usize::MAX).then(|| {
                    let c = &self.cal.calendars[ci];
                    (c.source.clone(), c.target(), id.clone(), Some(copy), c.etags.get(&id).cloned())
                })
            }
        };
        if let Some(e) = self.selected().map(|e| e.uid) {
            for (acc, f, real) in self.route(&[e]) {
                self.send(acc, Cmd::SetFlag { folder: f, uids: real, flag: "\\Answered", on: true });
            }
        }
        if self.demo {
            return self.say(format!("demo: would reply “{word}” to {} and update the calendar", inv.organizer));
        }
        self.say(format!("{word} · replying to {}…", inv.organizer));
        let (cfg, tx) = (self.cfg.clone(), self.events_tx.clone());
        tokio::spawn(async move {
            if let Err(e) = crate::compose::send(&acct, msg).await {
                let _ = tx.send(Event::Error(usize::MAX, format!("reply failed: {e:#}")));
                return;
            }
            let r = match cal_job {
                Some((src, cal, id, Some(ics), etag)) => {
                    crate::dav::put_event(cfg, src, cal, id, ics, etag).await.map(|_| format!("{word} · replied and added to your calendar"))
                }
                Some((src, cal, id, None, etag)) => {
                    crate::dav::delete_event(cfg, src, cal, id, etag).await.map(|_| format!("{word} · replied and removed from your calendar"))
                }
                None => Ok(format!("{word} · replied")),
            };
            let _ = tx.send(Event::CalWrite(r.map_err(|e| format!("replied, but the calendar wasn't updated: {e:#}"))));
        });
    }

    fn toggle_html(&mut self) {
        let Some(b) = &self.body else { return };
        if b.html.is_none() {
            self.say("this message has no HTML part");
            return;
        }
        let uid = b.uid;
        if !self.html_view.remove(&uid) {
            self.html_view.insert(uid);
        }
        self.body_scroll = 0;
    }

    fn open_html(&mut self) {
        let Some(b) = &self.body else { return };
        let dir = crate::config::cache_dir().join("view");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("{}.html", b.uid));
        let html = b.html.clone().unwrap_or_else(|| {
            format!("<pre style=\"white-space:pre-wrap;font:14px monospace\">{}</pre>", html_escape(&b.text))
        });
        match std::fs::write(&path, html) {
            Ok(()) => self.actions.push(Action::Open(path.to_string_lossy().into_owned())),
            Err(e) => self.fail(format!("{e}")),
        }
    }

    fn save_attachments(&mut self) {
        let Some(b) = &self.body else { return };
        if b.attachments.is_empty() {
            self.say("no attachments");
            return;
        }
        let dir = self.cfg.ui.download_dir.clone();
        let _ = std::fs::create_dir_all(&dir);
        let mut saved = Vec::new();
        for i in 0..b.attachments.len() {
            if let Some((name, bytes)) = crate::parse::attachment_bytes(&b.raw, i) {
                let path = unique_path(&dir, &sanitize(&name));
                if std::fs::write(&path, bytes).is_ok() {
                    saved.push(path.file_name().unwrap().to_string_lossy().into_owned());
                }
            }
        }
        self.say(format!("saved to {}: {}", dir.display(), saved.join(", ")));
    }

    fn unsubscribe(&mut self) {
        let Some(u) = self.body.as_ref().and_then(|b| b.list_unsubscribe.clone()) else {
            self.say("no List-Unsubscribe header");
            return;
        };
        if let Some(addr) = u.strip_prefix("mailto:") {
            let (to, q) = addr.split_once('?').unwrap_or((addr, ""));
            let subject = q
                .split('&')
                .find_map(|kv| kv.strip_prefix("subject="))
                .unwrap_or("unsubscribe")
                .replace("%20", " ");
            self.compose("Unsubscribe", Draft { to: to.into(), subject, body: "unsubscribe\n".into(), ..Default::default() });
        } else {
            self.actions.push(Action::Open(u));
            self.say("opened unsubscribe link");
        }
    }

    fn add_contact(&mut self) {
        let Some(e) = self.selected() else { return };
        let (name, email) = (e.from_name.clone(), e.from_addr.clone());
        if self.cfg.dav_sources().is_empty() {
            self.fail("no [[dav]] source configured");
            return;
        }
        if self.contacts.iter().any(|c| c.emails.iter().any(|x| x.eq_ignore_ascii_case(&email))) {
            self.say(format!("{email} is already a contact"));
            return;
        }
        let (cfg, tx) = (self.cfg.clone(), self.events_tx.clone());
        self.say(format!("adding {email}…"));
        tokio::spawn(async move {
            let ev = match crate::dav::add_contact_to_first(cfg.clone(), name, email).await {
                Ok(()) => match crate::dav::refresh_contacts(cfg).await {
                    Ok((c, _)) => Event::Contacts(c),
                    Err(e) => Event::Error(usize::MAX, format!("contacts: {e:#}")),
                },
                Err(e) => Event::Error(usize::MAX, format!("add contact: {e:#}")),
            };
            let _ = tx.send(ev);
        });
    }

    fn yank_sender(&mut self) {
        let Some(e) = self.selected() else { return };
        let addr = e.from_addr.clone();
        let ok = std::process::Command::new("wl-copy")
            .arg(&addr)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            self.say(format!("yanked {addr}"));
        } else {
            self.fail("wl-copy not available");
        }
    }

    // ---------------------------------------------------------------- prompts

    fn contact_completions(&self, input: &str) -> Vec<String> {
        let last = input.rsplit(',').next().unwrap_or("").trim();
        if last.is_empty() {
            return vec![];
        }
        contacts::query(&self.contacts, last).into_iter().take(8).map(|(c, i)| c.mailbox(i)).collect()
    }

    fn folder_completions(&self, input: &str) -> Vec<String> {
        let a = self.acct_idx();
        let q = input.to_lowercase();
        let cur = self.current.as_ref().map(|c| c.1.as_str());
        let mut v: Vec<(bool, String)> = self.accts[a]
            .folders
            .iter()
            .filter(|f| f.selectable && Some(f.name.as_str()) != cur)
            .filter(|f| q.is_empty() || f.name.to_lowercase().contains(&q) || f.label.to_lowercase().contains(&q))
            .map(|f| (!f.label.to_lowercase().starts_with(&q), f.name.clone()))
            .collect();
        v.sort();
        v.into_iter().map(|(_, n)| n).collect()
    }

    fn path_completions(input: &str) -> Vec<String> {
        let expanded = crate::theme::expand(if input.is_empty() { "./" } else { input });
        let s = expanded.to_string_lossy().into_owned();
        let (dir, prefix) = match s.rfind('/') {
            Some(i) => (PathBuf::from(&s[..=i]), s[i + 1..].to_string()),
            None => (PathBuf::from("."), s.clone()),
        };
        let Ok(rd) = std::fs::read_dir(&dir) else { return vec![] };
        let mut out: Vec<String> = rd
            .flatten()
            .filter_map(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                if !n.starts_with(&prefix) || (n.starts_with('.') && !prefix.starts_with('.')) {
                    return None;
                }
                let full = dir.join(&n);
                let mut p = full.to_string_lossy().into_owned();
                if full.is_dir() {
                    p.push('/');
                }
                Some(p)
            })
            .collect();
        out.sort();
        out.truncate(8);
        out
    }

    fn refresh_completions(&mut self) {
        let Mode::Prompt(p) = &self.mode else { return };
        let (kind, input) = (p.kind, p.input.clone());
        let c = match kind {
            PromptKind::Move => self.folder_completions(&input),
            PromptKind::Attach => Self::path_completions(&input),
            PromptKind::Filter | PromptKind::CalFilter | PromptKind::PeopleFilter | PromptKind::QuickAdd | PromptKind::ConfirmDelete => vec![],
        };
        if let Mode::Prompt(p) = &mut self.mode {
            p.sel = match kind {
                PromptKind::Move => (!c.is_empty()).then_some(0),
                _ => None,
            };
            p.completions = c;
        }
    }

    fn prompt_key(&mut self, k: KeyEvent) {
        let Mode::Prompt(p) = &mut self.mode else { return };
        let kind = p.kind;
        if kind == PromptKind::ConfirmDelete {
            self.mode = Mode::Normal;
            if matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                self.delete_event();
            } else {
                self.say("kept");
            }
            return;
        }
        if k.code == KeyCode::Esc {
            let prev = std::mem::replace(&mut self.mode, Mode::Normal);
            if kind == PromptKind::Filter {
                self.filter.clear();
                self.refilter();
            }
            if kind == PromptKind::CalFilter {
                self.cal_filter("");
            }
            drop(prev);
            if kind == PromptKind::Attach {
                // Back to the composer we came from.
                if let Some(c) = self.parked.take() {
                    self.mode = Mode::Compose(c);
                }
            }
            return;
        }
        let Mode::Prompt(p) = &mut self.mode else { return };
        let n = p.completions.len();
        match k.code {
            KeyCode::Char('n') if ctrl(&k) => {
                if n > 0 {
                    p.sel = Some(p.sel.map_or(0, |s| (s + 1) % n));
                }
            }
            KeyCode::Down => {
                if n > 0 {
                    p.sel = Some(p.sel.map_or(0, |s| (s + 1) % n));
                }
            }
            KeyCode::Char('p') if ctrl(&k) => {
                if n > 0 {
                    p.sel = Some(p.sel.map_or(n - 1, |s| (s + n - 1) % n));
                }
            }
            KeyCode::Up => {
                if n > 0 {
                    p.sel = Some(p.sel.map_or(n - 1, |s| (s + n - 1) % n));
                }
            }
            KeyCode::Tab => {
                if n == 0 {
                    return;
                }
                let pick = p.completions[p.sel.unwrap_or(0)].clone();
                p.input = pick;
                self.refresh_completions();
            }
            KeyCode::Char('w') if ctrl(&k) => {
                let t = p.input.trim_end().to_string();
                let cut = t.rfind([' ', ',', '/']).map(|i| i + 1).unwrap_or(0);
                p.input.truncate(cut);
                self.after_prompt_edit(kind);
            }
            KeyCode::Char('u') if ctrl(&k) => {
                p.input.clear();
                self.after_prompt_edit(kind);
            }
            KeyCode::Backspace => {
                p.input.pop();
                self.after_prompt_edit(kind);
            }
            KeyCode::Char(c) if !ctrl(&k) => {
                p.input.push(c);
                self.after_prompt_edit(kind);
            }
            KeyCode::Enter => self.submit_prompt(),
            _ => {}
        }
    }

    fn after_prompt_edit(&mut self, kind: PromptKind) {
        if kind == PromptKind::PeopleFilter {
            if let Mode::Prompt(p) = &self.mode {
                self.people.filter = p.input.clone();
                self.people.cursor = 0;
                self.people.offset = 0;
            }
            return;
        }
        if kind == PromptKind::CalFilter {
            let q = match &self.mode {
                Mode::Prompt(p) => p.input.clone(),
                _ => String::new(),
            };
            return self.cal_filter(&q);
        }
        if kind == PromptKind::Filter {
            if let Mode::Prompt(p) = &self.mode {
                self.filter = p.input.clone();
            }
            self.server_hits.clear();
            self.refilter();
        } else {
            self.refresh_completions();
        }
    }

    fn submit_prompt(&mut self) {
        let Mode::Prompt(p) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
        let picked = p.sel.and_then(|s| p.completions.get(s).cloned());
        match p.kind {
            PromptKind::Filter => {
                self.filter = p.input.trim().to_string();
                self.refilter();
                if !self.filter.is_empty() {
                    if let Some((a, f)) = self.current.clone() {
                        self.say(format!("searching server for “{}”…", self.filter));
                        if a == UNIFIED {
                            for i in 0..self.accts.len() {
                                self.send(i, Cmd::Search { folder: "INBOX".into(), query: self.filter.clone() });
                            }
                        } else {
                            self.send(a, Cmd::Search { folder: f, query: self.filter.clone() });
                        }
                    }
                    self.focus = Focus::List;
                }
            }
            PromptKind::CalFilter => self.cal_filter(&p.input),
            PromptKind::PeopleFilter => self.people.filter = p.input.trim().to_string(),
            PromptKind::QuickAdd => self.quick_add(&p.input),
            PromptKind::ConfirmDelete => {}
            PromptKind::Move => {
                let to = picked.unwrap_or_else(|| p.input.trim().to_string());
                if !to.is_empty() {
                    self.move_to(to, "moved");
                }
            }
            PromptKind::Attach => {
                let Some(mut c) = self.parked.take() else { return };
                let path = crate::theme::expand(picked.as_deref().unwrap_or(p.input.trim()));
                if path.is_file() {
                    self.say(format!("attached {}", path.display()));
                    c.draft.attach_paths.push(path);
                } else {
                    self.fail(format!("not a file: {}", path.display()));
                }
                self.mode = Mode::Compose(c);
            }
        }
    }

    // ---------------------------------------------------------------- mouse

    pub fn on_mouse(&mut self, m: MouseEvent) {
        let at = Position { x: m.column, y: m.row };
        let inside = |r: Rect| r.contains(at);
        match m.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let d: isize = if m.kind == MouseEventKind::ScrollDown { 3 } else { -3 };
                self.wheel(at, d);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let double = self
                    .last_click
                    .is_some_and(|(t, x, y)| t.elapsed() < Duration::from_millis(400) && x == m.column && y == m.row);
                self.last_click = Some((Instant::now(), m.column, m.row));
                // The "? help" corner of the status bar.
                if inside(self.hits.status) && m.column + 8 >= self.hits.status.x + self.hits.status.width {
                    return self.open_settings(settings::KEYS);
                }
                match &mut self.mode {
                    Mode::Settings(_) => self.settings_click(at, double),
                    Mode::Normal if self.screen == crate::calview::View::Calendar => self.cal_click(at, double),
                    Mode::Normal => self.click(at, double),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn wheel(&mut self, at: Position, d: isize) {
        if self.screen == crate::calview::View::Calendar && matches!(self.mode, Mode::Normal) {
            return self.cal_wheel(at, d);
        }
        if let Mode::Settings(st) = &mut self.mode {
            if st.tab == settings::GENERAL {
                let k = if d > 0 { KeyCode::Char('j') } else { KeyCode::Char('k') };
                self.settings_key(KeyEvent::new(k, KeyModifiers::NONE));
            } else {
                st.scroll = (st.scroll as isize + d).max(0) as usize;
            }
            return;
        }
        if let Mode::Compose(c) = &mut self.mode {
            let k = if d > 0 { KeyCode::Down } else { KeyCode::Up };
            if c.on_body() && c.mode != VMode::Insert {
                for _ in 0..3 {
                    c.key(KeyEvent::new(k, KeyModifiers::NONE));
                }
            }
            return;
        }
        if self.hits.list.contains(at) {
            // Scroll the list itself; the selection stays put.
            let max = self.rows.len().saturating_sub(self.list_height) as isize;
            self.list_offset = (self.list_offset as isize + d).clamp(0, max.max(0)) as usize;
        } else if self.hits.body.contains(at) {
            self.scroll_body(d);
        } else if self.hits.side.contains(at) {
            let n = self.side_rows().len() as isize;
            self.side_cursor = (self.side_cursor as isize + d.signum()).clamp(0, (n - 1).max(0)) as usize;
        }
    }

    fn click(&mut self, at: Position, double: bool) {
        let h = self.hits.clone();
        if h.side.contains(at) && at.y > h.side.y {
            let row = (at.y - h.side.y - 1) as usize + h.side_offset;
            if row < self.side_rows().len() {
                self.side_cursor = row;
                self.focus = Focus::Sidebar;
                self.right();
            }
        } else if h.list.contains(at) && at.y > h.list.y {
            let row = (at.y - h.list.y - 1) as usize + self.list_offset;
            if row < self.rows.len() {
                self.focus = Focus::List;
                if row != self.cursor {
                    self.set_cursor(row);
                }
                if double {
                    // Double click reads it: a folded thread opens as a conversation.
                    self.focus = Focus::Body;
                }
            }
        } else if h.body.contains(at) && self.selected().is_some() {
            self.focus = Focus::Body;
        }
    }

    fn settings_click(&mut self, at: Position, double: bool) {
        let h = self.hits.clone();
        let Mode::Settings(st) = &mut self.mode else { return };
        if let Some((_, tab)) = h.tabs.iter().find(|(r, _)| r.contains(at)) {
            st.tab = *tab;
            st.scroll = 0;
            st.editing = None;
            return;
        }
        if st.tab != settings::GENERAL {
            return;
        }
        let Some(&(_, row)) = h.settings_rows.iter().find(|(y, _)| *y == at.y) else { return };
        let same = st.cursor == row;
        st.cursor = row;
        st.editing = None;
        let is_bool = matches!(
            settings::general_rows().get(row),
            Some(GeneralRow::Item(i)) if matches!(settings::ITEMS[*i].kind, settings::Kind::Bool)
        );
        // Switches flip on one click; values open for editing on a second.
        if is_bool || same || double {
            self.activate_setting();
        }
    }

    // ---------------------------------------------------------------- config edits

    /// Rewrite config.toml through `f` (which keeps comments), then reload it. In demo
    /// mode there's no file; nothing is written.
    pub fn edit_config(&mut self, f: impl FnOnce(&str) -> anyhow::Result<String>) -> bool {
        let Some(path) = self.cfg_path.clone() else { return true };
        let r = std::fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|t| f(&t))
            .and_then(|t| accounts::save(&path, &t))
            .and_then(|_| crate::config::load(&path));
        match r {
            Ok(cfg) => {
                self.cfg = cfg;
                self.apply_calendar_config();
                true
            }
            Err(e) => {
                self.say_error(format!("{e:#}"));
                false
            }
        }
    }

    /// Set the calendar's zone from config; an unreadable name keeps the default.
    pub fn apply_timezone(&mut self) {
        match crate::calendar::parse_zone(&self.cfg.ui.calendar_timezone) {
            Some(z) => crate::calendar::set_zone(z),
            None => self.say_error(format!("unknown calendar_timezone `{}` — using America/New_York", self.cfg.ui.calendar_timezone)),
        }
    }

    /// Hidden calendars and the default, from config.
    pub fn apply_calendar_config(&mut self) {
        self.cal.hidden = self.cfg.dav.iter().flat_map(|d| d.hidden.iter().map(|h| (d.name.clone(), h.clone()))).collect();
        self.cal.default = self.cfg.ui.default_calendar.clone();
    }

    // ---------------------------------------------------------------- calendar sources

    /// Rows of the Calendars tab: each source, then its calendars; the last is "add".
    pub fn dav_rows(&self) -> Vec<DavRow> {
        let mut rows = Vec::new();
        for (i, d) in self.cfg.dav.iter().enumerate() {
            rows.push(DavRow::Source(i));
            for (ci, c) in self.cal.calendars.iter().enumerate() {
                if c.source == d.name {
                    rows.push(DavRow::Calendar(ci));
                }
            }
        }
        rows.push(DavRow::Add);
        rows
    }

    fn calendars_key(&mut self, k: KeyEvent) -> bool {
        let rows = self.dav_rows();
        let Mode::Settings(st) = &mut self.mode else { return false };
        if let Some(i) = st.dav_confirm.take() {
            if matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                let name = self.cfg.dav[i].name.clone();
                if self.edit_config(|t| accounts::remove_dav(t, i)) {
                    self.say(format!("removed {name}"));
                    self.cal.syncing = false;
                    self.sync_calendars();
                    self.sync_contacts();
                }
            } else {
                self.say("kept");
            }
            return true;
        }
        if let Some(p) = st.dav_picking {
            match k.code {
                KeyCode::Char('j') | KeyCode::Down => st.dav_picking = Some((p + 1).min(accounts::DAV_PRESETS.len() - 1)),
                KeyCode::Char('k') | KeyCode::Up => st.dav_picking = Some(p.saturating_sub(1)),
                KeyCode::Enter | KeyCode::Char('l') => {
                    st.dav_picking = None;
                    self.open_source_form(None, p);
                }
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('h') => st.dav_picking = None,
                _ => {}
            }
            return true;
        }
        let cur = st.dav_cursor.min(rows.len().saturating_sub(1));
        match (k.code, rows.get(cur).copied()) {
            (KeyCode::Char('j') | KeyCode::Down, _) => st.dav_cursor = (cur + 1).min(rows.len() - 1),
            (KeyCode::Char('k') | KeyCode::Up, _) => st.dav_cursor = cur.saturating_sub(1),
            (KeyCode::Char('a'), _) | (KeyCode::Enter, Some(DavRow::Add)) => st.dav_picking = Some(0),
            (KeyCode::Enter | KeyCode::Char('e'), Some(DavRow::Source(i))) => self.open_source_form(Some(i), 0),
            (KeyCode::Char('d'), Some(DavRow::Source(i))) => st.dav_confirm = Some(i),
            (KeyCode::Char(' ') | KeyCode::Enter, Some(DavRow::Calendar(ci))) => self.toggle_calendar_index(ci),
            (KeyCode::Char('*'), Some(DavRow::Calendar(ci))) => self.set_default_calendar(ci),
            (KeyCode::Char('$'), _) => {
                self.say("syncing calendars and contacts…");
                self.cal.syncing = false;
                self.sync_calendars();
                self.sync_contacts();
            }
            _ => return false,
        }
        true
    }

    fn open_source_form(&mut self, index: Option<usize>, preset: usize) {
        if self.cfg_path.is_none() {
            return self.say("demo: sources can't be changed");
        }
        let (spec, title) = match index {
            Some(i) => {
                let s = accounts::DavSpec::from_config(&self.cfg, i);
                let t = format!("Edit {}", s.name);
                (s, t)
            }
            None => {
                let (label, url) = accounts::DAV_PRESETS[preset];
                // Most people add a server their mail account already signs in to.
                let acct = self.cfg.accounts.first();
                let spec = accounts::DavSpec {
                    url: url.to_string(),
                    username: acct.map(|a| a.email.clone()).unwrap_or_default(),
                    account: acct.map(|a| a.name.clone()).unwrap_or_default(),
                    ..Default::default()
                };
                (spec, format!("New source · {label}"))
            }
        };
        let note = "Calendars and address books on the server are found automatically. Leave Password cmd empty to sign in as the account named below.";
        let form = crate::composer::SourceForm { index, old_name: index.map(|i| self.cfg.dav[i].name.clone()) };
        if let Mode::Settings(st) = std::mem::replace(&mut self.mode, Mode::Normal) {
            self.parked_settings = Some(st);
        }
        self.mode = Mode::Compose(Box::new(Composer::new_source(&title, &spec, form, note)));
    }

    fn save_source(&mut self, c: Box<Composer>) {
        let Some(form) = c.source.clone() else { return };
        let spec = accounts::DavSpec::from_fields(&c.values());
        if let Err(e) = spec.validate(&self.cfg, form.index) {
            self.say_error(format!("{e:#}"));
            self.mode = Mode::Compose(c);
            return;
        }
        if !self.edit_config(|t| accounts::write_dav(t, form.index, form.old_name.as_deref(), &spec)) {
            self.mode = Mode::Compose(c);
            return;
        }
        self.restore_settings();
        self.say(format!("saved {} · syncing…", spec.name));
        self.cal.syncing = false;
        self.sync_calendars();
        self.sync_contacts();
    }

    // ---------------------------------------------------------------- accounts

    /// Keys on the Accounts tab. Returns false for ones the general settings keys handle.
    fn accounts_key(&mut self, k: KeyEvent) -> bool {
        let n = self.cfg.accounts.len();
        let Mode::Settings(st) = &mut self.mode else { return false };
        if let Some(i) = st.confirm.take() {
            if matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                self.delete_account(i);
            } else {
                self.say("kept");
            }
            return true;
        }
        if let Some(p) = st.picking {
            match k.code {
                KeyCode::Char('j') | KeyCode::Down => st.picking = Some((p + 1).min(accounts::PRESETS.len() - 1)),
                KeyCode::Char('k') | KeyCode::Up => st.picking = Some(p.saturating_sub(1)),
                KeyCode::Enter | KeyCode::Char('l') => {
                    st.picking = None;
                    self.open_account_form(None, p);
                }
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('h') => st.picking = None,
                _ => {}
            }
            return true;
        }
        match k.code {
            KeyCode::Char('j') | KeyCode::Down => st.acct_cursor = (st.acct_cursor + 1).min(n),
            KeyCode::Char('k') | KeyCode::Up => st.acct_cursor = st.acct_cursor.saturating_sub(1),
            KeyCode::Char('a') => st.picking = Some(0),
            KeyCode::Enter | KeyCode::Char('e') => {
                if st.acct_cursor >= n {
                    st.picking = Some(0);
                } else {
                    let i = st.acct_cursor;
                    self.open_account_form(Some(i), 0);
                }
            }
            KeyCode::Char('d') if st.acct_cursor < n => st.confirm = Some(st.acct_cursor),
            _ => return false,
        }
        true
    }

    fn open_account_form(&mut self, index: Option<usize>, preset: usize) {
        if self.cfg_path.is_none() {
            return self.say("demo: accounts can't be changed");
        }
        let (spec, title, note) = match index {
            Some(i) => {
                let s = accounts::Spec::from_config(&self.cfg, i);
                let title = format!("Edit {}", s.name);
                (s, title, "Changes reconnect the account as soon as you save.".to_string())
            }
            None => {
                let p = &accounts::PRESETS[preset];
                (accounts::Spec::from_preset(p), format!("New {} account", p.label), p.note.to_string())
            }
        };
        let form = crate::composer::AccountForm {
            index,
            old_name: index.map(|i| self.cfg.accounts[i].name.clone()),
            oauth: spec.oauth.clone(),
            save_sent: spec.save_sent,
        };
        if let Mode::Settings(st) = std::mem::replace(&mut self.mode, Mode::Normal) {
            self.parked_settings = Some(st);
        }
        self.mode = Mode::Compose(Box::new(Composer::new_account(&title, &spec, form, &note)));
    }

    fn restore_settings(&mut self) {
        self.mode = match self.parked_settings.take() {
            Some(st) => Mode::Settings(st),
            None => Mode::Normal,
        };
    }

    fn save_account(&mut self, c: Box<Composer>) {
        let Some(form) = c.account.clone() else { return };
        let spec = accounts::Spec::from_fields(&c.values(), c.body_text(), form.oauth.clone(), form.save_sent);
        let fail = |app: &mut App, c: Box<Composer>, e: String| {
            app.say_error(e);
            app.mode = Mode::Compose(c);
        };
        if let Err(e) = spec.validate(&self.cfg, form.index) {
            return fail(self, c, format!("{e:#}"));
        }
        let Some(path) = self.cfg_path.clone() else { return };
        let result = std::fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|text| accounts::write(&text, form.index, form.old_name.as_deref(), &spec))
            .and_then(|text| accounts::save(&path, &text).map(|_| ()))
            .and_then(|_| crate::config::load(&path));
        let cfg = match result {
            Ok(c) => c,
            Err(e) => return fail(self, c, format!("{e:#}")),
        };
        // Keep the UI settings in effect; take the new accounts and sources.
        let ui = self.cfg.ui.clone();
        self.cfg = Config { ui, ..cfg };
        let i = form.index.unwrap_or(self.cfg.accounts.len() - 1);
        self.connect_account(i);
        self.restore_settings();
        if let Mode::Settings(st) = &mut self.mode {
            st.acct_cursor = i;
        }
        let mut msg = format!("saved {}", spec.name);
        if !spec.caldav.is_empty() || !spec.carddav.is_empty() {
            msg.push_str(" · calendar and contacts added");
            self.sync_contacts();
            self.cal.syncing = false;
            self.sync_calendars();
        }
        if !spec.oauth.is_empty() {
            msg.push_str(&format!(" · now run `tern auth {}` to sign in", spec.name));
        }
        self.say(msg);
    }

    /// Start (or restart) account `i`'s connection from the current config.
    fn connect_account(&mut self, i: usize) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let acct = self.cfg.accounts[i].clone();
        if !self.demo {
            crate::imap::spawn(i, acct.clone(), rx, self.events_tx.clone());
        }
        if i < self.accts.len() {
            // The old worker ends when its channel is dropped here.
            self.accts[i].cfg = acct;
            self.accts[i].tx = tx;
            self.accts[i].connected = false;
        } else {
            self.accts.push(AcctState { cfg: acct, folders: vec![], tx, connected: false });
        }
        self.send(i, Cmd::Folders);
        match self.current.clone() {
            Some((a, f)) if a == i => self.send(i, Cmd::Select { folder: f }),
            None => self.open_folder(i, "INBOX".into()),
            _ => {}
        }
    }

    fn delete_account(&mut self, i: usize) {
        let Some(path) = self.cfg_path.clone() else { return };
        let name = self.cfg.accounts[i].name.clone();
        let r = std::fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|t| accounts::remove(&t, i, &name))
            .and_then(|t| accounts::save(&path, &t));
        match r {
            Ok(()) => {
                if let Ok(cfg) = crate::config::load(&path) {
                    let ui = self.cfg.ui.clone();
                    self.cfg = Config { ui, ..cfg };
                }
                self.say(format!("removed {name} from config.toml · restart tern to close its connection"));
            }
            Err(e) => self.say_error(format!("{e:#}")),
        }
    }

    // ---------------------------------------------------------------- settings

    pub fn open_settings(&mut self, tab: usize) {
        self.mode = Mode::Settings(Box::new(Settings { tab, cursor: 1, ..Default::default() }));
    }

    fn settings_key(&mut self, k: KeyEvent) {
        if matches!(&self.mode, Mode::Settings(st) if st.tab == settings::ACCOUNTS && st.editing.is_none()) && self.accounts_key(k) {
            return;
        }
        if matches!(&self.mode, Mode::Settings(st) if st.tab == settings::CALENDARS) && self.calendars_key(k) {
            return;
        }
        let Mode::Settings(st) = &mut self.mode else { return };
        let rows = settings::general_rows();
        if let Some(buf) = st.editing.as_mut() {
            match k.code {
                KeyCode::Esc => st.editing = None,
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char('u') if ctrl(&k) => buf.clear(),
                KeyCode::Char(c) if !ctrl(&k) => buf.push(c),
                KeyCode::Enter => {
                    let value = st.editing.take().unwrap_or_default();
                    if let Some(GeneralRow::Item(i)) = rows.get(st.cursor) {
                        let i = *i;
                        self.apply_setting(i, &value);
                    }
                }
                _ => {}
            }
            return;
        }
        if st.typing {
            match k.code {
                KeyCode::Esc => {
                    st.typing = false;
                    st.filter.clear();
                }
                KeyCode::Enter => st.typing = false,
                KeyCode::Backspace => {
                    st.filter.pop();
                }
                KeyCode::Char(c) if !ctrl(&k) => st.filter.push(c),
                _ => {}
            }
            st.scroll = 0;
            return;
        }
        let g = std::mem::take(&mut st.pending_g);
        let is_item = |c: usize| matches!(rows.get(c), Some(GeneralRow::Item(_)));
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char(',') => {
                if !st.filter.is_empty() && k.code == KeyCode::Esc {
                    st.filter.clear();
                } else {
                    self.mode = Mode::Normal;
                }
            }
            KeyCode::Char('?') => self.mode = Mode::Normal,
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Tab => {
                st.tab = (st.tab + 1) % settings::TABS.len();
                st.scroll = 0;
            }
            KeyCode::Char('h') | KeyCode::Left | KeyCode::BackTab => {
                st.tab = (st.tab + settings::TABS.len() - 1) % settings::TABS.len();
                st.scroll = 0;
            }
            KeyCode::Char(c @ '1'..='5') => {
                st.tab = c as usize - '1' as usize;
                st.scroll = 0;
            }
            KeyCode::Char('/') if st.tab == settings::KEYS => {
                st.typing = true;
                st.filter.clear();
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if st.tab == settings::GENERAL {
                    if let Some(n) = (st.cursor + 1..rows.len()).find(|&c| is_item(c)) {
                        st.cursor = n;
                    }
                } else {
                    st.scroll += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if st.tab == settings::GENERAL {
                    if let Some(n) = (0..st.cursor).rev().find(|&c| is_item(c)) {
                        st.cursor = n;
                    }
                } else {
                    st.scroll = st.scroll.saturating_sub(1);
                }
            }
            KeyCode::Char('d') if ctrl(&k) => st.scroll += 10,
            KeyCode::Char('u') if ctrl(&k) => st.scroll = st.scroll.saturating_sub(10),
            KeyCode::Char('g') if g => {
                st.scroll = 0;
                st.cursor = 1;
            }
            KeyCode::Char('g') => st.pending_g = true,
            KeyCode::Char('G') => {
                st.scroll = usize::MAX / 2;
                st.cursor = rows.len() - 1;
            }
            KeyCode::Enter | KeyCode::Char(' ') if st.tab == settings::GENERAL => self.activate_setting(),
            KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Char('-') if st.tab == settings::GENERAL => {
                let up = k.code != KeyCode::Char('-');
                if let Some(GeneralRow::Item(i)) = rows.get(st.cursor) {
                    let i = *i;
                    if let settings::Kind::Num { step, min, max, .. } = settings::ITEMS[i].kind {
                        let cur: i64 = settings::get(&self.cfg.ui, settings::ITEMS[i].key).parse().unwrap_or(0);
                        let next = (cur + if up { step } else { -step }).clamp(min, max);
                        self.apply_setting(i, &next.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    /// Enter / space / click on a General row: toggle a switch or start editing a value.
    pub fn activate_setting(&mut self) {
        let Mode::Settings(st) = &mut self.mode else { return };
        let Some(GeneralRow::Item(i)) = settings::general_rows().into_iter().nth(st.cursor) else { return };
        let item = &settings::ITEMS[i];
        let cur = settings::get(&self.cfg.ui, item.key);
        match item.kind {
            settings::Kind::Bool => {
                let next = if cur == "true" { "false" } else { "true" };
                self.apply_setting(i, next);
            }
            _ => st.editing = Some(cur),
        }
    }

    fn apply_setting(&mut self, i: usize, raw: &str) {
        let item = &settings::ITEMS[i];
        let value = match settings::set(&mut self.cfg.ui, item, raw) {
            Ok(v) => v,
            Err(e) => return self.fail(format!("{e:#}")),
        };
        match item.key {
            "threads" => {
                self.threaded = self.cfg.ui.threads;
                self.refilter();
            }
            "collapse_threads" => self.fold_all(self.cfg.ui.collapse_threads),
            "calendar_timezone" => {
                self.apply_timezone();
                self.cal.today_at_top();
            }
            "mouse" => self.actions.push(Action::Mouse(self.cfg.ui.mouse)),
            _ => {}
        }
        let shown = settings::get(&self.cfg.ui, item.key);
        match &self.cfg_path {
            Some(p) => match settings::persist(p, item.key, value) {
                Ok(()) => self.say(format!("{} → {} · saved", item.label, if shown.is_empty() { "default" } else { &shown })),
                Err(e) => self.fail(format!("couldn't save: {e:#}")),
            },
            None => self.say(format!("{} → {} (demo: not saved)", item.label, shown)),
        }
    }

    // ---------------------------------------------------------------- composing

    /// The composer on screen: the active one, or the one parked behind the attach prompt.
    pub fn composer_mut(&mut self) -> Option<&mut Composer> {
        match &mut self.mode {
            Mode::Compose(c) => Some(c),
            _ => self.parked.as_deref_mut(),
        }
    }

    fn compose(&mut self, title: &str, d: Draft) {
        // A reply starts from the account the message came to; anything else from
        // the account you're looking at.
        let from = d.source.as_ref().map(|s| s.0).unwrap_or_else(|| self.acct_idx());
        let mut c = Composer::new(title, d);
        c.from_acct = Some(from);
        self.mode = Mode::Compose(Box::new(c));
    }

    /// ^f in the composer: the next account sends it, with its signature.
    fn cycle_from(&mut self) {
        let n = self.accts.len();
        if n < 2 {
            return self.say("only one account to send from");
        }
        let sigs: Vec<Option<String>> = self.accts.iter().map(|a| a.cfg.signature.clone()).collect();
        let names: Vec<String> = self.accts.iter().map(|a| a.cfg.sender_mailbox()).collect();
        let cur = self.compose_acct();
        let Mode::Compose(c) = &mut self.mode else { return };
        let next = (cur + 1) % n;
        c.swap_signature(sigs[cur].as_deref(), sigs[next].as_deref());
        c.from_acct = Some(next);
        self.say(format!("from {}", names[next]));
    }

    /// Back from the external editor: fold its text into the composer that sent us there.
    pub fn after_edit(&mut self, d: Draft, text: Option<String>) {
        let mut c = self.parked.take();
        match text {
            Some(t) => {
                let mut d = c.as_ref().map(|c| c.draft.clone()).unwrap_or(d);
                d.update_from_text(&t);
                let title = c.as_ref().map(|c| c.title.clone()).unwrap_or_else(|| "Message".into());
                let mut n = Composer::new(&title, d);
                n.focus = n.fields.len();
                n.mode = VMode::Normal;
                c = Some(Box::new(n));
            }
            None => self.say("editor quit without changes"),
        }
        if let Some(c) = c {
            self.mode = Mode::Compose(c);
        }
    }

    pub fn after_edit_failed(&mut self, e: String) {
        if let Some(c) = self.parked.take() {
            self.mode = Mode::Compose(c);
        }
        self.fail(e);
    }

    fn compose_key(&mut self, k: KeyEvent) {
        let Mode::Compose(c) = &mut self.mode else { return };
        let out = c.key(k);
        let q = c.completion_query();
        let completions = q.map(|q| self.contact_completions(&q)).unwrap_or_default();
        let Mode::Compose(c) = &mut self.mode else { return };
        if completions != c.completions {
            c.sel = None;
            c.completions = completions;
        }
        match out {
            Outcome::None => {}
            Outcome::Call(call) => self.add_call(call, crate::calview::CallTarget::Form),
            Outcome::CycleFrom => self.cycle_from(),
            Outcome::Say(s) => self.say(s),
            Outcome::Send | Outcome::SaveDraft if matches!(&self.mode, Mode::Compose(c) if c.contact.is_some()) => {
                let Mode::Compose(c) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
                self.save_contact(c);
            }
            Outcome::Send | Outcome::SaveDraft if matches!(&self.mode, Mode::Compose(c) if c.source.is_some()) => {
                let Mode::Compose(c) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
                self.save_source(c);
            }
            Outcome::Send | Outcome::SaveDraft if matches!(&self.mode, Mode::Compose(c) if c.event.is_some()) => {
                let Mode::Compose(c) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
                self.save_event(c);
            }
            Outcome::Send | Outcome::SaveDraft if matches!(&self.mode, Mode::Compose(c) if c.account.is_some()) => {
                let Mode::Compose(c) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
                self.save_account(c);
            }
            Outcome::Attach | Outcome::External if matches!(&self.mode, Mode::Compose(c) if !c.is_mail()) => {
                self.say("not available for events");
            }
            Outcome::Send => {
                let Mode::Compose(c) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
                self.send_draft(c);
            }
            Outcome::Discard if matches!(&self.mode, Mode::Compose(c) if c.account.is_some() || c.source.is_some()) => {
                self.restore_settings();
                self.say("not saved");
            }
            Outcome::Discard => {
                self.mode = Mode::Normal;
                crate::drafts::clear();
                self.say("discarded");
            }
            Outcome::SaveDraft => self.save_draft(),
            Outcome::Attach => {
                let Mode::Compose(c) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
                self.parked = Some(c);
                let start = dirs::home_dir().map(|h| format!("{}/", h.display())).unwrap_or_default();
                self.mode = Mode::Prompt(Prompt {
                    kind: PromptKind::Attach,
                    completions: Self::path_completions(&start),
                    input: start,
                    sel: None,
                });
            }
            Outcome::External => {
                let Mode::Compose(c) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
                let d = c.to_draft();
                self.parked = Some(c);
                self.actions.push(Action::Edit(d));
            }
        }
    }

    fn save_draft(&mut self) {
        let Mode::Compose(c) = &mut self.mode else { return };
        let d = c.to_draft();
        let a = self.acct_idx();
        let Some(drafts) = self.folder_for(a, Role::Drafts) else {
            self.fail("no Drafts folder");
            return;
        };
        match d.build_lenient(&self.accts[a].cfg) {
            Ok(raw) => {
                if let Mode::Compose(c) = &mut self.mode {
                    c.mark_saved();
                }
                self.send(a, Cmd::Append { folder: drafts, raw, flags: "(\\Seen \\Draft)" });
                // The copy this one supersedes goes; later saves only add the new one.
                if let Mode::Compose(c) = &mut self.mode {
                    if let Some((ra, rf, ru)) = c.draft.replaces.take() {
                        self.send(ra, Cmd::Expunge { folder: rf, uids: vec![ru] });
                    }
                }
                self.say("saved to Drafts");
            }
            Err(e) => self.fail(format!("{e:#}")),
        }
    }

    /// Every couple of seconds while a message is being written, keep a copy on disk.
    fn autosave_draft(&mut self) {
        if self.last_autosave.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.last_autosave = Instant::now();
        let Mode::Compose(c) = &self.mode else { return };
        if !c.is_mail() || !c.dirty() {
            return;
        }
        let d = c.to_draft();
        if d.to.trim().is_empty() && d.subject.trim().is_empty() && d.body.trim().is_empty() {
            return;
        }
        let account = c.from_acct.or(Some(self.acct_idx())).and_then(|i| self.accts.get(i)).map(|a| a.cfg.name.clone());
        crate::drafts::save(&crate::drafts::Saved { title: c.title.clone(), account, draft: d, saved_at: crate::outbox::now() });
    }

    /// gr: pick the autosaved message back up.
    fn resume_draft(&mut self) {
        let Some(s) = crate::drafts::load() else {
            return self.say("no unsent message saved");
        };
        let mut c = Composer::new(&s.title, s.draft);
        c.from_acct = s.account.and_then(|n| self.accts.iter().position(|a| a.cfg.name == n));
        c.mode = VMode::Normal;
        self.mode = Mode::Compose(Box::new(c));
        self.say("picked up where you left off");
    }

    /// ⏎ on a message in a Drafts folder: continue it in the composer. Sending it
    /// removes the server copy.
    fn edit_server_draft(&mut self) -> bool {
        if !self.current_folder().is_some_and(|f| f.role == Role::Drafts) {
            return false;
        }
        let (Some(b), Some((a, folder))) = (self.body.clone(), self.current.clone()) else { return false };
        let mut d = compose::draft_from_raw(&b.raw);
        d.replaces = Some((a, folder, b.uid));
        let mut c = Composer::new("Draft", d);
        c.from_acct = Some(a);
        c.mode = VMode::Normal;
        self.mode = Mode::Compose(Box::new(c));
        true
    }

    /// Sending puts the message in the outbox (on disk) with a short undo window; the
    /// outbox then sends it, retrying on failure. Nothing is lost if the network or
    /// tern goes away in between.
    fn send_draft(&mut self, c: Box<Composer>) {
        let d = c.to_draft();
        let a = c.from_acct.unwrap_or_else(|| self.acct_idx()).min(self.accts.len() - 1);
        let acct = self.accts[a].cfg.clone();
        let msg = match d.build(&acct) {
            Ok(m) => m,
            Err(e) => {
                self.fail(format!("{e:#}"));
                self.mode = Mode::Compose(c);
                return;
            }
        };
        if self.demo {
            self.say(format!("demo: would send “{}” to {}", d.subject, d.to));
            return;
        }
        let env = msg.envelope();
        let name = |i: usize| self.accts.get(i).map(|s| s.cfg.name.clone()).unwrap_or_default();
        let undo = self.cfg.ui.undo_send_secs;
        let q = crate::outbox::Queued {
            id: uuid::Uuid::new_v4().to_string(),
            account: acct.name.clone(),
            from: env.from().map(|f| f.to_string()).unwrap_or_else(|| acct.email.clone()),
            to: env.to().iter().map(|t| t.to_string()).collect(),
            subject: d.subject.clone(),
            send_after: crate::outbox::now() + undo as i64,
            attempts: 0,
            last_error: None,
            answers: d.source.as_ref().map(|(i, f, u)| (name(*i), f.clone(), *u)),
            replaces_draft: d.replaces.as_ref().map(|(i, f, u)| (name(*i), f.clone(), *u)),
        };
        if let Err(e) = crate::outbox::put(&q, &msg.formatted()) {
            self.fail(format!("couldn't queue the message: {e:#}"));
            self.mode = Mode::Compose(c);
            return;
        }
        crate::drafts::clear();
        if undo > 0 {
            self.say(format!("sending “{}” in {undo}s · ^z to undo", d.subject));
            self.undo = Some((q.id, c, Instant::now()));
        } else {
            self.say(format!("sending “{}”…", d.subject));
        }
        self.outbox_count = crate::outbox::list().len();
    }

    /// Keys in the outbox panel: ⏎ edit (takes it out of the queue), r retry now,
    /// d drop, esc close.
    fn outbox_key(&mut self, k: KeyEvent) {
        let items = crate::outbox::list();
        let Mode::Outbox(cur) = &mut self.mode else { return };
        let n = items.len();
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            KeyCode::Char('j') | KeyCode::Down => *cur = (*cur + 1).min(n.saturating_sub(1)),
            KeyCode::Char('k') | KeyCode::Up => *cur = cur.saturating_sub(1),
            KeyCode::Char('r') if n > 0 => {
                let mut q = items[(*cur).min(n - 1)].clone();
                q.send_after = 0;
                let _ = crate::outbox::save(&q);
                self.say(format!("retrying “{}”…", q.subject));
            }
            KeyCode::Char('d') if n > 0 => {
                let q = &items[(*cur).min(n - 1)];
                if self.sending.contains(&q.id) {
                    return self.say("it's being sent right now");
                }
                crate::outbox::remove(&q.id);
                *cur = cur.saturating_sub(1);
                self.say(format!("dropped “{}”", q.subject));
            }
            KeyCode::Enter | KeyCode::Char('e') if n > 0 => {
                let q = items[(*cur).min(n - 1)].clone();
                if self.sending.contains(&q.id) {
                    return self.say("it's being sent right now");
                }
                let raw = crate::outbox::raw(&q.id).unwrap_or_default();
                crate::outbox::remove(&q.id);
                let mut d = compose::draft_from_raw(&raw);
                d.source = None;
                let mut c = Composer::new("Edit queued message", d);
                c.from_acct = self.accts.iter().position(|s| s.cfg.name == q.account);
                self.mode = Mode::Compose(Box::new(c));
            }
            _ => {}
        }
        self.outbox_count = crate::outbox::list().len();
    }

    /// ^z right after sending: take the message back out of the outbox.
    fn undo_send(&mut self) {
        let Some((id, c, at)) = self.undo.take() else {
            return self.say("nothing to undo");
        };
        if self.sending.contains(&id) || at.elapsed() > Duration::from_secs(self.cfg.ui.undo_send_secs.max(1) + 1) {
            return self.say("too late — it's already on its way");
        }
        crate::outbox::remove(&id);
        self.outbox_count = crate::outbox::list().len();
        self.mode = Mode::Compose(c);
        self.say("not sent — back in the composer");
    }

    /// Send whatever is due in the outbox. Runs every tick; each message is in flight
    /// at most once.
    fn pump_outbox(&mut self) {
        if self.demo {
            return;
        }
        let now = crate::outbox::now();
        for q in crate::outbox::list() {
            if q.send_after > now || self.sending.contains(&q.id) {
                continue;
            }
            let Some(acct) = self.cfg.accounts.iter().find(|a| a.name == q.account).cloned() else {
                continue;
            };
            let raw = match crate::outbox::raw(&q.id) {
                Ok(r) => r,
                Err(_) => continue,
            };
            self.sending.insert(q.id.clone());
            let tx = self.events_tx.clone();
            tokio::spawn(async move {
                let r = compose::send_raw(&acct, &q.from, &q.to, &raw).await.map_err(|e| format!("{e:#}"));
                let _ = tx.send(Event::OutboxResult(q.id.clone(), r));
            });
        }
    }

    fn on_outbox_result(&mut self, id: String, r: Result<(), String>) {
        self.sending.remove(&id);
        let Some(mut q) = crate::outbox::list().into_iter().find(|q| q.id == id) else { return };
        match r {
            Ok(()) => {
                let raw = crate::outbox::raw(&id).unwrap_or_default();
                crate::outbox::remove(&id);
                let idx = |name: &str| self.accts.iter().position(|s| s.cfg.name == name);
                if let Some(a) = idx(&q.account) {
                    if self.accts[a].cfg.save_sent {
                        if let Some(f) = self.folder_for(a, Role::Sent) {
                            self.send(a, Cmd::Append { folder: f, raw, flags: "(\\Seen)" });
                        }
                    }
                }
                if let Some((acc, folder, uid)) = &q.answers {
                    if let Some(a) = idx(acc) {
                        self.send(a, Cmd::SetFlag { folder: folder.clone(), uids: vec![*uid], flag: "\\Answered", on: true });
                    }
                }
                if let Some((acc, folder, uid)) = &q.replaces_draft {
                    if let Some(a) = idx(acc) {
                        self.send(a, Cmd::Expunge { folder: folder.clone(), uids: vec![*uid] });
                    }
                }
                self.say(format!("sent ✓ “{}”", q.subject));
            }
            Err(e) => {
                q.attempts += 1;
                q.last_error = Some(e.clone());
                q.send_after = crate::outbox::now() + crate::outbox::backoff(q.attempts);
                let _ = crate::outbox::save(&q);
                self.fail(format!("“{}” not sent ({e}) — kept in the outbox, retrying in {}s · go to see it", q.subject, crate::outbox::backoff(q.attempts)));
            }
        }
        self.outbox_count = crate::outbox::list().len();
    }
}

fn ui_state_file() -> PathBuf {
    crate::config::cache_dir().join("state.json")
}

fn load_ui_state() -> HashSet<String> {
    std::fs::read(ui_state_file())
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("collapsed").cloned())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

fn save_ui_state(collapsed: &HashSet<String>) {
    let _ = std::fs::create_dir_all(crate::config::cache_dir());
    let _ = std::fs::write(ui_state_file(), serde_json::json!({ "collapsed": collapsed }).to_string());
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn sanitize(name: &str) -> String {
    let n: String = name.chars().map(|c| if c == '/' || c == '\0' { '_' } else { c }).collect();
    if n.is_empty() || n == "." || n == ".." {
        "attachment".into()
    } else {
        n
    }
}

fn unique_path(dir: &std::path::Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.to_string(), String::new()),
    };
    (1..)
        .map(|i| dir.join(format!("{stem}-{i}{ext}")))
        .find(|p| !p.exists())
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};

    fn app(n: u32) -> App {
        let cfg: Config = toml::from_str(
            "[[accounts]]\nname='t'\nemail='me@x'\n[accounts.imap]\nhost='h'\nusername='u'\npassword='p'\n[accounts.smtp]\nhost='h'\nusername='u'\npassword='p'\n[ui]\nthreads=false\n",
        )
        .unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (ev, _) = tokio::sync::mpsc::unbounded_channel();
        let acct = AcctState { cfg: cfg.accounts[0].clone(), folders: vec![], tx, connected: true };
        let mut a = App::new(cfg, vec![acct], ev);
        a.threaded = false;
        a.current = Some((0, "INBOX".into()));
        a.all = (0..n)
            .map(|i| Envelope {
                uid: 100 - i,
                from_name: String::new(),
                from_addr: String::new(),
                to: String::new(),
                subject: format!("m{i}"),
                date: Some(Local.timestamp_opt(1_700_000_000 - i as i64 * 60, 0).unwrap()),
                seen: true,
                flagged: false,
                answered: false,
                has_attachment: false,
                message_id: String::new(),
                acct: 0,
                in_reply_to: String::new(),
                references: vec![],
            })
            .collect();
        a.refilter();
        a
    }

    #[test]
    fn delete_marked_keeps_highlight() {
        let mut a = app(20);
        // Mark rows 2..12, then highlight row 15 (below all of them).
        a.marked = (2..12).map(|i| 100 - i).collect();
        a.set_cursor(15);
        let uid = a.selected().unwrap().uid;
        a.trash();
        assert_eq!(a.all.len(), 10);
        assert_eq!(a.selected().unwrap().uid, uid);
        // The server's confirmation runs the removal again; still put.
        a.remove_local(&(2..12).map(|i| 100 - i).collect::<Vec<_>>());
        assert_eq!(a.selected().unwrap().uid, uid);
    }

    #[test]
    fn c_opens_the_calendar_and_back() {
        let mut a = app(3);
        a.demo = true;
        a.on_key(KeyEvent::new(KeyCode::Char('C'), KeyModifiers::SHIFT));
        assert_eq!(a.screen, crate::calview::View::Calendar);
        a.on_key(KeyEvent::new(KeyCode::Char('C'), KeyModifiers::SHIFT));
        assert_eq!(a.screen, crate::calview::View::Mail);
    }

    #[test]
    fn switching_folders_drops_old_threads() {
        let mut a = app(5);
        a.threaded = true;
        a.refilter();
        a.open_folder(0, "Sent".into());
        assert!(a.rows.is_empty() && a.threads.is_empty() && a.conv_thread().is_none());
    }

    #[test]
    fn deleting_the_highlighted_moves_to_the_next() {
        let mut a = app(5);
        a.set_cursor(1);
        a.trash();
        assert_eq!(a.selected().unwrap().subject, "m2");
        a.set_cursor(3);
        a.trash();
        // Last row gone: fall back to the one above.
        assert_eq!(a.selected().unwrap().subject, "m3");
    }
}
