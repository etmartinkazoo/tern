//! The calendar view: calendars down the left, an agenda in the middle, the selected
//! event on the right. Read-only for now; everything comes from the DAV cache.

use crate::app::{Action, App, Focus};
use crate::calendar::{self, Occurrence};
use crate::dav::CachedCalendar;
use crate::model::Event;
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone};
use chrono_tz::Tz;
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
    /// "<source>/<id>" of the default calendar, from config.
    pub default: Option<String>,
    /// Whether the sidebar has focus (then its cursor picks where events go).
    pub side_focused: bool,
    /// Expansions per (source, calendar, item), with what they were computed for.
    #[allow(clippy::type_complexity)]
    expanded: std::collections::HashMap<(String, String, String), ((u64, NaiveDate, NaiveDate, &'static str), Vec<Occurrence>)>,
    /// Whether the calendar has been shown yet (it opens on today the first time).
    pub opened: bool,
    /// Last sync failure per source, for the settings view.
    pub source_errors: std::collections::HashMap<String, String>,
}

const BEFORE: i64 = 14;
const AFTER: i64 = 120;

fn day_start(d: NaiveDate) -> DateTime<Tz> {
    calendar::zone().from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap()).earliest().unwrap_or_else(calendar::now)
}

impl CalState {
    pub fn new() -> Self {
        let today = calendar::today();
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
            default: None,
            side_focused: false,
            opened: false,
            expanded: Default::default(),
            source_errors: Default::default(),
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
        let zone = calendar::zone().name();
        for (i, c) in self.calendars.iter().enumerate() {
            if !self.visible(i) {
                continue;
            }
            for (id, ics) in &c.items {
                // Expanding is the slow part; reuse it until the event, the window or
                // the zone changes.
                let key = (c.source.clone(), c.id.clone(), id.clone());
                let stamp = (fingerprint(ics), from, to, zone);
                match self.expanded.get(&key) {
                    Some((st, list)) if *st == stamp => {
                        occ.extend(list.iter().cloned().map(|mut o| {
                            o.calendar = i;
                            o
                        }));
                    }
                    _ => {
                        let list = calendar::expand(ics, id, i, a, b);
                        occ.extend(list.iter().cloned());
                        self.expanded.insert(key, (stamp, list));
                    }
                }
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
        let today = calendar::today();
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
        let kept = keep.and_then(|(uid, start)| {
            self.rows.iter().position(|r| matches!(r, Row::Event(i) if self.occ[*i].uid == uid && self.occ[*i].start == start))
        });
        match kept {
            Some(c) => {
                self.cursor = c;
                self.scroll_into_view();
            }
            // Nothing selected yet: start at the anchor day, at the top.
            None => {
                self.cursor = self.first_on_or_after(self.date);
                self.day_at_top();
            }
        }
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
        self.day_at_top();
    }

    /// Scroll so the cursor's day heading is the first row: the agenda reads from
    /// that day on, with earlier days one k away.
    fn day_at_top(&mut self) {
        let day = self.cursor_day();
        self.offset = self.rows.iter().position(|r| matches!(r, Row::Day(d) if *d >= day)).unwrap_or(0);
        self.scroll_into_view();
    }

    /// Re-anchor on today (on opening, and when the zone changes).
    pub fn today_at_top(&mut self) {
        self.date = calendar::today();
        self.expand();
        self.cursor = self.first_on_or_after(self.date);
        self.day_at_top();
    }

    /// Where new events go: the calendar under the sidebar cursor, else the selected
    /// event's, else the first visible one.
    pub fn target(&self) -> Option<usize> {
        if self.side_focused {
            if let Some(Some(i)) = self.side_rows().get(self.side_cursor) {
                return Some(*i);
            }
        }
        if let Some(d) = &self.default {
            if let Some(i) = self.calendars.iter().position(|c| format!("{}/{}", c.source, c.id) == *d) {
                return Some(i);
            }
        }
        if let Some(o) = self.selected() {
            return Some(o.calendar);
        }
        (0..self.calendars.len()).find(|&i| self.visible(i))
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
            if self.cal.rows.is_empty() || !self.cal.opened {
                self.cal.opened = true;
                self.cal.today_at_top();
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
        self.cal.source_errors = errors
            .iter()
            .filter_map(|e| e.split_once(": ").map(|(s, m)| (s.to_string(), m.to_string())))
            .collect();
        if !errors.is_empty() {
            self.say_error(format!("calendar: {}", errors.join("; ")));
        }
    }

    /// Keys in the calendar view. Returns false for keys it leaves to the mail keymap
    /// (quit, settings, help).
    pub fn calendar_key(&mut self, k: KeyEvent) -> bool {
        self.cal.side_focused = self.focus == Focus::Sidebar;
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
            (_, KeyCode::Char('t')) => self.cal.goto_day(calendar::today()),
            (_, KeyCode::Char('a')) => match self.cal.target() {
                Some(i) => {
                    let name = self.cal.calendars[i].name.clone();
                    self.mode = crate::app::Mode::Prompt(crate::app::Prompt {
                        kind: crate::app::PromptKind::QuickAdd,
                        input: String::new(),
                        completions: vec![],
                        sel: None,
                    });
                    self.say(format!("adding to {name}"));
                }
                None => self.say_error("no calendar to add to — sync first ($)"),
            },
            (_, KeyCode::Char('A')) => self.open_event_form(None),
            (_, KeyCode::Char('e')) => {
                if let Some(o) = self.cal.selected().cloned() {
                    self.open_event_form(Some(o));
                }
            }
            (_, KeyCode::Char('d')) if !ctrl => {
                if let Some(o) = self.cal.selected().cloned() {
                    let series = if o.recurring { " (the whole series)" } else { "" };
                    self.mode = crate::app::Mode::Prompt(crate::app::Prompt {
                        kind: crate::app::PromptKind::ConfirmDelete,
                        input: format!("delete “{}”{series}?", o.summary),
                        completions: vec![],
                        sel: None,
                    });
                }
            }
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

    fn open_event_form(&mut self, existing: Option<Occurrence>) {
        let today = calendar::today();
        let (draft, form, title) = match existing {
            Some(o) => {
                let ics = self.cal.calendars[o.calendar].items.get(&o.item).cloned().unwrap_or_default();
                let Some(d) = calendar::draft_of(&ics) else {
                    return self.say_error("couldn't read that event");
                };
                let title = if o.recurring { "Edit series" } else { "Edit event" };
                (d, crate::composer::EventForm { calendar: o.calendar, item: Some(o.item.clone()) }, title)
            }
            None => {
                let Some(cal) = self.cal.target() else {
                    return self.say_error("no calendar to add to — sync first ($)");
                };
                let d = crate::when::Draft {
                    title: String::new(),
                    date: if self.cal.rows.is_empty() { today } else { self.cal.cursor_day() },
                    start: None,
                    end: None,
                    days: 1,
                    location: String::new(),
                    description: String::new(),
                };
                (d, crate::composer::EventForm { calendar: cal, item: None }, "New event")
            }
        };
        let name = &self.cal.calendars[form.calendar].name;
        let title = format!("{title} · {name}");
        self.mode = crate::app::Mode::Compose(Box::new(crate::composer::Composer::new_event(&title, &draft, form)));
        self.focus = Focus::Body;
    }

    pub fn quick_add(&mut self, input: &str) {
        let today = calendar::today();
        let default = if self.cal.rows.is_empty() { today } else { self.cal.cursor_day() };
        let Some(d) = crate::when::parse_on(input, today, default) else {
            return self.say_error("couldn't find a title in that");
        };
        let Some(cal) = self.cal.target() else { return };
        self.write_event(cal, None, d);
    }

    pub fn save_event(&mut self, c: Box<crate::composer::Composer>) {
        let Some(form) = c.event.clone() else { return };
        match c.event_draft(calendar::today()) {
            Ok(d) => self.write_event(form.calendar, form.item, d),
            Err(e) => {
                self.say_error(e);
                self.mode = crate::app::Mode::Compose(c);
            }
        }
    }

    /// Create (item None) or update an event: show it at once, then write it to the
    /// server and resync, which either confirms it or puts back what the server has.
    fn write_event(&mut self, cal: usize, item: Option<String>, d: crate::when::Draft) {
        let c = &self.cal.calendars[cal];
        let (ics, id, etag) = match &item {
            Some(id) => {
                let orig = c.items.get(id).cloned().unwrap_or_default();
                match calendar::edit_ics(&orig, &d) {
                    Ok(ics) => (ics, id.clone(), c.etags.get(id).cloned()),
                    Err(e) => return self.say_error(format!("{e:#}")),
                }
            }
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                (calendar::new_ics(&d, &format!("{id}@tern")), format!("{id}.ics"), None)
            }
        };
        let when = match d.start {
            Some(s) => format!("{} {}", d.date.format("%a %e %b"), s.format("%H:%M")),
            None => d.date.format("%a %e %b").to_string(),
        };
        let verb = if item.is_some() { "saved" } else { "added" };
        let done = format!("{verb} “{}” · {when}", d.title);
        self.cal.calendars[cal].items.insert(id.clone(), ics.clone());
        self.cal.goto_day(d.date);
        self.cal.expand();
        if self.demo {
            return self.say(format!("{done} (demo: not saved)"));
        }
        self.say(format!("{done} · saving…"));
        let (source, cal_id) = (self.cal.calendars[cal].source.clone(), self.cal.calendars[cal].target());
        let (cfg, tx) = (self.cfg.clone(), self.events_tx.clone());
        tokio::spawn(async move {
            let r = crate::dav::put_event(cfg, source, cal_id, id, ics, etag).await;
            let _ = tx.send(Event::CalWrite(r.map(|_| done).map_err(|e| format!("{e:#}"))));
        });
    }

    pub fn delete_event(&mut self) {
        let Some(o) = self.cal.selected().cloned() else { return };
        let c = &self.cal.calendars[o.calendar];
        let (source, cal_id, etag) = (c.source.clone(), c.target(), c.etags.get(&o.item).cloned());
        self.cal.calendars[o.calendar].items.remove(&o.item);
        self.cal.expand();
        let done = format!("deleted “{}”", o.summary);
        if self.demo {
            return self.say(format!("{done} (demo)"));
        }
        let (cfg, tx) = (self.cfg.clone(), self.events_tx.clone());
        tokio::spawn(async move {
            let r = crate::dav::delete_event(cfg, source, cal_id, o.item, etag).await;
            let _ = tx.send(Event::CalWrite(r.map(|_| done).map_err(|e| format!("{e:#}"))));
        });
    }

    pub fn toggle_calendar(&mut self, side_row: usize) {
        let Some(Some(i)) = self.cal.side_rows().get(side_row).copied() else { return };
        self.toggle_calendar_index(i);
    }

    /// Show or hide a calendar, and remember it in config.toml.
    pub fn toggle_calendar_index(&mut self, i: usize) {
        let c = &self.cal.calendars[i];
        let key = (c.source.clone(), c.id.clone());
        let hide = !self.cal.hidden.remove(&key);
        if hide {
            self.cal.hidden.insert(key.clone());
        }
        self.cal.expand();
        let (src, id) = key;
        if self.cfg.dav.iter().any(|d| d.name == src) {
            self.edit_config(|t| crate::accounts::set_hidden(t, &src, &id, hide));
        }
    }

    /// Make a calendar the one new events and accepted invitations go to.
    pub fn set_default_calendar(&mut self, i: usize) {
        let c = &self.cal.calendars[i];
        let key = format!("{}/{}", c.source, c.id);
        let name = c.name.clone();
        self.cfg.ui.default_calendar = Some(key.clone());
        if self.edit_config(|t| crate::accounts::set_default_calendar(t, &key)) {
            self.say(format!("new events go to {name}"));
        }
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

fn fingerprint(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}
