//! The calendar view: calendars down the left, an agenda in the middle, the selected
//! event on the right. Read-only for now; everything comes from the DAV cache.

use crate::app::{Action, App, Focus};
use crate::calendar::{self, Occurrence};
use crate::dav::CachedCalendar;
use crate::model::Event;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashSet;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Mail,
    Calendar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Day(NaiveDate),
    Event(usize),
}

pub struct CalState {
    pub calendars: Vec<CachedCalendar>,
    /// (source, id) of calendars switched off.
    pub hidden: HashSet<(String, String)>,
    pub occ: Vec<Occurrence>,
    pub rows: Vec<Row>,
    pub cursor: usize,
    pub offset: usize,
    pub height: usize,
    pub side_cursor: usize,
    /// The day the agenda is anchored on.
    pub date: NaiveDate,
    window: (NaiveDate, NaiveDate),
    pub filter: String,
    pub syncing: bool,
    pub synced_at: Option<Instant>,
    pub detail_scroll: usize,
    pending_g: bool,
}

const BEFORE: i64 = 14;
const AFTER: i64 = 120;

fn day_start(d: NaiveDate) -> DateTime<Local> {
    Local.from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap()).earliest().unwrap_or_else(Local::now)
}

impl CalState {
    pub fn new() -> Self {
        let today = Local::now().date_naive();
        CalState {
            calendars: crate::dav::load_cache(),
            hidden: HashSet::new(),
            occ: vec![],
            rows: vec![],
            cursor: 0,
            offset: 0,
            height: 20,
            side_cursor: 0,
            date: today,
            window: (today, today),
            filter: String::new(),
            syncing: false,
            synced_at: None,
            detail_scroll: 0,
            pending_g: false,
        }
    }

    pub fn visible(&self, i: usize) -> bool {
        let c = &self.calendars[i];
        !self.hidden.contains(&(c.source.clone(), c.id.clone()))
    }

    /// Re-expand every visible calendar over a window around the anchor day.
    pub fn expand(&mut self) {
        let from = self.date - Duration::days(BEFORE);
        let to = self.date + Duration::days(AFTER);
        self.window = (from, to);
        let (a, b) = (day_start(from), day_start(to));
        let mut occ = Vec::new();
        for (i, c) in self.calendars.iter().enumerate() {
            if !self.visible(i) {
                continue;
            }
            for ics in c.items.values() {
                occ.extend(calendar::expand(ics, i, a, b));
            }
        }
        let q = self.filter.to_lowercase();
        if !q.is_empty() {
            occ.retain(|o| {
                o.summary.to_lowercase().contains(&q)
                    || o.location.to_lowercase().contains(&q)
                    || o.description.to_lowercase().contains(&q)
            });
        }
        occ.sort_by(|x, y| x.start.cmp(&y.start).then(y.all_day.cmp(&x.all_day)).then(x.summary.cmp(&y.summary)));
        self.occ = occ;
        self.layout();
    }

    /// Group occurrences under day headings. A multi-day event appears on each of its
    /// days; today always gets a heading, events or not.
    fn layout(&mut self) {
        let keep = self.selected().map(|o| (o.uid.clone(), o.start));
        let today = Local::now().date_naive();
        let mut days: std::collections::BTreeMap<NaiveDate, Vec<usize>> = Default::default();
        days.entry(today).or_default();
        for (i, o) in self.occ.iter().enumerate() {
            let first = o.start.date_naive();
            // An all-day DTEND is exclusive; a timed event ending at midnight ends the day before.
            let last_instant = if o.end > o.start { o.end - Duration::seconds(1) } else { o.end };
            let last = last_instant.date_naive().max(first);
            let mut d = first.max(self.window.0);
            while d <= last && d <= self.window.1 {
                days.entry(d).or_default().push(i);
                d += Duration::days(1);
            }
        }
        self.rows.clear();
        for (d, evs) in days {
            self.rows.push(Row::Day(d));
            self.rows.extend(evs.into_iter().map(Row::Event));
        }
        self.cursor = keep
            .and_then(|(uid, start)| {
                self.rows.iter().position(|r| matches!(r, Row::Event(i) if self.occ[*i].uid == uid && self.occ[*i].start == start))
            })
            .unwrap_or_else(|| self.first_on_or_after(self.date));
        self.scroll_into_view();
    }

    pub fn selected(&self) -> Option<&Occurrence> {
        match self.rows.get(self.cursor) {
            Some(Row::Event(i)) => self.occ.get(*i),
            _ => None,
        }
    }

    /// The day the cursor is in.
    pub fn cursor_day(&self) -> NaiveDate {
        self.rows[..=self.cursor.min(self.rows.len().saturating_sub(1))]
            .iter()
            .rev()
            .find_map(|r| match r {
                Row::Day(d) => Some(*d),
                _ => None,
            })
            .unwrap_or(self.date)
    }

    fn first_on_or_after(&self, d: NaiveDate) -> usize {
        let day = self.rows.iter().position(|r| matches!(r, Row::Day(x) if *x >= d));
        match day {
            // Land on that day's first event, or on the heading of an empty day.
            Some(i) => match self.rows.get(i + 1) {
                Some(Row::Event(_)) => i + 1,
                _ => i,
            },
            None => self.rows.len().saturating_sub(1),
        }
    }

    fn scroll_into_view(&mut self) {
        let h = self.height.max(1);
        // Keep the day heading above the cursor visible when possible.
        let top = self.cursor.saturating_sub(1);
        if top < self.offset {
            self.offset = top;
        } else if self.cursor + 2 >= self.offset + h {
            self.offset = (self.cursor + 3).saturating_sub(h);
        }
    }

    fn step(&mut self, delta: isize) {
        let n = self.rows.len() as isize;
        let mut c = self.cursor as isize;
        let dir = delta.signum();
        let mut left = delta.abs();
        while left > 0 {
            let mut next = c + dir;
            // Skip headings, except an empty day's (it's the only thing to stand on).
            while next >= 0 && next < n {
                let empty_day = matches!(self.rows[next as usize], Row::Day(_))
                    && !matches!(self.rows.get(next as usize + 1), Some(Row::Event(_)));
                if matches!(self.rows[next as usize], Row::Event(_)) || empty_day {
                    break;
                }
                next += dir;
            }
            if next < 0 || next >= n {
                break;
            }
            c = next;
            left -= 1;
        }
        self.cursor = c as usize;
        self.detail_scroll = 0;
        self.scroll_into_view();
    }

    fn goto_day(&mut self, d: NaiveDate) {
        self.date = d;
        if d < self.window.0 + Duration::days(3) || d > self.window.1 - Duration::days(14) {
            self.expand();
        }
        self.cursor = self.first_on_or_after(d);
        self.detail_scroll = 0;
        self.scroll_into_view();
    }

    /// Rows of the sidebar: source headings and calendars.
    pub fn side_rows(&self) -> Vec<Option<usize>> {
        let mut rows = Vec::new();
        let mut last = None;
        for (i, c) in self.calendars.iter().enumerate() {
            if last != Some(&c.source) {
                rows.push(None);
                last = Some(&c.source);
            }
            rows.push(Some(i));
        }
        rows
    }
}

impl App {
    pub fn toggle_view(&mut self) {
        self.screen = match self.screen {
            View::Mail => View::Calendar,
            View::Calendar => View::Mail,
        };
        if self.screen == View::Calendar {
            self.focus = Focus::List;
            if self.cal.rows.is_empty() {
                self.cal.expand();
            }
            let stale = self.cal.synced_at.is_none_or(|t| t.elapsed().as_secs() > 300);
            if stale {
                self.sync_calendars();
            }
        }
    }

    pub fn sync_calendars(&mut self) {
        if self.cal.syncing {
            return;
        }
        if self.demo {
            self.cal.calendars = crate::demo::calendars();
            self.cal.synced_at = Some(Instant::now());
            self.cal.expand();
            return;
        }
        if self.cfg.dav_sources().is_empty() {
            self.say("no calendars — add a [[dav]] source to config.toml");
            return;
        }
        self.cal.syncing = true;
        let (cfg, tx) = (self.cfg.clone(), self.events_tx.clone());
        tokio::spawn(async move {
            let ev = match crate::dav::refresh_calendars(cfg).await {
                Ok((cals, errors)) => Event::Calendars(cals, errors),
                Err(e) => Event::Calendars(vec![], vec![format!("{e:#}")]),
            };
            let _ = tx.send(ev);
        });
    }

    pub fn on_calendars(&mut self, cals: Vec<CachedCalendar>, errors: Vec<String>) {
        self.cal.syncing = false;
        self.cal.synced_at = Some(Instant::now());
        if !cals.is_empty() || errors.is_empty() {
            let n: usize = cals.iter().map(|c| c.items.len()).sum();
            self.cal.calendars = cals;
            self.cal.expand();
            if errors.is_empty() {
                self.say(format!("{} calendars · {n} events synced", self.cal.calendars.len()));
            }
        }
        if !errors.is_empty() {
            self.say_error(format!("calendar: {}", errors.join("; ")));
        }
    }

    /// Keys in the calendar view. Returns false for keys it leaves to the mail keymap
    /// (quit, settings, help).
    pub fn calendar_key(&mut self, k: KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let g = std::mem::take(&mut self.cal.pending_g);
        let half = (self.cal.height / 2).max(1) as isize;
        match (self.focus, k.code) {
            (_, KeyCode::Char('C')) => self.toggle_view(),
            (_, KeyCode::Char('$')) => {
                self.say("syncing calendars…");
                self.sync_calendars();
            }
            (_, KeyCode::Char('r')) if ctrl => self.sync_calendars(),
            (_, KeyCode::Tab) => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::List,
                    Focus::List => Focus::Body,
                    Focus::Body => Focus::Sidebar,
                }
            }
            (_, KeyCode::BackTab) => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Body,
                    Focus::List => Focus::Sidebar,
                    Focus::Body => Focus::List,
                }
            }
            (_, KeyCode::Char('t')) => self.cal.goto_day(Local::now().date_naive()),
            (_, KeyCode::Char('/')) => {
                self.mode = crate::app::Mode::Prompt(crate::app::Prompt {
                    kind: crate::app::PromptKind::CalFilter,
                    input: self.cal.filter.clone(),
                    completions: vec![],
                    sel: None,
                })
            }
            (_, KeyCode::Esc) => {
                if !self.cal.filter.is_empty() {
                    self.cal.filter.clear();
                    self.cal.expand();
                }
            }
            (_, KeyCode::Char('o')) => {
                if let Some(u) = self.cal.selected().map(|o| o.url.clone()).filter(|u| !u.is_empty()) {
                    self.actions.push(Action::Open(u));
                }
            }

            // Sidebar: pick calendars to show.
            (Focus::Sidebar, KeyCode::Char('j') | KeyCode::Down) => {
                let n = self.cal.side_rows().len();
                self.cal.side_cursor = (self.cal.side_cursor + 1).min(n.saturating_sub(1));
            }
            (Focus::Sidebar, KeyCode::Char('k') | KeyCode::Up) => self.cal.side_cursor = self.cal.side_cursor.saturating_sub(1),
            (Focus::Sidebar, KeyCode::Char(' ') | KeyCode::Enter) => self.toggle_calendar(self.cal.side_cursor),
            (Focus::Sidebar, KeyCode::Char('l') | KeyCode::Right) => self.focus = Focus::List,

            // Detail pane scrolls.
            (Focus::Body, KeyCode::Char('j') | KeyCode::Down) => self.cal.detail_scroll += 1,
            (Focus::Body, KeyCode::Char('k') | KeyCode::Up) => self.cal.detail_scroll = self.cal.detail_scroll.saturating_sub(1),
            (Focus::Body, KeyCode::Char('h') | KeyCode::Left) => self.focus = Focus::List,

            // Agenda.
            (_, KeyCode::Char('j') | KeyCode::Down) => self.cal.step(1),
            (_, KeyCode::Char('k') | KeyCode::Up) => self.cal.step(-1),
            (_, KeyCode::Char('d')) if ctrl => self.cal.step(half),
            (_, KeyCode::Char('u')) if ctrl => self.cal.step(-half),
            (_, KeyCode::Char('l') | KeyCode::Right) => {
                let d = self.cal.cursor_day() + Duration::days(1);
                self.cal.goto_day(d)
            }
            (_, KeyCode::Char('h') | KeyCode::Left) => {
                let d = self.cal.cursor_day() - Duration::days(1);
                self.cal.goto_day(d)
            }
            (_, KeyCode::Char('L')) => {
                let d = self.cal.cursor_day() + Duration::days(7);
                self.cal.goto_day(d)
            }
            (_, KeyCode::Char('H')) => {
                let d = self.cal.cursor_day() - Duration::days(7);
                self.cal.goto_day(d)
            }
            (_, KeyCode::Char(']')) => {
                let d = self.cal.cursor_day();
                let next = d.with_day(1).unwrap().checked_add_months(chrono::Months::new(1)).unwrap_or(d);
                self.cal.goto_day(next)
            }
            (_, KeyCode::Char('[')) => {
                let d = self.cal.cursor_day();
                let prev = d.with_day(1).unwrap().checked_sub_months(chrono::Months::new(1)).unwrap_or(d);
                self.cal.goto_day(prev)
            }
            (_, KeyCode::Char('g')) if g => {
                self.cal.cursor = 0;
                self.cal.step(1);
                self.cal.step(-1);
            }
            (_, KeyCode::Char('g')) => self.cal.pending_g = true,
            (_, KeyCode::Char('G')) => {
                self.cal.cursor = self.cal.rows.len().saturating_sub(1);
                self.cal.scroll_into_view();
            }
            (_, KeyCode::Enter) => self.focus = Focus::Body,
            _ => return false,
        }
        true
    }

    pub fn cal_wheel(&mut self, at: ratatui::layout::Position, d: isize) {
        if self.hits.body.contains(at) {
            self.cal.detail_scroll = (self.cal.detail_scroll as isize + d).max(0) as usize;
        } else if self.hits.list.contains(at) {
            self.cal.step(d.signum());
        }
    }

    /// Click a calendar to show or hide it, an event to select it (twice to read it).
    pub fn cal_click(&mut self, at: ratatui::layout::Position, double: bool) {
        let h = self.hits.clone();
        if h.side.contains(at) && at.y > h.side.y {
            let row = (at.y - h.side.y - 1) as usize;
            if row < self.cal.side_rows().len() {
                self.focus = Focus::Sidebar;
                self.cal.side_cursor = row;
                self.toggle_calendar(row);
            }
        } else if h.list.contains(at) && at.y > h.list.y {
            let row = (at.y - h.list.y - 1) as usize + self.cal.offset;
            if matches!(self.cal.rows.get(row), Some(Row::Event(_))) {
                self.focus = if double { Focus::Body } else { Focus::List };
                self.cal.cursor = row;
                self.cal.detail_scroll = 0;
            }
        } else if h.body.contains(at) {
            self.focus = Focus::Body;
        }
    }

    pub fn toggle_calendar(&mut self, side_row: usize) {
        let Some(Some(i)) = self.cal.side_rows().get(side_row).copied() else { return };
        let c = &self.cal.calendars[i];
        let key = (c.source.clone(), c.id.clone());
        if !self.cal.hidden.remove(&key) {
            self.cal.hidden.insert(key);
        }
        self.cal.expand();
    }

    pub fn cal_filter(&mut self, q: &str) {
        self.cal.filter = q.trim().to_string();
        self.cal.expand();
    }
}

/// A calendar's color: the server's, else one from the theme by position.
pub fn color(c: &CachedCalendar, i: usize, t: &crate::theme::Theme) -> ratatui::style::Color {
    if let Some(col) = c.color.as_deref().and_then(crate::theme::parse_color) {
        return col;
    }
    [t.blue, t.magenta, t.green, t.yellow, t.cyan, t.red][i % 6]
}
