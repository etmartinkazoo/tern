//! The in-pane composer: header fields and a body with vim-style modal editing, drawn in
//! the message column so the list stays in view while you write.

use crate::compose::Draft;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_textarea::{CursorMove, Input, TextArea};
use unicode_width::UnicodeWidthChar;

// Mail field positions.
pub const TO: usize = 0;
pub const CC: usize = 1;
pub const BCC: usize = 2;
pub const SUBJECT: usize = 3;

/// What an account form is editing (index None = a new account).
#[derive(Clone, Debug)]
pub struct AccountForm {
    pub index: Option<usize>,
    pub old_name: Option<String>,
    pub oauth: String,
    pub save_sent: bool,
}

/// What a calendar/contacts source form is editing (index None = new).
#[derive(Clone, Debug)]
pub struct SourceForm {
    pub index: Option<usize>,
    pub old_name: Option<String>,
}

/// What an event form is editing: which calendar, and which resource (None = new).
#[derive(Clone, Debug)]
pub struct EventForm {
    pub calendar: usize,
    pub item: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VMode {
    Normal,
    Insert,
    Visual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    Meet,
    Zoom,
}

/// What the app should do after a key.
pub enum Outcome {
    None,
    Send,
    SaveDraft,
    Discard,
    Attach,
    External,
    /// ^f: send from the next account.
    CycleFrom,
    /// `:meet` / `:zoom`: put a call link on this event.
    Call(Call),
    Say(String),
}

pub struct Composer {
    pub title: String,
    pub draft: Draft,
    pub labels: Vec<&'static str>,
    pub placeholders: Vec<&'static str>,
    pub fields: Vec<TextArea<'static>>,
    /// How many leading fields hold addresses (contact completion); 0 for events.
    addr_fields: usize,
    /// Set when this form edits a calendar event instead of a message.
    pub event: Option<EventForm>,
    /// Set when this form edits an account.
    pub account: Option<AccountForm>,
    /// Set when this form edits a calendar/contacts source.
    pub source: Option<SourceForm>,
    /// Set when this form edits a contact.
    pub contact: Option<crate::people::ContactForm>,
    /// A line of guidance under the title.
    pub note: String,
    /// The account a message is sent from (None: the open folder's).
    pub from_acct: Option<usize>,
    pub body: TextArea<'static>,
    pub focus: usize,
    pub mode: VMode,
    /// `:` command line being typed, if any.
    pub command: Option<String>,
    pending: Option<char>,
    pub completions: Vec<String>,
    pub sel: Option<usize>,
    /// First visible row of the soft-wrapped body.
    pub scroll: usize,
    original: String,
}

fn area(text: &str) -> TextArea<'static> {
    let lines: Vec<String> = if text.is_empty() { vec![String::new()] } else { text.lines().map(String::from).collect() };
    let mut t = TextArea::new(lines);
    t.set_cursor_line_style(ratatui::style::Style::default());
    t
}

impl Composer {
    pub fn new(title: &str, draft: Draft) -> Self {
        let fields = vec![area(&draft.to), area(&draft.cc), area(&draft.bcc), area(&draft.subject)];
        let mut body = area(&draft.body);
        body.move_cursor(CursorMove::Top);
        let focus = if draft.to.trim().is_empty() {
            TO
        } else if draft.subject.trim().is_empty() {
            SUBJECT
        } else {
            fields.len()
        };
        let mut c = Composer {
            title: title.to_string(),
            original: String::new(),
            draft,
            labels: vec!["To", "Cc", "Bcc", "Subject"],
            placeholders: vec!["name or address — tab completes", "", "", ""],
            fields,
            addr_fields: 3,
            event: None,
            account: None,
            source: None,
            contact: None,
            note: String::new(),
            from_acct: None,
            body,
            focus,
            mode: VMode::Insert,
            command: None,
            pending: None,
            completions: vec![],
            sel: None,
            scroll: 0,
        };
        for f in c.fields.iter_mut() {
            f.move_cursor(CursorMove::End);
        }
        c.original = c.snapshot();
        c
    }

    /// A form for a calendar event, in the same pane with the same keys.
    pub fn new_event(title: &str, d: &crate::when::Draft, form: EventForm) -> Self {
        let date = d.date.format("%Y-%m-%d").to_string();
        let (start, end) = match (d.start, d.end) {
            (Some(s), Some(e)) => (s.format("%H:%M").to_string(), e.format("%H:%M").to_string()),
            _ if d.days > 1 => (String::new(), (d.date + chrono::Duration::days(d.days - 1)).format("%Y-%m-%d").to_string()),
            _ => (String::new(), String::new()),
        };
        let fields = vec![area(&d.title), area(&date), area(&start), area(&end), area(&d.location)];
        let mut body = area(&d.description);
        body.move_cursor(CursorMove::Top);
        let mut c = Composer {
            title: title.to_string(),
            draft: Draft::default(),
            labels: vec!["Title", "Date", "Start", "End", "Where"],
            placeholders: vec![
                "what",
                "2026-10-16 · fri · tomorrow · oct 20",
                "14:00 · 3pm · empty for all day",
                "15:30 · 1h · or the last day of an all-day event",
                "",
            ],
            fields,
            addr_fields: 0,
            event: Some(form),
            account: None,
            source: None,
            contact: None,
            note: String::new(),
            from_acct: None,
            body,
            focus: 0,
            mode: if d.title.is_empty() { VMode::Insert } else { VMode::Normal },
            command: None,
            pending: None,
            completions: vec![],
            sel: None,
            scroll: 0,
            original: String::new(),
        };
        for f in c.fields.iter_mut() {
            f.move_cursor(CursorMove::End);
        }
        c.original = c.snapshot();
        c
    }

    /// The account form: servers, sign-in, calendar and contacts; the body is the
    /// signature.
    pub fn new_account(title: &str, spec: &crate::accounts::Spec, form: AccountForm, note: &str) -> Self {
        let fields: Vec<TextArea<'static>> = spec.fields().iter().map(|v| area(v)).collect();
        let mut body = area(&spec.signature);
        body.move_cursor(CursorMove::Top);
        let mut c = Composer {
            title: title.to_string(),
            draft: Draft::default(),
            labels: crate::accounts::LABELS.to_vec(),
            placeholders: crate::accounts::PLACEHOLDERS.to_vec(),
            fields,
            addr_fields: 0,
            event: None,
            account: Some(form),
            source: None,
            contact: None,
            note: note.to_string(),
            from_acct: None,
            body,
            focus: 0,
            mode: if spec.name.is_empty() { VMode::Insert } else { VMode::Normal },
            command: None,
            pending: None,
            completions: vec![],
            sel: None,
            scroll: 0,
            original: String::new(),
        };
        for f in c.fields.iter_mut() {
            f.move_cursor(CursorMove::End);
        }
        c.original = c.snapshot();
        c
    }

    /// A calendar/contacts server form.
    pub fn new_source(title: &str, spec: &crate::accounts::DavSpec, form: SourceForm, note: &str) -> Self {
        let mut c = Composer::new_account(title, &crate::accounts::Spec::default(), crate::composer::AccountForm { index: None, old_name: None, oauth: String::new(), save_sent: true }, note);
        c.account = None;
        c.source = Some(form);
        c.labels = crate::accounts::DAV_LABELS.to_vec();
        c.placeholders = crate::accounts::DAV_PLACEHOLDERS.to_vec();
        c.fields = spec.fields().iter().map(|v| area(v)).collect();
        for f in c.fields.iter_mut() {
            f.move_cursor(CursorMove::End);
        }
        c.mode = if spec.name.is_empty() { VMode::Insert } else { VMode::Normal };
        c.mark_saved();
        c
    }

    /// A message form (not an event, account, source or contact).
    pub fn is_mail(&self) -> bool {
        self.event.is_none() && self.account.is_none() && self.source.is_none() && self.contact.is_none()
    }

    /// A generic form: labelled one-line fields over a body.
    pub fn new_form(title: &str, labels: &[&'static str], placeholders: &[&'static str], values: &[String], body: &str, note: &str) -> Self {
        let mut c = Composer::new_account(title, &crate::accounts::Spec::default(), AccountForm { index: None, old_name: None, oauth: String::new(), save_sent: true }, note);
        c.account = None;
        c.labels = labels.to_vec();
        c.placeholders = placeholders.to_vec();
        c.fields = values.iter().map(|v| area(v)).collect();
        for f in c.fields.iter_mut() {
            f.move_cursor(CursorMove::End);
        }
        let mut b = area(body);
        b.move_cursor(CursorMove::Top);
        c.body = b;
        c.mode = if values.first().is_some_and(|v| v.is_empty()) { VMode::Insert } else { VMode::Normal };
        c.mark_saved();
        c
    }

    /// Replace a single-line field's text.
    pub fn set_field(&mut self, i: usize, text: &str) {
        if i < self.fields.len() {
            let mut t = area(text);
            t.move_cursor(CursorMove::End);
            self.fields[i] = t;
        }
    }

    /// Every single-line field's text, in order.
    pub fn values(&self) -> Vec<String> {
        self.fields.iter().map(|f| f.lines().join(" ").trim().to_string()).collect()
    }

    /// Swap one signature for another in the body, wherever the old one sits.
    pub fn swap_signature(&mut self, old: Option<&str>, new: Option<&str>) {
        let text = self.body.lines().join("\n");
        let replaced = match (old.filter(|s| !s.is_empty()), new) {
            (Some(o), n) if text.contains(o) => text.replacen(o, n.unwrap_or(""), 1),
            (None, Some(n)) if !n.is_empty() && !text.contains(n) => format!("{}\n\n{n}", text.trim_end()),
            _ => return,
        };
        let (row, col) = self.body.cursor();
        let mut t = area(&replaced);
        t.move_cursor(CursorMove::Jump(row as u16, col as u16));
        self.body = t;
    }

    pub fn body_text(&self) -> String {
        self.body.lines().join("\n").trim_end().to_string()
    }

    /// Read the event form back. Dates and times accept what quick add accepts.
    pub fn event_draft(&self, today: chrono::NaiveDate) -> Result<crate::when::Draft, String> {
        use crate::when;
        let f = |i: usize| self.fields[i].lines().join(" ").trim().to_string();
        let title = f(0);
        if title.is_empty() {
            return Err("the event needs a title".into());
        }
        let date_s = f(1);
        let words: Vec<&str> = date_s.split_whitespace().collect();
        let date = if words.is_empty() {
            today
        } else {
            when::date_at(&words, today).map(|(d, _)| d).ok_or_else(|| format!("can't read the date `{date_s}`"))?
        };
        let (start_s, end_s) = (f(2), f(3));
        let start = if start_s.is_empty() {
            None
        } else {
            Some(when::time(&start_s).or_else(|| when::time(&format!("{start_s}:00"))).ok_or_else(|| format!("can't read the start `{start_s}`"))?)
        };
        let (end, days) = match start {
            Some(s) => {
                let e = if end_s.is_empty() {
                    s + chrono::Duration::hours(1)
                } else if let Some(t) = when::time(&end_s).or_else(|| when::time(&format!("{end_s}:00"))) {
                    t
                } else {
                    let ws: Vec<&str> = end_s.split_whitespace().collect();
                    s + when::duration(&ws).map(|(d, _)| d).ok_or_else(|| format!("can't read the end `{end_s}`"))?
                };
                (Some(e), 1)
            }
            None => {
                let days = if end_s.is_empty() {
                    1
                } else {
                    let ws: Vec<&str> = end_s.split_whitespace().collect();
                    let last = when::date_at(&ws, date).map(|(d, _)| d).ok_or_else(|| format!("can't read the last day `{end_s}`"))?;
                    ((last - date).num_days() + 1).max(1)
                };
                (None, days)
            }
        };
        Ok(when::Draft {
            title,
            date,
            start,
            end,
            days,
            // A link typed (or :meet / :zoom'd) into Where is also the event's URL.
            url: if f(4).starts_with("http") { f(4) } else { String::new() },
            location: f(4),
            description: self.body.lines().join("\n").trim_end().to_string(),
        })
    }

    fn body_idx(&self) -> usize {
        self.fields.len()
    }

    /// Where Enter in an address field lands.
    fn subject_idx(&self) -> usize {
        if self.addr_fields > 0 {
            SUBJECT
        } else {
            self.focus + 1
        }
    }

    pub fn on_body(&self) -> bool {
        self.focus == self.body_idx()
    }

    fn snapshot(&self) -> String {
        let mut s: String = self.fields.iter().map(|f| f.lines().join(" ")).collect::<Vec<_>>().join("\n");
        s.push('\n');
        s.push_str(&self.body.lines().join("\n"));
        s
    }

    pub fn mark_saved(&mut self) {
        self.original = self.snapshot();
    }

    pub fn dirty(&self) -> bool {
        self.snapshot() != self.original
    }

    /// The draft with the editor's current contents.
    pub fn to_draft(&self) -> Draft {
        let field = |i: usize| self.fields[i].lines().join(" ").trim().to_string();
        let mut d = self.draft.clone();
        d.to = field(TO).trim_end_matches(',').trim().to_string();
        d.cc = field(CC).trim_end_matches(',').trim().to_string();
        d.bcc = field(BCC).trim_end_matches(',').trim().to_string();
        d.subject = field(SUBJECT);
        d.body = self.body.lines().join("\n").trim_end().to_string() + "\n";
        d
    }

    pub fn is_address_field(&self) -> bool {
        self.focus < self.addr_fields
    }

    /// The text being completed: the part of the address field after the last comma.
    pub fn completion_query(&self) -> Option<String> {
        if !self.is_address_field() || self.mode != VMode::Insert {
            return None;
        }
        let line = self.fields[self.focus].lines().join(" ");
        let last = line.rsplit(',').next().unwrap_or("").trim().to_string();
        (!last.is_empty()).then_some(last)
    }

    fn accept_completion(&mut self) -> bool {
        let Some(pick) = self.sel.or((!self.completions.is_empty()).then_some(0)).and_then(|i| self.completions.get(i).cloned()) else {
            return false;
        };
        let line = self.fields[self.focus].lines().join(" ");
        let head = match line.rfind(',') {
            Some(i) => format!("{}, ", line[..i].trim_end()),
            None => String::new(),
        };
        let mut t = area(&format!("{head}{pick}, "));
        t.move_cursor(CursorMove::End);
        self.fields[self.focus] = t;
        self.completions.clear();
        self.sel = None;
        true
    }

    fn cur(&mut self) -> &mut TextArea<'static> {
        if self.focus == self.body_idx() {
            &mut self.body
        } else {
            &mut self.fields[self.focus]
        }
    }

    fn focus_to(&mut self, f: usize) {
        if self.mode == VMode::Visual {
            self.cur().cancel_selection();
            self.mode = VMode::Normal;
        }
        // A source form has no body to move into.
        let last = if self.source.is_some() { self.fields.len() - 1 } else { self.body_idx() };
        self.focus = f.min(last);
        self.completions.clear();
        self.sel = None;
    }

    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        // Available in every mode.
        if ctrl {
            match k.code {
                KeyCode::Char('s') => return Outcome::Send,
                KeyCode::Char('t') => return Outcome::Attach,
                KeyCode::Char('f') if self.is_mail() => return Outcome::CycleFrom,
                KeyCode::Char('e') if self.mode != VMode::Insert => return Outcome::External,
                _ => {}
            }
        }
        if let Some(cmd) = self.command.as_mut() {
            match k.code {
                KeyCode::Esc => self.command = None,
                KeyCode::Backspace => {
                    if cmd.pop().is_none() {
                        self.command = None;
                    }
                }
                KeyCode::Enter => {
                    let cmd = self.command.take().unwrap_or_default();
                    return self.run_command(cmd.trim());
                }
                KeyCode::Char(c) => cmd.push(c),
                _ => {}
            }
            return Outcome::None;
        }
        match self.mode {
            VMode::Insert => self.insert_key(k),
            VMode::Normal | VMode::Visual => self.normal_key(k),
        }
    }

    fn run_command(&mut self, cmd: &str) -> Outcome {
        let (name, arg) = cmd.split_once(' ').map(|(a, b)| (a, b.trim())).unwrap_or((cmd, ""));
        match name {
            "w" => Outcome::SaveDraft,
            "wq" | "x" | "send" => Outcome::Send,
            "q" if self.dirty() => Outcome::Say("unsent changes — :q! to discard, :wq to send".into()),
            "q" | "q!" | "cq" => Outcome::Discard,
            "e" | "edit" => Outcome::External,
            "meet" if self.event.is_some() => Outcome::Call(Call::Meet),
            "zoom" if self.event.is_some() => Outcome::Call(Call::Zoom),
            "a" | "attach" if !arg.is_empty() => {
                let p = crate::theme::expand(arg);
                if p.is_file() {
                    self.draft.attach_paths.push(p);
                    Outcome::Say(format!("attached {arg}"))
                } else {
                    Outcome::Say(format!("not a file: {arg}"))
                }
            }
            "a" | "attach" => Outcome::Attach,
            "cc" | "bcc" | "to" | "subject" => {
                let i = ["to", "cc", "bcc", "subject"].iter().position(|n| *n == name).unwrap();
                self.focus_to(i);
                self.mode = VMode::Insert;
                Outcome::None
            }
            "" => Outcome::None,
            _ => Outcome::Say(format!("unknown command :{name}")),
        }
    }

    fn insert_key(&mut self, k: KeyEvent) -> Outcome {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let n = self.completions.len();
        match k.code {
            KeyCode::Esc => {
                self.mode = VMode::Normal;
                self.completions.clear();
                self.sel = None;
                let t = self.cur();
                if t.cursor().1 > 0 {
                    t.move_cursor(CursorMove::Back);
                }
                return Outcome::None;
            }
            KeyCode::Char('n') if ctrl && n > 0 => {
                self.sel = Some(self.sel.map_or(0, |s| (s + 1) % n));
                return Outcome::None;
            }
            KeyCode::Char('p') if ctrl && n > 0 => {
                self.sel = Some(self.sel.map_or(n - 1, |s| (s + n - 1) % n));
                return Outcome::None;
            }
            KeyCode::Down if n > 0 && self.focus < self.body_idx() => {
                self.sel = Some(self.sel.map_or(0, |s| (s + 1) % n));
                return Outcome::None;
            }
            KeyCode::Up if n > 0 && self.focus < self.body_idx() => {
                self.sel = Some(self.sel.map_or(n - 1, |s| (s + n - 1) % n));
                return Outcome::None;
            }
            KeyCode::Tab if self.focus < self.body_idx() => {
                if !self.accept_completion() {
                    self.focus_to(self.focus + 1);
                }
                return Outcome::None;
            }
            KeyCode::BackTab => {
                self.focus_to(self.focus.saturating_sub(1));
                return Outcome::None;
            }
            // Vim's insert-mode ^u (the widget would undo instead).
            KeyCode::Char('u') if ctrl => {
                self.cur().delete_line_by_head();
                return Outcome::None;
            }
            // Enter takes the suggestion on screen; otherwise address fields jump to
            // Subject (Tab visits Cc/Bcc) and Subject drops into the body.
            KeyCode::Enter if self.focus < self.body_idx() => {
                if !self.accept_completion() {
                    let next = if self.focus < self.addr_fields { self.subject_idx() } else { self.focus + 1 };
                    self.focus_to(next);
                }
                return Outcome::None;
            }
            KeyCode::Down if self.focus < self.body_idx() => {
                self.focus_to(self.focus + 1);
                return Outcome::None;
            }
            KeyCode::Up if self.focus < self.body_idx() => {
                self.focus_to(self.focus.saturating_sub(1));
                return Outcome::None;
            }
            KeyCode::Up if self.on_body() && self.body.cursor().0 == 0 => {
                self.focus_to(self.body_idx() - 1);
                return Outcome::None;
            }
            _ => {}
        }
        let _ = shift;
            // Keep the textarea's emacs-style ctrl keys (^w ^u ^h) but not ones that would
        // fight the composer (^n ^p move lines, ^m/^j newline in header fields).
        let t = self.cur();
        t.input(Input::from(k));
        Outcome::None
    }

    fn normal_key(&mut self, k: KeyEvent) -> Outcome {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let visual = self.mode == VMode::Visual;
        let on_body = self.on_body();

        if let Some(p) = self.pending.take() {
            return self.pending_key(p, k);
        }

        let KeyCode::Char(c) = k.code else {
            match k.code {
                KeyCode::Esc => {
                    if visual {
                        self.cur().cancel_selection();
                        self.mode = VMode::Normal;
                    }
                }
                KeyCode::Tab => self.focus_to(self.focus + 1),
                KeyCode::BackTab => self.focus_to(self.focus.saturating_sub(1)),
                KeyCode::Left => self.cur().move_cursor(CursorMove::Back),
                KeyCode::Right => self.cur().move_cursor(CursorMove::Forward),
                KeyCode::Down => return self.normal_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)),
                KeyCode::Up => return self.normal_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE)),
                KeyCode::Enter => return self.normal_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)),
                _ => {}
            }
            return Outcome::None;
        };

        if ctrl {
            match c {
                'r' => {
                    self.cur().redo();
                }
                'd' if on_body => {
                    for _ in 0..10 {
                        self.body.move_cursor(CursorMove::Down)
                    }
                }
                'u' if on_body => {
                    for _ in 0..10 {
                        self.body.move_cursor(CursorMove::Up)
                    }
                }
                _ => {}
            }
            return Outcome::None;
        }

        match c {
            'h' => self.cur().move_cursor(CursorMove::Back),
            'l' => self.cur().move_cursor(CursorMove::Forward),
            'j' => {
                if on_body {
                    self.body.move_cursor(CursorMove::Down)
                } else if !visual {
                    self.focus_to(self.focus + 1)
                }
            }
            'k' => {
                if on_body && self.body.cursor().0 > 0 {
                    self.body.move_cursor(CursorMove::Up)
                } else if !visual && self.focus > 0 {
                    self.focus_to(self.focus - 1)
                }
            }
            'w' => self.cur().move_cursor(CursorMove::WordForward),
            'b' => self.cur().move_cursor(CursorMove::WordBack),
            'e' => self.cur().move_cursor(CursorMove::WordEnd),
            '0' | '^' => self.cur().move_cursor(CursorMove::Head),
            '$' => self.cur().move_cursor(CursorMove::End),
            '}' => self.cur().move_cursor(CursorMove::ParagraphForward),
            '{' => self.cur().move_cursor(CursorMove::ParagraphBack),
            'G' => {
                if on_body {
                    self.body.move_cursor(CursorMove::Bottom)
                } else {
                    self.focus_to(self.body_idx());
                    self.body.move_cursor(CursorMove::Bottom)
                }
            }
            'g' | 'd' | 'c' | 'y' | 'Z' | 'r' if !visual || c == 'g' => self.pending = Some(c),
            ':' => self.command = Some(String::new()),
            'v' => {
                if visual {
                    self.cur().cancel_selection();
                    self.mode = VMode::Normal;
                } else {
                    self.cur().start_selection();
                    self.mode = VMode::Visual;
                }
            }
            'V' if !visual => {
                let t = self.cur();
                t.move_cursor(CursorMove::Head);
                t.start_selection();
                t.move_cursor(CursorMove::End);
                self.mode = VMode::Visual;
            }
            'y' if visual => {
                self.cur().copy();
                self.mode = VMode::Normal;
            }
            'd' | 'x' if visual => {
                self.cur().cut();
                self.mode = VMode::Normal;
            }
            'c' if visual => {
                self.cur().cut();
                self.mode = VMode::Insert;
            }
            'x' => {
                self.cur().delete_next_char();
            }
            'X' => {
                self.cur().delete_char();
            }
            'D' => {
                self.cur().delete_line_by_end();
            }
            'C' => {
                self.cur().delete_line_by_end();
                self.mode = VMode::Insert;
            }
            'S' => {
                let t = self.cur();
                t.move_cursor(CursorMove::Head);
                t.delete_line_by_end();
                self.mode = VMode::Insert;
            }
            'J' if on_body => {
                let t = self.cur();
                t.move_cursor(CursorMove::End);
                if t.delete_next_char() {
                    t.insert_char(' ');
                }
            }
            'p' => self.paste(true),
            'P' => self.paste(false),
            'u' => {
                self.cur().undo();
            }
            'i' => self.mode = VMode::Insert,
            'a' => {
                self.cur().move_cursor(CursorMove::Forward);
                self.mode = VMode::Insert;
            }
            'I' => {
                self.cur().move_cursor(CursorMove::Head);
                self.mode = VMode::Insert;
            }
            'A' => {
                self.cur().move_cursor(CursorMove::End);
                self.mode = VMode::Insert;
            }
            'o' | 'O' if on_body => {
                let t = self.cur();
                if c == 'o' {
                    t.move_cursor(CursorMove::End);
                    t.insert_newline();
                } else {
                    t.move_cursor(CursorMove::Head);
                    t.insert_newline();
                    t.move_cursor(CursorMove::Up);
                }
                self.mode = VMode::Insert;
            }
            _ => {}
        }
        Outcome::None
    }

    fn pending_key(&mut self, p: char, k: KeyEvent) -> Outcome {
        let KeyCode::Char(c) = k.code else { return Outcome::None };
        match (p, c) {
            ('g', 'g') => {
                if self.on_body() && self.body.cursor().0 > 0 {
                    self.body.move_cursor(CursorMove::Top);
                } else {
                    self.focus_to(TO);
                }
            }
            ('Z', 'Z') => return Outcome::Send,
            ('Z', 'Q') => return Outcome::Discard,
            ('d', 'd') => self.delete_line(),
            ('c', 'c') => {
                let t = self.cur();
                t.move_cursor(CursorMove::Head);
                t.delete_line_by_end();
                self.mode = VMode::Insert;
            }
            ('y', 'y') => {
                let line = {
                    let t = self.cur();
                    t.lines()[t.cursor().0].clone()
                };
                self.cur().set_yank_text(format!("{line}\n"));
            }
            ('d', 'w') => {
                self.cur().delete_next_word();
            }
            ('c', 'w') | ('c', 'e') => {
                self.cur().delete_next_word();
                self.mode = VMode::Insert;
            }
            ('d', 'b') => {
                self.cur().delete_word();
            }
            ('d', '$') => {
                self.cur().delete_line_by_end();
            }
            ('d', '0') => {
                self.cur().delete_line_by_head();
            }
            ('r', ch) => {
                let t = self.cur();
                if t.delete_next_char() {
                    t.insert_char(ch);
                    t.move_cursor(CursorMove::Back);
                }
            }
            _ => {}
        }
        Outcome::None
    }

    fn delete_line(&mut self) {
        let t = self.cur();
        let (row, _) = t.cursor();
        let n = t.lines().len();
        let line = t.lines()[row].clone();
        if n == 1 {
            t.move_cursor(CursorMove::Head);
            t.delete_line_by_end();
        } else if row + 1 == n {
            t.move_cursor(CursorMove::Up);
            t.move_cursor(CursorMove::End);
            t.start_selection();
            t.move_cursor(CursorMove::Bottom);
            t.move_cursor(CursorMove::End);
            t.cut();
            t.move_cursor(CursorMove::Head);
        } else {
            t.move_cursor(CursorMove::Head);
            t.start_selection();
            t.move_cursor(CursorMove::Down);
            t.move_cursor(CursorMove::Head);
            t.cut();
        }
        t.set_yank_text(format!("{line}\n"));
    }

    /// Linewise when the yank ends in a newline (from dd/yy), like vim.
    fn paste(&mut self, after: bool) {
        let t = self.cur();
        let text = t.yank_text();
        if let Some(line) = text.strip_suffix('\n') {
            if after {
                t.move_cursor(CursorMove::End);
                t.insert_newline();
            } else {
                t.move_cursor(CursorMove::Head);
                t.insert_newline();
                t.move_cursor(CursorMove::Up);
            }
            t.set_yank_text(line);
            t.paste();
            t.set_yank_text(text);
            t.move_cursor(CursorMove::Head);
        } else {
            if after {
                t.move_cursor(CursorMove::Forward);
            }
            t.paste();
        }
    }
}

/// Soft-wrap one line to `width` columns as (start, end) char ranges. Breaks after the
/// last space that fits, splitting words only when one is wider than the line. Display
/// only: the text you send keeps one line per paragraph, so every client wraps it to fit.
pub fn layout(line: &str, width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut w = 0;
        let mut end = start;
        let mut last_space = None;
        while end < chars.len() {
            let cw = chars[end].width().unwrap_or(0);
            if w + cw > width {
                break;
            }
            w += cw;
            if chars[end] == ' ' {
                last_space = Some(end);
            }
            end += 1;
        }
        if end < chars.len() {
            if let Some(sp) = last_space.filter(|&sp| sp > start) {
                end = sp + 1;
            } else if end == start {
                end += 1;
            }
        }
        out.push((start, end));
        start = end;
    }
    if out.is_empty() {
        out.push((0, 0));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(c: &mut Composer, keys: &str) {
        for ch in keys.chars() {
            let code = match ch {
                '\x1b' => KeyCode::Esc,
                '\n' => KeyCode::Enter,
                ch => KeyCode::Char(ch),
            };
            c.key(KeyEvent::new(code, KeyModifiers::NONE));
        }
    }

    fn new(body: &str) -> Composer {
        Composer::new("t", Draft { to: "a@b.c".into(), subject: "s".into(), body: body.into(), ..Default::default() })
    }

    #[test]
    fn dd_and_p_move_a_line() {
        let mut c = new("one\ntwo\nthree");
        press(&mut c, "\x1bddp");
        assert_eq!(c.body.lines(), ["two", "one", "three"]);
    }

    #[test]
    fn soft_wrap_layout() {
        // Breaks after the last space that fits; nothing is inserted into the text.
        assert_eq!(layout("the quick brown fox jumps", 10), vec![(0, 10), (10, 20), (20, 25)]);
        // A word longer than the width is split.
        assert_eq!(layout("abcdefghijkl", 5), vec![(0, 5), (5, 10), (10, 12)]);
        assert_eq!(layout("", 5), vec![(0, 0)]);
        let mut c = new("");
        press(&mut c, "the quick brown fox jumps over the lazy dog");
        assert_eq!(c.body.lines(), ["the quick brown fox jumps over the lazy dog"]);
    }

    #[test]
    fn insert_and_command() {
        let mut c = new("hello");
        press(&mut c, "\x1bAworld\x1b");
        assert!(c.dirty());
        press(&mut c, ":q\n");
        assert!(matches!(c.key(KeyEvent::new(KeyCode::Char('Z'), KeyModifiers::NONE)), Outcome::None));
        assert!(matches!(c.key(KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::NONE)), Outcome::Discard));
        assert_eq!(c.to_draft().body, "helloworld\n");
    }

    #[test]
    fn signature_swaps() {
        let mut c = new("Hi\n\n-- \nTed (personal)");
        c.swap_signature(Some("-- \nTed (personal)"), Some("-- \nTed Martin · Lumin"));
        assert_eq!(c.body.lines().join("\n"), "Hi\n\n-- \nTed Martin · Lumin");
        c.swap_signature(Some("-- \nTed Martin · Lumin"), None);
        assert_eq!(c.body.lines().join("\n").trim_end(), "Hi");
    }

    #[test]
    fn field_navigation() {
        let mut c = new("x");
        assert!(c.on_body());
        press(&mut c, "\x1bkk");
        assert_eq!(c.focus, BCC);
        press(&mut c, "jj");
        assert!(c.on_body());
    }
}
