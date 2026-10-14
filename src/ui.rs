//! Drawing. Dense by design: one line per message, no boxes, hairline separators, and
//! color doing the work borders usually do.

use crate::app::{App, Focus, Hits, Mode, PromptKind};
use crate::settings::{self, GeneralRow};
use crate::composer::{self, Composer, VMode};
use crate::model::Role;
use crate::calview::{self, Row as CalRow, View};
use crate::thread::RowKind;
use crate::theme::{mix, Theme};
use chrono::{Datelike, Local};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Truncate to `w` display columns, ending in an ellipsis when cut.
pub fn fit(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw > w - 1 {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    out
}

fn pad(s: &str, w: usize) -> String {
    let s = fit(s, w);
    let n = w.saturating_sub(s.width());
    format!("{s}{}", " ".repeat(n))
}

fn rpad(s: &str, w: usize) -> String {
    let s = fit(s, w);
    format!("{}{s}", " ".repeat(w.saturating_sub(s.width())))
}

struct Icons {
    on: bool,
}

impl Icons {
    fn folder(&self, r: Role) -> &'static str {
        if !self.on {
            return match r {
                Role::Inbox => "◆",
                _ => "·",
            };
        }
        match r {
            Role::Inbox => "\u{f01c}",
            Role::Sent => "\u{f1d8}",
            Role::Drafts => "\u{f040}",
            Role::Archive => "\u{f187}",
            Role::Trash => "\u{f1f8}",
            Role::Junk => "\u{f05e}",
            Role::All => "\u{f1c0}",
            Role::Flagged => "\u{f005}",
            Role::Other => "\u{f114}",
        }
    }
    fn flag(&self) -> &'static str {
        if self.on { "\u{f024}" } else { "★" }
    }
    fn clip(&self) -> &'static str {
        if self.on { "\u{f0c6}" } else { "+" }
    }
    fn replied(&self) -> &'static str {
        if self.on { "\u{f112}" } else { "↩" }
    }
}

pub fn short_date(d: Option<chrono::DateTime<Local>>, fmt: &str) -> String {
    let Some(d) = d else { return String::new() };
    let now = Local::now();
    if d.date_naive() == now.date_naive() {
        d.format("%H:%M").to_string()
    } else if (now - d).num_days() < 6 && d < now {
        d.format("%a").to_string()
    } else if d.year() == now.year() {
        d.format(fmt).to_string()
    } else {
        d.format("%Y-%m").to_string()
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let t = app.theme.clone();
    let area = f.area();
    let base = if app.cfg.ui.paint_background { Style::default().bg(t.bg).fg(t.fg) } else { Style::default().fg(t.fg) };
    f.render_widget(Block::default().style(base), area);

    let [main, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    app.hits = Hits::default();

    let account_form = matches!(&app.mode, Mode::Compose(c) if c.account.is_some());
    if matches!(app.mode, Mode::Settings(_)) || account_form {
        draw_settings(f, app, main, &t);
    } else if app.screen == View::Calendar {
        draw_calendar(f, app, main, &t);
    } else if app.zoom {
        app.hits.body = main;
        draw_body(f, app, main, &t);
    } else {
        let side_w = app.cfg.ui.sidebar_width.min(main.width / 4).max(12);
        let rest = main.width.saturating_sub(side_w + 2);
        let list_w = (rest as u32 * app.cfg.ui.list_percent.clamp(20, 80) as u32 / 100) as u16;
        let [side, s1, list, s2, body] = Layout::horizontal([
            Constraint::Length(side_w),
            Constraint::Length(1),
            Constraint::Length(list_w),
            Constraint::Length(1),
            Constraint::Min(10),
        ])
        .areas(main);
        app.hits.side = side;
        app.hits.list = list;
        app.hits.body = body;
        draw_sidebar(f, app, side, &t);
        draw_sep(f, s1, &t);
        draw_list(f, app, list, &t);
        draw_sep(f, s2, &t);
        draw_body(f, app, body, &t);
    }
    app.hits.status = status;
    draw_status(f, app, status, &t);

    match &app.mode {
        Mode::Prompt(p) if !p.completions.is_empty() => draw_completions(f, app, main, &t),
        _ => {}
    }
}

fn draw_sep(f: &mut Frame, r: Rect, t: &Theme) {
    let lines: Vec<Line> = (0..r.height).map(|_| Line::from("│")).collect();
    f.render_widget(Paragraph::new(lines).style(Style::default().fg(mix(t.subtle, t.bg, 0.35))), r);
}

fn pane_title<'a>(text: &str, focused: bool, t: &Theme) -> Span<'a> {
    let st = if focused {
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.muted).add_modifier(Modifier::BOLD)
    };
    Span::styled(format!(" {text}"), st)
}

fn draw_sidebar(f: &mut Frame, app: &mut App, r: Rect, t: &Theme) {
    let icons = Icons { on: app.cfg.ui.icons };
    let focused = app.focus == Focus::Sidebar;
    let w = r.width as usize;
    let mut lines = vec![Line::from(vec![
        pane_title("tern", focused, t),
        Span::styled(
            format!(" {}", fit(&app.accts.first().map(|a| a.cfg.email.clone()).unwrap_or_default(), w.saturating_sub(6))),
            Style::default().fg(t.subtle),
        ),
    ])];
    let rows = app.side_rows();
    let height = r.height.saturating_sub(1) as usize;
    let offset = app.side_cursor.saturating_sub(height.saturating_sub(1));
    app.hits.side_offset = offset;
    for (i, row) in rows.iter().enumerate().skip(offset).take(height) {
        let a = &app.accts[row.acct];
        let cursor = focused && i == app.side_cursor;
        let Some(fi) = row.folder else {
            let folded = app.collapsed.contains(&a.cfg.name);
            let arrow = if folded { "▸" } else { "▾" };
            let dot = if a.connected { "" } else { " ○" };
            let unread: u32 = if folded { a.folders.iter().filter(|f| f.role == Role::Inbox).map(|f| f.unread).sum() } else { 0 };
            let count = if unread > 0 { unread.to_string() } else { String::new() };
            let label = format!(" {arrow} {}{dot}", a.cfg.name.to_uppercase());
            let mut line = Line::from(vec![
                Span::styled(pad(&label, w.saturating_sub(count.width() + 1)), Style::default().fg(t.muted).add_modifier(Modifier::BOLD)),
                Span::styled(rpad(&count, count.width() + 1), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
            ]);
            if cursor {
                line = line.style(Style::default().bg(t.sel_bg));
            }
            lines.push(line);
            continue;
        };
        let fo = &a.folders[fi];
        let open = app.current.as_ref() == Some(&(row.acct, fo.name.clone()));
        let indent = "  ".repeat(fo.depth.min(4));
        let count = if fo.unread > 0 { fo.unread.to_string() } else { String::new() };
        let label_w = w.saturating_sub(indent.width() + 5 + count.width() + 1);
        let bar = if open { "▎" } else { " " };
        let mut fg = if open { t.fg } else { t.muted };
        if !fo.selectable {
            fg = t.subtle;
        }
        let mut st = Style::default().fg(fg);
        if open {
            st = st.add_modifier(Modifier::BOLD);
        }
        let bg = if cursor { Some(t.sel_bg) } else { None };
        let mut spans = vec![
            Span::styled(bar, Style::default().fg(t.accent)),
            Span::styled(format!("{indent}{} ", icons.folder(fo.role)), Style::default().fg(if open { t.accent } else { t.subtle })),
            Span::styled(pad(&fo.label, label_w + 1), st),
            Span::styled(rpad(&count, count.width() + 1), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
        ];
        let used: usize = spans.iter().map(|s| s.content.width()).sum();
        spans.push(Span::raw(" ".repeat(w.saturating_sub(used))));
        let mut line = Line::from(spans);
        if let Some(bg) = bg {
            line = line.style(Style::default().bg(bg));
        }
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), r);
}

fn draw_list(f: &mut Frame, app: &mut App, r: Rect, t: &Theme) {
    let icons = Icons { on: app.cfg.ui.icons };
    let focused = app.focus == Focus::List;
    let w = r.width as usize;
    app.list_height = r.height.saturating_sub(1) as usize;

    let folder = app.current_folder().map(|f| f.label.clone()).unwrap_or_else(|| {
        app.current.as_ref().map(|c| c.1.clone()).unwrap_or_default()
    });
    let mut head = vec![pane_title(&folder, focused, t)];
    let unread = app.current_folder().map(|f| f.unread).unwrap_or(0);
    let mut meta = format!("  {}", app.total);
    if app.threaded && !app.threads.is_empty() {
        meta.push_str(&format!(" · {} threads", app.threads.len()));
    }
    if unread > 0 {
        meta.push_str(&format!(" · {unread} unread"));
    }
    head.push(Span::styled(meta, Style::default().fg(t.subtle)));
    if !app.filter.is_empty() {
        head.push(Span::styled(format!("  /{} ({})", app.filter, app.view.len()), Style::default().fg(t.yellow)));
    }
    if !app.marked.is_empty() {
        head.push(Span::styled(format!("  {} marked", app.marked.len()), Style::default().fg(t.magenta)));
    }
    let mut lines = vec![Line::from(head)];

    if app.view.is_empty() {
        let msg = if app.loading {
            "loading…"
        } else if !app.filter.is_empty() {
            "no matches"
        } else {
            "nothing here"
        };
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(format!("  {msg}"), Style::default().fg(t.subtle))));
        f.render_widget(Paragraph::new(lines), r);
        return;
    }

    let date_w = 6;
    let from_w = (w / 4).clamp(8, 22);
    let subj_w = w.saturating_sub(4 + from_w + 1 + 1 + date_w + 1);
    let fmt = app.cfg.ui.date_format.clone();
    // In Sent and Drafts the interesting name is who it went to.
    let outgoing = app.current_folder().is_some_and(|f| matches!(f.role, Role::Sent | Role::Drafts));
    let rows = app.list_height;
    for (row, rw) in app.rows.iter().enumerate().skip(app.list_offset).take(rows) {
        let e = &app.all[rw.msg];
        let th = (rw.thread != usize::MAX).then(|| &app.threads[rw.thread]);
        let folded = rw.kind == RowKind::Collapsed;
        let sel = row == app.cursor;
        let uids: Vec<u32> = match (folded, th) {
            (true, Some(t)) => t.members.iter().map(|&i| app.all[i].uid).collect(),
            _ => vec![e.uid],
        };
        let marked = uids.iter().any(|u| app.marked.contains(u));
        // A folded conversation speaks for all of its messages.
        let (unread, flagged, attach) = match (folded, th) {
            (true, Some(t)) => (t.unread > 0, t.flagged, t.attachment),
            _ => (!e.seen, e.flagged, e.has_attachment),
        };

        let (state, state_c) = if unread {
            ("●", t.accent)
        } else if e.answered {
            (icons.replied(), t.subtle)
        } else {
            (" ", t.subtle)
        };
        let (flag, flag_c) = if flagged {
            (icons.flag(), t.yellow)
        } else if attach {
            (icons.clip(), t.subtle)
        } else {
            (" ", t.subtle)
        };
        let base_fg = if unread { t.fg } else { t.muted };
        let mut from_st = Style::default().fg(base_fg);
        let mut subj_st = Style::default().fg(if unread { t.fg } else { t.muted });
        if unread {
            from_st = from_st.add_modifier(Modifier::BOLD);
            subj_st = subj_st.add_modifier(Modifier::BOLD);
        }
        let who = match (folded, th) {
            (true, Some(t)) => t.participants.clone(),
            _ if outgoing => format!("→ {}", e.recipient()),
            _ => e.sender().to_string(),
        };
        // Subject column: count badge for folded threads, tree glyphs for unfolded ones.
        let mut subj_spans: Vec<Span> = Vec::new();
        let mut used = 0;
        match (rw.kind, th) {
            (RowKind::Collapsed, Some(t)) => {
                let badge = format!("{} ", t.members.len());
                used += badge.width();
                subj_spans.push(Span::styled(badge, Style::default().fg(if unread { app.theme.accent } else { app.theme.subtle }).add_modifier(Modifier::BOLD)));
                let root = &app.all[t.members[0]];
                subj_spans.push(Span::styled(pad(&root.subject, subj_w.saturating_sub(used)), subj_st));
            }
            (RowKind::Head, Some(_)) => {
                subj_spans.push(Span::styled("▾ ", Style::default().fg(app.theme.subtle)));
                subj_spans.push(Span::styled(pad(&e.subject, subj_w.saturating_sub(2)), subj_st));
            }
            (RowKind::Child, Some(t)) => {
                let tree = format!("{} ", t.trees[rw.member]);
                used += tree.width();
                subj_spans.push(Span::styled(tree, Style::default().fg(mix(app.theme.subtle, app.theme.bg, 0.2))));
                // Repeating "Re: same subject" down a tree is noise; show it only if it changed.
                let root = &app.all[t.members[0]];
                let same = crate::thread::normalize_subject(&e.subject).0 == crate::thread::normalize_subject(&root.subject).0;
                let text = if same { String::new() } else { e.subject.clone() };
                subj_spans.push(Span::styled(pad(&text, subj_w.saturating_sub(used)), subj_st));
            }
            _ => subj_spans.push(Span::styled(pad(&e.subject, subj_w), subj_st)),
        }
        let mark = if marked { "▌" } else { " " };
        let mut spans = vec![
            Span::styled(mark, Style::default().fg(t.magenta)),
            Span::styled(state, Style::default().fg(state_c)),
            Span::styled(flag, Style::default().fg(flag_c)),
            Span::raw(" "),
            Span::styled(pad(&who, from_w), from_st),
            Span::raw(" "),
        ];
        spans.extend(subj_spans);
        spans.push(Span::raw(" "));
        spans.push(Span::styled(rpad(&short_date(e.date, &fmt), date_w), Style::default().fg(if unread { t.accent } else { t.subtle })));
        let mut line = Line::from(spans);
        if sel {
            let bg = if focused { t.sel_bg } else { mix(t.sel_bg, t.bg, 0.5) };
            line = line.style(Style::default().bg(bg));
        } else if marked {
            line = line.style(Style::default().bg(mix(t.bg, t.magenta, 0.10)));
        }
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), r);
}

#[derive(Clone, Copy)]
enum Kind {
    Text,
    Quote(usize),
    Sig,
    Attribution,
}

fn classify(text: &str) -> Vec<(String, Kind)> {
    let mut out = Vec::new();
    let mut sig = false;
    for line in text.lines() {
        if line == "-- " || line == "--" {
            sig = true;
        }
        let depth = line.chars().take_while(|c| *c == '>' || *c == ' ').filter(|c| *c == '>').count();
        let kind = if depth > 0 {
            Kind::Quote(depth)
        } else if sig {
            Kind::Sig
        } else if line.starts_with("On ") && line.trim_end().ends_with("wrote:") {
            Kind::Attribution
        } else {
            Kind::Text
        };
        out.push((line.to_string(), kind));
    }
    out
}

fn styled_line<'a>(text: &str, kind: Kind, t: &Theme) -> Line<'a> {
    let base = match kind {
        Kind::Text => Style::default().fg(t.fg),
        Kind::Quote(d) => Style::default().fg(t.quote[(d - 1) % 4]),
        Kind::Sig | Kind::Attribution => Style::default().fg(t.subtle),
    };
    // Underline links so they read as links even in a dense wall of text.
    let mut spans = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("http://").or_else(|| rest.find("https://")) {
        let i = match (rest.find("http://"), rest.find("https://")) {
            (Some(a), Some(b)) => a.min(b),
            _ => i,
        };
        if i > 0 {
            spans.push(Span::styled(rest[..i].to_string(), base));
        }
        let end = rest[i..].find(|c: char| c.is_whitespace() || c == '>' || c == ')' || c == ']').map(|e| i + e).unwrap_or(rest.len());
        spans.push(Span::styled(
            rest[i..end].to_string(),
            Style::default().fg(t.cyan).add_modifier(Modifier::UNDERLINED),
        ));
        rest = &rest[end..];
    }
    if !rest.is_empty() {
        spans.push(Span::styled(rest.to_string(), base));
    }
    Line::from(spans)
}

fn wrap(lines: &[(String, Kind)], width: usize) -> Vec<(String, Kind)> {
    let width = width.max(10);
    let mut out = Vec::new();
    for (l, k) in lines {
        if l.width() <= width {
            out.push((l.clone(), *k));
            continue;
        }
        let prefix: String = match k {
            Kind::Quote(_) => l.chars().take_while(|c| *c == '>' || *c == ' ').collect(),
            _ => String::new(),
        };
        let opts = textwrap::Options::new(width)
            .subsequent_indent(&prefix)
            .break_words(true)
            .word_separator(textwrap::WordSeparator::AsciiSpace);
        for w in textwrap::wrap(l, opts) {
            out.push((w.into_owned(), *k));
        }
    }
    out
}

fn header_line<'a>(label: &str, value: &str, w: usize, t: &Theme) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!(" {label:<8}"), Style::default().fg(t.subtle)),
        Span::styled(fit(value, w.saturating_sub(10)), Style::default().fg(t.muted)),
    ])
}

fn human_size(n: usize) -> String {
    match n {
        n if n >= 1 << 20 => format!("{:.1}M", n as f64 / (1 << 20) as f64),
        n if n >= 1 << 10 => format!("{}K", n >> 10),
        n => format!("{n}B"),
    }
}

fn draw_body(f: &mut Frame, app: &mut App, r: Rect, t: &Theme) {
    let from = app.accts[app.current.as_ref().map(|c| c.0).unwrap_or(0)].cfg.sender_mailbox();
    let active = matches!(app.mode, Mode::Compose(_));
    if let Some(c) = app.composer_mut() {
        return draw_composer(f, c, active, &from, r, t);
    }
    if app.conv_thread().is_some() {
        return draw_conversation(f, app, r, t);
    }
    let focused = app.focus == Focus::Body;
    let w = r.width as usize;
    let icons = Icons { on: app.cfg.ui.icons };
    // A meeting invitation gets a card, checked against the synced calendars.
    let invite = app.body.as_ref().and_then(|b| b.invite.as_deref()).and_then(crate::calendar::invite);
    if invite.is_some() && app.cal.occ.is_empty() && !app.cal.calendars.is_empty() {
        app.cal.expand();
    }
    let conflicts: Vec<String> = invite
        .as_ref()
        .map(|inv| {
            app.cal
                .occ
                .iter()
                .filter(|o| o.uid != inv.uid && !o.all_day && !inv.all_day && o.start < inv.end && inv.start < o.end)
                .map(|o| format!("{} {}–{}", o.summary, o.start.format("%H:%M"), o.end.format("%H:%M")))
                .collect()
        })
        .unwrap_or_default();
    let me = app.current.as_ref().map(|c| app.accts[c.0].cfg.email.to_lowercase()).unwrap_or_default();
    // Your answer lives in your calendar's copy once you've given one.
    let answered = invite.as_ref().and_then(|inv| {
        let needle = format!("UID:{}", inv.uid);
        app.cal
            .calendars
            .iter()
            .flat_map(|c| c.items.values())
            .find(|ics| ics.contains(&needle))
            .and_then(|ics| crate::calendar::invite(ics))
            .and_then(|copy| copy.attendees.into_iter().find(|a| a.0 == me).map(|a| a.2))
    });
    let Some(b) = &app.body else {
        let mut lines = vec![Line::from(pane_title("", focused, t))];
        let msg = if app.selected().is_some() { "loading…" } else { "" };
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(format!("  {msg}"), Style::default().fg(t.subtle))));
        f.render_widget(Paragraph::new(lines), r);
        return;
    };

    let mut head: Vec<Line> = Vec::new();
    let subj = if b.subject.is_empty() { "(no subject)" } else { &b.subject };
    let st = Style::default().fg(if focused { t.accent } else { t.fg }).add_modifier(Modifier::BOLD);
    for (i, l) in textwrap::wrap(subj, w.saturating_sub(2).max(10)).into_iter().take(2).enumerate() {
        let flag = if i == 0 && app.selected().is_some_and(|e| e.flagged) {
            Span::styled(format!(" {}", icons.flag()), Style::default().fg(t.yellow))
        } else {
            Span::raw("")
        };
        head.push(Line::from(vec![Span::styled(format!(" {l}"), st), flag]));
    }
    head.push(header_line("From", &b.from, w, t));
    head.push(header_line("To", &b.to, w, t));
    if !b.cc.is_empty() {
        head.push(header_line("Cc", &b.cc, w, t));
    }
    let date = if b.date.is_empty() {
        app.selected().and_then(|e| e.date).map(|d| d.format("%a %e %b %Y %H:%M").to_string()).unwrap_or_default()
    } else {
        b.date.clone()
    };
    head.push(header_line("Date", &date, w, t));
    if !b.attachments.is_empty() {
        let list = b
            .attachments
            .iter()
            .map(|a| format!("{} {}", a.name, human_size(a.size)))
            .collect::<Vec<_>>()
            .join("  ");
        head.push(Line::from(vec![
            Span::styled(format!(" {:<8}", icons.clip()), Style::default().fg(t.subtle)),
            Span::styled(fit(&list, w.saturating_sub(10)), Style::default().fg(t.yellow)),
        ]));
    }

    if let Some(inv) = &invite {
        head.extend(invite_card(inv, &conflicts, &me, answered.as_deref(), w, t));
    }
    let text_w = w.saturating_sub(2);
    // Plain text by default; `H` swaps in the HTML part rendered with its styling.
    let as_html = app.html_view.contains(&b.uid) && b.html.is_some();
    let body_lines: Vec<Line<'static>> = if as_html {
        let key = (b.uid, text_w as u16, t.accent);
        match &app.html_cache {
            Some((k, lines)) if *k == key => lines.clone(),
            _ => {
                let lines = html_lines(b.html.as_deref().unwrap_or(""), text_w, t);
                app.html_cache = Some((key, lines.clone()));
                lines
            }
        }
    } else {
        wrap(&classify(&b.text), text_w).iter().map(|(s, k)| styled_line(s, *k, t)).collect()
    };
    let avail = (r.height as usize).saturating_sub(head.len() + 1);
    app.body_height = avail + 2;
    app.body_lines = body_lines.len();
    let max = body_lines.len().saturating_sub(avail);
    app.body_scroll = app.body_scroll.min(max);

    let pct = if body_lines.len() <= avail {
        "all".to_string()
    } else {
        format!("{}%", (app.body_scroll + avail).min(body_lines.len()) * 100 / body_lines.len().max(1))
    };
    let tag = match (as_html, b.html.is_some()) {
        (true, _) => " html",
        (false, true) => " text · H html",
        _ => "",
    };
    let rule_w = w.saturating_sub(pct.width() + tag.width() + 4);
    head.push(Line::from(vec![
        Span::styled(format!(" {}", "─".repeat(rule_w)), Style::default().fg(mix(t.subtle, t.bg, 0.4))),
        Span::styled(tag.to_string(), Style::default().fg(if as_html { t.magenta } else { t.subtle })),
        Span::styled(format!(" {pct}"), Style::default().fg(t.subtle)),
    ]));
    let head_h = head.len() as u16;
    f.render_widget(Paragraph::new(head), Rect { height: head_h.min(r.height), ..r });

    let body_area = Rect { y: r.y + head_h, height: r.height.saturating_sub(head_h), x: r.x + 1, width: r.width.saturating_sub(1) };
    let lines: Vec<Line> = body_lines.into_iter().skip(app.body_scroll).take(avail).collect();
    f.render_widget(Paragraph::new(lines), body_area);
}

fn invite_card(inv: &crate::calendar::Invite, conflicts: &[String], me: &str, answered: Option<&str>, w: usize, t: &Theme) -> Vec<Line<'static>> {
    let bar = |c| Span::styled(" ▌ ", Style::default().fg(c));
    let (accent, title) = match inv.method.as_str() {
        "CANCEL" => (t.red, "Cancelled"),
        "REPLY" => (t.green, "Reply"),
        _ => (t.accent, "Invitation"),
    };
    let mut out = vec![Line::from("")];
    out.push(Line::from(vec![
        bar(accent),
        Span::styled(format!("{title} · "), Style::default().fg(accent).add_modifier(Modifier::BOLD)),
        Span::styled(fit(&inv.summary, w.saturating_sub(18)), Style::default().fg(t.fg).add_modifier(Modifier::BOLD)),
    ]));
    let when = if inv.all_day {
        format!("{} · all day", inv.start.format("%a %e %b %Y"))
    } else {
        format!("{} · {}–{}", inv.start.format("%a %e %b %Y"), inv.start.format("%H:%M"), inv.end.format("%H:%M"))
    };
    let rec = if inv.recurring { "  ↻ repeats" } else { "" };
    out.push(Line::from(vec![bar(accent), Span::styled(when, Style::default().fg(t.fg)), Span::styled(rec, Style::default().fg(t.subtle))]));
    if !inv.location.is_empty() {
        let mut l = styled_line(&fit(&inv.location, w.saturating_sub(6)), Kind::Text, t);
        l.spans.insert(0, bar(accent));
        out.push(l);
    }
    let count = |s: &str| inv.attendees.iter().filter(|a| a.2 == s).count();
    let org = if inv.organizer_name.is_empty() { inv.organizer.clone() } else { inv.organizer_name.clone() };
    out.push(Line::from(vec![
        bar(accent),
        Span::styled(format!("from {org} · {} guests", inv.attendees.len()), Style::default().fg(t.muted)),
        Span::styled(
            format!("  {} yes · {} maybe · {} no · {} waiting", count("ACCEPTED"), count("TENTATIVE"), count("DECLINED"), count("NEEDS-ACTION")),
            Style::default().fg(t.subtle),
        ),
    ]));
    if inv.method == "REQUEST" {
        let mine = answered.or_else(|| inv.attendees.iter().find(|a| a.0 == me).map(|a| a.2.as_str())).unwrap_or("NEEDS-ACTION");
        let (ans, c) = match mine {
            "ACCEPTED" => ("you said yes", t.green),
            "TENTATIVE" => ("you said maybe", t.yellow),
            "DECLINED" => ("you said no", t.red),
            _ => ("waiting for your answer", t.yellow),
        };
        let key = |k: &str, label: &str, col| {
            vec![
                Span::styled(format!(" {k} "), Style::default().fg(t.bg).bg(col).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" {label}  "), Style::default().fg(t.muted)),
            ]
        };
        let mut spans = vec![bar(accent), Span::styled(format!("{ans}   "), Style::default().fg(c))];
        spans.extend(key("a", "yes", t.green));
        spans.extend(key("t", "maybe", t.yellow));
        spans.extend(key("n", "no", t.red));
        out.push(Line::from(spans));
    }
    if !conflicts.is_empty() {
        out.push(Line::from(vec![
            bar(accent),
            Span::styled("overlaps ", Style::default().fg(t.red).add_modifier(Modifier::BOLD)),
            Span::styled(fit(&conflicts.join(" · "), w.saturating_sub(14)), Style::default().fg(t.red)),
        ]));
    } else if inv.method == "REQUEST" {
        out.push(Line::from(vec![bar(accent), Span::styled("no conflicts", Style::default().fg(t.subtle))]));
    }
    out
}

/// HTML laid out for the terminal: headings and bold, italics, links, code, lists and
/// tables keep their shape. The email's own colors are left out on purpose: they are
/// chosen for white backgrounds and turn unreadable on a dark theme.
fn html_lines(html: &str, width: usize, t: &Theme) -> Vec<Line<'static>> {
    use html2text::render::text_renderer::RichAnnotation as A;
    let width = width.max(20);
    // html2text panics on some malformed input; fall back to plain conversion.
    let rich = std::panic::catch_unwind(|| html2text::from_read_rich(html.as_bytes(), width));
    let Ok(rich) = rich else {
        return crate::parse::html_to_text(html).lines().map(|l| Line::from(l.to_string())).collect();
    };
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut blank = 0;
    for line in rich {
        let spans: Vec<Span<'static>> = line
            .tagged_strings()
            .map(|ts| {
                let mut st = Style::default().fg(t.fg);
                for a in &ts.tag {
                    st = match a {
                        A::Link(_) => st.fg(t.cyan).add_modifier(Modifier::UNDERLINED),
                        A::Image(_) => st.fg(t.magenta).add_modifier(Modifier::ITALIC),
                        A::Emphasis => st.add_modifier(Modifier::ITALIC),
                        A::Strong => st.add_modifier(Modifier::BOLD),
                        A::Strikeout => st.add_modifier(Modifier::CROSSED_OUT),
                        A::Code | A::Preformat(_) => st.fg(t.yellow),
                        _ => st,
                    };
                }
                Span::styled(ts.s.replace('\u{a0}', " ").replace('\u{336}', ""), st)
            })
            .collect();
        // Marketing mail stacks empty spacer rows; keep at most one.
        let empty = spans.iter().all(|s| s.content.trim().is_empty());
        blank = if empty { blank + 1 } else { 0 };
        if blank > 1 {
            continue;
        }
        out.push(Line::from(spans));
    }
    while out.first().is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty())) {
        out.remove(0);
    }
    out
}

/// A folded conversation, oldest first: unread messages and the latest open, the rest one
/// line each, and quoted tails hidden since the earlier messages are right above.
fn draw_conversation(f: &mut Frame, app: &mut App, r: Rect, t: &Theme) {
    let Some(th) = app.conv_thread().cloned() else { return };
    let focused = app.focus == Focus::Body;
    let w = r.width as usize;
    let icons = Icons { on: app.cfg.ui.icons };
    let fmt = app.cfg.ui.date_format.clone();
    let me = app.current.as_ref().map(|c| app.accts[c.0].cfg.email.clone()).unwrap_or_default();

    let mut head: Vec<Line> = Vec::new();
    let subject = app.all[th.members[0]].subject.clone();
    let st = Style::default().fg(if focused { t.accent } else { t.fg }).add_modifier(Modifier::BOLD);
    for l in textwrap::wrap(&subject, w.saturating_sub(2).max(10)).into_iter().take(2) {
        head.push(Line::from(Span::styled(format!(" {l}"), st)));
    }
    head.push(Line::from(vec![
        Span::styled(format!(" {} messages", th.members.len()), Style::default().fg(t.muted)),
        Span::styled(format!(" · {}", th.participants), Style::default().fg(t.subtle)),
    ]));

    let text_w = w.saturating_sub(3);
    let rule = Style::default().fg(mix(t.subtle, t.bg, 0.55));
    let mut body: Vec<Line> = Vec::new();
    let mut anchor = None;
    for (k, &i) in th.chrono.iter().enumerate() {
        let e = app.all[i].clone();
        let open = app.conv_open.contains(&e.uid);
        let cached = app.cached(e.uid).cloned();
        if k > 0 {
            body.push(Line::from(Span::styled(format!(" {}", "╌".repeat(w.saturating_sub(2))), rule)));
        }
        if open && anchor.is_none() {
            anchor = Some(body.len());
        }
        let mine = !me.is_empty() && e.from_addr.eq_ignore_ascii_case(&me);
        let name = if mine { "me".to_string() } else { e.sender().to_string() };
        let date = short_date(e.date, &fmt);
        let dot = if !e.seen { Span::styled("● ", Style::default().fg(t.accent)) } else { Span::raw("  ") };
        let name_st = if open {
            Style::default().fg(if mine { t.green } else { t.fg }).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.muted)
        };
        let name_w = name.width().min(24);
        let room = w.saturating_sub(3 + name_w + 2 + date.width() + 1);
        let snippet = if open {
            String::new()
        } else {
            cached
                .as_ref()
                .map(|b| {
                    // Run the opening lines together so a bare "Hi Ted," doesn't fill the preview.
                    b.text
                        .lines()
                        .map(str::trim)
                        .take_while(|l| !l.starts_with('>') && !l.ends_with("wrote:") && *l != "--")
                        .filter(|l| !l.is_empty())
                        .take(4)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default()
        };
        body.push(Line::from(vec![
            Span::raw(" "),
            dot,
            Span::styled(fit(&name, name_w), name_st),
            Span::raw("  "),
            Span::styled(pad(&snippet, room), Style::default().fg(t.subtle)),
            Span::styled(rpad(&date, date.width() + 1), Style::default().fg(t.subtle)),
        ]));
        if !open {
            continue;
        }
        match cached {
            Some(b) => {
                if !b.attachments.is_empty() {
                    let list = b.attachments.iter().map(|a| a.name.clone()).collect::<Vec<_>>().join("  ");
                    body.push(Line::from(vec![
                        Span::styled(format!("   {} ", icons.clip()), Style::default().fg(t.subtle)),
                        Span::styled(fit(&list, w.saturating_sub(6)), Style::default().fg(t.yellow)),
                    ]));
                }
                body.push(Line::from(""));
                let (text, hidden) = crate::thread::strip_quoted(&b.text);
                for (s, k) in wrap(&classify(&text), text_w) {
                    let mut l = styled_line(&s, k, t);
                    l.spans.insert(0, Span::raw("   "));
                    body.push(l);
                }
                if hidden {
                    body.push(Line::from(Span::styled("   ··· quoted text hidden", Style::default().fg(t.subtle))));
                }
            }
            None => body.push(Line::from(Span::styled("   loading…", Style::default().fg(t.subtle)))),
        }
    }

    let avail = (r.height as usize).saturating_sub(head.len() + 1);
    app.body_height = avail + 2;
    app.body_lines = body.len();
    if app.conv_jump && th.chrono.iter().all(|&i| app.cached(app.all[i].uid).is_some()) {
        app.body_scroll = anchor.unwrap_or(0);
        app.conv_jump = false;
    }
    let max = body.len().saturating_sub(avail);
    app.body_scroll = app.body_scroll.min(max);
    let pct = if body.len() <= avail {
        "all".to_string()
    } else {
        format!("{}%", (app.body_scroll + avail).min(body.len()) * 100 / body.len().max(1))
    };
    head.push(Line::from(vec![
        Span::styled(format!(" {}", "─".repeat(w.saturating_sub(pct.width() + 3))), Style::default().fg(mix(t.subtle, t.bg, 0.4))),
        Span::styled(format!(" {pct}"), Style::default().fg(t.subtle)),
    ]));
    let head_h = head.len() as u16;
    f.render_widget(Paragraph::new(head), Rect { height: head_h.min(r.height), ..r });
    let area = Rect { y: r.y + head_h, height: r.height.saturating_sub(head_h), ..r };
    let lines: Vec<Line> = body.into_iter().skip(app.body_scroll).take(avail).collect();
    f.render_widget(Paragraph::new(lines), area);
}

/// The composer body, soft-wrapped to the pane. Wrapping is display only; the text keeps
/// one line per paragraph.
fn draw_soft_wrapped(f: &mut Frame, c: &mut Composer, area: Rect, cur_st: Style, show_cursor: bool, t: &Theme) {
    let width = area.width.max(1) as usize;
    let lines = c.body.lines().to_vec();
    let (crow, ccol) = c.body.cursor();
    let sel = c.body.selection_range();
    let in_sel = |r: usize, col: usize| match sel {
        Some(((r1, c1), (r2, c2))) => (r, col) >= (r1, c1) && (r, col) < (r2, c2),
        None => false,
    };
    let kinds = classify(&lines.join("\n"));
    // Lay out every row first so we know where the cursor lands.
    let mut rows: Vec<(usize, usize, usize)> = Vec::new(); // (line, start, end)
    let mut cursor_row = 0;
    for (li, line) in lines.iter().enumerate() {
        let segs = composer::layout(line, width);
        let nseg = segs.len();
        for (si, (a, b)) in segs.into_iter().enumerate() {
            if li == crow && ccol >= a && (ccol < b || si + 1 == nseg) {
                cursor_row = rows.len();
            }
            rows.push((li, a, b));
        }
    }
    let h = area.height as usize;
    if cursor_row < c.scroll {
        c.scroll = cursor_row;
    } else if h > 0 && cursor_row >= c.scroll + h {
        c.scroll = cursor_row + 1 - h;
    }
    let base = Style::default().fg(t.fg);
    let mut out: Vec<Line> = Vec::new();
    for &(li, a, b) in rows.iter().skip(c.scroll).take(h) {
        let line_st = match kinds.get(li).map(|k| k.1) {
            Some(Kind::Quote(d)) => Style::default().fg(t.quote[(d - 1) % 4]),
            Some(Kind::Sig) | Some(Kind::Attribution) => Style::default().fg(t.subtle),
            _ => base,
        };
        let chars: Vec<char> = lines[li].chars().collect();
        let mut spans: Vec<Span> = Vec::new();
        for (col, ch) in chars.iter().enumerate().take(b).skip(a) {
            let st = if show_cursor && li == crow && col == ccol {
                cur_st
            } else if in_sel(li, col) {
                line_st.bg(t.sel_bg)
            } else {
                line_st
            };
            spans.push(Span::styled(ch.to_string(), st));
        }
        // The cursor past the end of a line sits in the cell after it.
        if show_cursor && li == crow && ccol == chars.len() && b == chars.len() {
            spans.push(Span::styled(" ", cur_st));
        }
        out.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(out), area);
}

fn draw_composer(f: &mut Frame, c: &mut Composer, active: bool, from: &str, r: Rect, t: &Theme) {
    let w = r.width as usize;
    let label_w = (c.labels.iter().map(|l| l.width()).max().unwrap_or(7) + 3).max(9) as u16;
    let (mode_txt, mode_c) = match c.mode {
        VMode::Insert => ("INSERT", t.green),
        VMode::Normal => ("NORMAL", t.accent),
        VMode::Visual => ("VISUAL", t.magenta),
    };
    let title = format!(" {}", c.title);
    let mut head = vec![
        Span::styled(title.clone(), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
    ];
    let pill = format!(" {mode_txt} ");
    let gap = w.saturating_sub(title.width() + pill.width() + 1);
    head.push(Span::raw(" ".repeat(gap)));
    head.push(Span::styled(pill, Style::default().fg(t.bg).bg(if active { mode_c } else { t.subtle }).add_modifier(Modifier::BOLD)));
    let mut y = r.y;
    let row = |y: u16| Rect { x: r.x, y, width: r.width, height: 1 };
    f.render_widget(Paragraph::new(Line::from(head)), row(y));
    y += 1;
    if c.event.is_none() && c.account.is_none() {
        f.render_widget(Paragraph::new(header_line("From", from, w, t)), row(y));
        y += 1;
    }
    if !c.note.is_empty() {
        for l in textwrap::wrap(&c.note, w.saturating_sub(3).max(10)).into_iter().take(3) {
            f.render_widget(Paragraph::new(Span::styled(format!(" {l}"), Style::default().fg(t.subtle))), row(y));
            y += 1;
        }
    }

    let field_style = Style::default().fg(t.fg);
    let cursor_style = |focused: bool| {
        if !(active && focused) {
            field_style
        } else if c.mode == VMode::Insert {
            Style::default().fg(t.bg).bg(t.green)
        } else {
            Style::default().fg(t.bg).bg(t.accent)
        }
    };
    let mut focus_row = y;
    for i in 0..c.fields.len() {
        // Bcc stays out of the way until it has something or you go to it.
        if c.event.is_none() && c.account.is_none() && i == composer::BCC && c.focus != i && c.fields[i].is_empty() {
            continue;
        }
        let focused = c.focus == i;
        let label_c = if focused && active { t.accent } else { t.subtle };
        f.render_widget(
            Paragraph::new(Span::styled(format!(" {:<w$}", c.labels[i], w = label_w as usize - 1), Style::default().fg(label_c))),
            Rect { width: label_w, ..row(y) },
        );
        let ta = &mut c.fields[i];
        ta.set_style(field_style);
        ta.set_cursor_style(cursor_style(focused));
        ta.set_selection_style(Style::default().bg(t.sel_bg));
        ta.set_placeholder_text(c.placeholders.get(i).copied().unwrap_or(""));
        ta.set_placeholder_style(Style::default().fg(t.subtle));
        let field_area = Rect { x: r.x + label_w, y, width: r.width.saturating_sub(label_w + 1), height: 1 };
        // An empty field we're not in shows its hint, aligned with filled ones.
        if ta.is_empty() && !(active && focused) {
            let hint = c.placeholders.get(i).copied().unwrap_or("");
            f.render_widget(Paragraph::new(Span::styled(hint, Style::default().fg(t.subtle))), field_area);
        } else {
            f.render_widget(&*ta, field_area);
        }
        if focused {
            focus_row = y;
        }
        y += 1;
    }
    let names: Vec<String> = c
        .draft
        .attach_paths
        .iter()
        .map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
        .chain(c.draft.carried.iter().map(|(n, _)| n.clone()))
        .collect();
    if !names.is_empty() {
        let icons = Icons { on: true };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {:<8}", icons.clip()), Style::default().fg(t.subtle)),
                Span::styled(fit(&names.join("  "), w.saturating_sub(10)), Style::default().fg(t.yellow)),
            ])),
            row(y),
        );
        y += 1;
    }
    f.render_widget(
        Paragraph::new(Span::styled(format!(" {}", "─".repeat(w.saturating_sub(2))), Style::default().fg(mix(t.subtle, t.bg, 0.4)))),
        row(y),
    );
    y += 1;

    // Body, then a one-line key hint at the bottom.
    let bottom = r.y + r.height;
    let body_h = bottom.saturating_sub(y + 1);
    let body_focused = c.on_body();
    let cur_st = cursor_style(body_focused);
    draw_soft_wrapped(f, c, Rect { x: r.x + 1, y, width: r.width.saturating_sub(2), height: body_h }, cur_st, active && body_focused, t);

    let key = |k: &str, label: &str| {
        vec![Span::styled(k.to_string(), Style::default().fg(t.yellow)), Span::styled(format!(" {label}  "), Style::default().fg(t.subtle))]
    };
    let mut hint = vec![Span::raw(" ")];
    let keys: &[(&str, &str)] = if c.account.is_some() {
        &[("^s ZZ", "save"), (":q!", "cancel"), ("tab", "next field"), ("body", "= signature")]
    } else if c.event.is_some() {
        &[("^s ZZ", "save"), (":q!", "discard"), ("tab", "next field")]
    } else {
        &[("^s", "send"), ("^t", "attach"), (":w", "draft"), (":q!", "discard"), (":e", "$EDITOR")]
    };
    for (k, l) in keys.iter().copied() {
        hint.extend(key(k, l));
    }
    f.render_widget(Paragraph::new(Line::from(hint)), row(bottom.saturating_sub(1)));

    // Address completions drop down under the field being typed in.
    if active && !c.completions.is_empty() {
        let n = c.completions.len().min(6) as u16;
        let cw = c.completions.iter().map(|s| s.width()).max().unwrap_or(10) as u16 + 4;
        let area = Rect { x: r.x + label_w, y: focus_row + 1, width: cw.min(r.width.saturating_sub(label_w)), height: n.min(bottom.saturating_sub(focus_row + 1)) };
        f.render_widget(Clear, area);
        let lines: Vec<Line> = c
            .completions
            .iter()
            .take(n as usize)
            .enumerate()
            .map(|(i, s)| {
                let st = if c.sel == Some(i) {
                    Style::default().fg(t.fg).bg(t.sel_bg).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(t.muted).bg(t.surface)
                };
                Line::from(Span::styled(pad(&format!("  {s}"), area.width as usize), st))
            })
            .collect();
        f.render_widget(Paragraph::new(lines), area);
    }
}

fn draw_status(f: &mut Frame, app: &App, r: Rect, t: &Theme) {
    let (mode, mode_c) = match &app.mode {
        Mode::Normal if app.screen == View::Calendar => ("CALENDAR", t.magenta),
        Mode::Normal => match app.focus {
            Focus::Sidebar => ("FOLDERS", t.blue),
            Focus::List => ("MAIL", t.accent),
            Focus::Body => ("READ", t.green),
        },
        Mode::Prompt(p) => match p.kind {
            PromptKind::Filter | PromptKind::CalFilter => ("SEARCH", t.yellow),
            PromptKind::QuickAdd => ("ADD", t.green),
            PromptKind::ConfirmDelete => ("DELETE?", t.red),
            PromptKind::Move => ("MOVE", t.magenta),
            PromptKind::Attach => ("ATTACH", t.yellow),
        },
        Mode::Settings(_) => ("SETTINGS", t.cyan),
        Mode::Compose(c) => match c.mode {
            VMode::Insert => ("INSERT", t.green),
            VMode::Normal => ("NORMAL", t.accent),
            VMode::Visual => ("VISUAL", t.magenta),
        },
    };
    let w = r.width as usize;
    let mut spans = vec![Span::styled(format!(" {mode} "), Style::default().fg(t.bg).bg(mode_c).add_modifier(Modifier::BOLD))];

    if let Mode::Compose(c) = &app.mode {
        if let Some(cmd) = &c.command {
            spans.push(Span::styled(format!(" :{cmd}"), Style::default().fg(t.fg)));
            spans.push(Span::styled("▏", Style::default().fg(t.accent)));
            spans.push(Span::styled("  w draft · wq send · q! discard · e $EDITOR · a <file>", Style::default().fg(t.subtle)));
            f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(t.surface)), r);
            return;
        }
    }
    if let Mode::Prompt(p) = &app.mode {
        let label = match p.kind {
            PromptKind::Filter | PromptKind::CalFilter => " /",
            PromptKind::QuickAdd => " add:",
            PromptKind::ConfirmDelete => "",
            PromptKind::Move => " move to:",
            PromptKind::Attach => " file:",
        };
        spans.push(Span::styled(label, Style::default().fg(t.muted)));
        spans.push(Span::styled(format!(" {}", p.input), Style::default().fg(t.fg)));
        spans.push(Span::styled("▏", Style::default().fg(t.accent)));
        let hint = match p.kind {
            PromptKind::Filter => "  enter: also search server · esc: clear",
            PromptKind::CalFilter => "  filters events as you type · esc: clear",
            PromptKind::QuickAdd => "  e.g. Lunch with Maya fri 12:30 1h @Osteria · enter adds",
            PromptKind::ConfirmDelete => "  y deletes · anything else keeps it",
            PromptKind::Move | PromptKind::Attach => "  tab: complete · ↑↓ pick · enter: ok",
        };
        spans.push(Span::styled(hint, Style::default().fg(t.subtle)));
        f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(t.surface)), r);
        return;
    }

    if app.screen == View::Calendar {
        let d = app.cal.cursor_day();
        spans.push(Span::styled(format!(" Calendar › {} ", d.format("%a %e %b %Y")), Style::default().fg(t.muted)));
    } else if let Some((a, _)) = &app.current {
        let folder = app.current_folder().map(|f| f.label.clone()).unwrap_or_default();
        spans.push(Span::styled(format!(" {} › {folder} ", app.accts[*a].cfg.name), Style::default().fg(t.muted)));
    }
    if app.zoom {
        spans.push(Span::styled(" zoom ", Style::default().fg(t.cyan)));
    }
    let right_hint = Span::styled(" ? help ", Style::default().fg(t.subtle));
    let left_w: usize = spans.iter().map(|s| s.content.width()).sum();
    let room = w.saturating_sub(left_w + right_hint.content.width() + 1);
    let status_c = if app.error { t.red } else { t.muted };
    let msg = rpad(&app.status, room);
    spans.push(Span::styled(msg, Style::default().fg(status_c)));
    spans.push(Span::raw(" "));
    spans.push(right_hint);
    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(t.surface)), r);
}

fn draw_completions(f: &mut Frame, app: &App, main: Rect, t: &Theme) {
    let Mode::Prompt(p) = &app.mode else { return };
    let n = p.completions.len().min(8) as u16;
    let w = p.completions.iter().map(|c| c.width()).max().unwrap_or(10).clamp(24, main.width as usize - 4) as u16 + 4;
    let area = Rect { x: main.x + 1, y: main.y + main.height - n, width: w.min(main.width - 2), height: n };
    f.render_widget(Clear, area);
    let lines: Vec<Line> = p
        .completions
        .iter()
        .take(8)
        .enumerate()
        .map(|(i, c)| {
            let sel = p.sel == Some(i);
            let st = if sel {
                Style::default().fg(t.fg).bg(t.sel_bg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(t.muted).bg(t.surface)
            };
            Line::from(Span::styled(pad(&format!("  {c}"), area.width as usize), st))
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

fn draw_calendar(f: &mut Frame, app: &mut App, main: Rect, t: &Theme) {
    let side_w = app.cfg.ui.sidebar_width.min(main.width / 4).max(14);
    let rest = main.width.saturating_sub(side_w + 2);
    let list_w = (rest as u32 * app.cfg.ui.list_percent.clamp(20, 80) as u32 / 100) as u16;
    let [side, s1, list, s2, body] = Layout::horizontal([
        Constraint::Length(side_w),
        Constraint::Length(1),
        Constraint::Length(list_w),
        Constraint::Length(1),
        Constraint::Min(10),
    ])
    .areas(main);
    app.hits.side = side;
    app.hits.list = list;
    app.hits.body = body;
    draw_sep(f, s1, t);
    draw_sep(f, s2, t);
    draw_cal_sidebar(f, app, side, t);
    draw_agenda(f, app, list, t);
    draw_event(f, app, body, t);
}

fn draw_cal_sidebar(f: &mut Frame, app: &mut App, r: Rect, t: &Theme) {
    let focused = app.focus == Focus::Sidebar;
    let w = r.width as usize;
    let cal = &app.cal;
    let mut lines = vec![Line::from(pane_title("Calendars", focused, t))];
    let rows = cal.side_rows();
    if rows.is_empty() {
        lines.push(Line::from(""));
        let msg = if cal.syncing { "  syncing…" } else { "  none yet — $ syncs" };
        lines.push(Line::from(Span::styled(msg, Style::default().fg(t.subtle))));
    }
    for (ri, row) in rows.iter().enumerate() {
        let cursor = focused && ri == cal.side_cursor;
        let mut line = match row {
            None => {
                let src = rows.get(ri + 1).and_then(|r| r.map(|i| cal.calendars[i].source.clone())).unwrap_or_default();
                Line::from(Span::styled(pad(&format!(" {}", src.to_uppercase()), w), Style::default().fg(t.muted).add_modifier(Modifier::BOLD)))
            }
            Some(i) => {
                let c = &cal.calendars[*i];
                let on = cal.visible(*i);
                let col = calview::color(c, *i, t);
                Line::from(vec![
                    Span::styled(if on { "  ● " } else { "  ○ " }, Style::default().fg(if on { col } else { t.subtle })),
                    Span::styled(pad(&c.name, w.saturating_sub(4)), Style::default().fg(if on { t.fg } else { t.subtle })),
                ])
            }
        };
        if cursor {
            line = line.style(Style::default().bg(t.sel_bg));
        }
        lines.push(line);
    }

    // A small month at the bottom, marking today, the cursor's day and busy days.
    let day = cal.cursor_day();
    let today = Local::now().date_naive();
    let busy: std::collections::HashSet<chrono::NaiveDate> = cal
        .rows
        .windows(2)
        .filter_map(|w| match (w[0], w[1]) {
            (CalRow::Day(d), CalRow::Event(_)) => Some(d),
            _ => None,
        })
        .collect();
    let first = day.with_day(1).unwrap();
    let lead = first.weekday().num_days_from_monday() as i64;
    let weeks = ((lead + days_in_month(first) as i64) as f64 / 7.0).ceil() as usize;
    let need = weeks + 3;
    let avail = (r.height as usize).saturating_sub(lines.len());
    if avail >= need && w >= 21 {
        lines.extend(std::iter::repeat_n(Line::from(""), avail - need));
        let title = first.format("%B %Y").to_string();
        lines.push(Line::from(Span::styled(format!(" {title}"), Style::default().fg(t.accent).add_modifier(Modifier::BOLD))));
        lines.push(Line::from(Span::styled(" Mo Tu We Th Fr Sa Su", Style::default().fg(t.subtle))));
        for wk in 0..weeks {
            let mut spans = vec![Span::raw(" ")];
            for dow in 0..7 {
                let n = wk as i64 * 7 + dow - lead;
                let d = first + chrono::Duration::days(n);
                if d.month() != first.month() {
                    spans.push(Span::raw("   "));
                    continue;
                }
                let mut st = Style::default().fg(if busy.contains(&d) { t.fg } else { t.subtle });
                if d == today {
                    st = Style::default().fg(t.bg).bg(t.accent).add_modifier(Modifier::BOLD);
                } else if d == day {
                    st = st.add_modifier(Modifier::BOLD | Modifier::UNDERLINED).fg(t.fg);
                }
                spans.push(Span::styled(format!("{:>2}", d.day()), st));
                spans.push(Span::raw(" "));
            }
            lines.push(Line::from(spans));
        }
    }
    f.render_widget(Paragraph::new(lines), r);
}

fn days_in_month(first: chrono::NaiveDate) -> u32 {
    let next = first.checked_add_months(chrono::Months::new(1)).unwrap();
    (next - first).num_days() as u32
}

fn draw_agenda(f: &mut Frame, app: &mut App, r: Rect, t: &Theme) {
    let focused = app.focus == Focus::List;
    let w = r.width as usize;
    app.cal.height = r.height.saturating_sub(1) as usize;
    let cal = &app.cal;
    let mut head = vec![pane_title("Agenda", focused, t)];
    let n = cal.rows.iter().filter(|r| matches!(r, CalRow::Event(_))).count();
    head.push(Span::styled(format!("  {n} events"), Style::default().fg(t.subtle)));
    if !cal.filter.is_empty() {
        head.push(Span::styled(format!("  /{}", cal.filter), Style::default().fg(t.yellow)));
    }
    if cal.syncing {
        head.push(Span::styled("  syncing…", Style::default().fg(t.subtle)));
    }
    let mut lines = vec![Line::from(head)];
    let now = Local::now();
    let today = now.date_naive();
    let time_w = 11;
    for (ri, row) in cal.rows.iter().enumerate().skip(cal.offset).take(cal.height) {
        let sel = ri == cal.cursor;
        let mut line = match row {
            CalRow::Day(d) => {
                let rel = match (*d - today).num_days() {
                    0 => "Today · ".to_string(),
                    1 => "Tomorrow · ".to_string(),
                    -1 => "Yesterday · ".to_string(),
                    _ => String::new(),
                };
                let label = format!(" {rel}{}", d.format("%a %e %b"));
                let st = if *d == today {
                    Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
                } else if *d < today {
                    Style::default().fg(t.subtle).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(t.fg).add_modifier(Modifier::BOLD)
                };
                let fill = w.saturating_sub(label.width() + 1);
                let empty = !matches!(cal.rows.get(ri + 1), Some(CalRow::Event(_)));
                let mut spans = vec![Span::styled(label, st), Span::raw(" ")];
                if empty {
                    spans.push(Span::styled("nothing scheduled", Style::default().fg(t.subtle)));
                } else {
                    spans.push(Span::styled("─".repeat(fill), Style::default().fg(mix(t.subtle, t.bg, 0.6))));
                }
                Line::from(spans)
            }
            CalRow::Event(i) => {
                let o = &cal.occ[*i];
                let day = cal.rows[..ri].iter().rev().find_map(|r| match r {
                    CalRow::Day(d) => Some(*d),
                    _ => None,
                });
                let continues = day.is_some_and(|d| o.start.date_naive() < d);
                let past = o.end <= now;
                let ongoing = o.start <= now && now < o.end;
                let time = if o.all_day {
                    "all day".to_string()
                } else if continues {
                    format!("…–{}", o.end.format("%H:%M"))
                } else if o.end > o.start {
                    format!("{}–{}", o.start.format("%H:%M"), o.end.format("%H:%M"))
                } else {
                    o.start.format("%H:%M").to_string()
                };
                let col = calview::color(&cal.calendars[o.calendar], o.calendar, t);
                let time_st = if ongoing {
                    Style::default().fg(t.green).add_modifier(Modifier::BOLD)
                } else if past {
                    Style::default().fg(t.subtle)
                } else {
                    Style::default().fg(t.muted)
                };
                let title_st = if past { Style::default().fg(t.muted) } else { Style::default().fg(t.fg) };
                let rec = if o.recurring { " ↻" } else { "" };
                let used = 2 + time_w + 3;
                let room = w.saturating_sub(used + rec.width());
                let mut title = o.summary.clone();
                if !o.location.is_empty() && title.width() + 3 < room {
                    title = fit(&title, room);
                }
                let loc_room = room.saturating_sub(title.width() + 2);
                let mut spans = vec![
                    Span::raw("  "),
                    Span::styled(pad(&time, time_w), time_st),
                    Span::styled(" ▌ ", Style::default().fg(if past { mix(col, t.bg, 0.5) } else { col })),
                    Span::styled(fit(&title, room), title_st),
                ];
                if !o.location.is_empty() && loc_room > 6 {
                    spans.push(Span::styled(format!("  {}", fit(&o.location, loc_room - 2)), Style::default().fg(t.subtle)));
                }
                spans.push(Span::styled(rec, Style::default().fg(t.subtle)));
                Line::from(spans)
            }
        };
        if sel {
            line = line.style(Style::default().bg(if focused { t.sel_bg } else { mix(t.sel_bg, t.bg, 0.5) }));
        }
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), r);
}

fn draw_event(f: &mut Frame, app: &mut App, r: Rect, t: &Theme) {
    let active = matches!(app.mode, Mode::Compose(_));
    if let Some(c) = app.composer_mut() {
        return draw_composer(f, c, active, "", r, t);
    }
    let focused = app.focus == Focus::Body;
    let w = r.width as usize;
    let Some(o) = app.cal.selected().cloned() else {
        let lines = vec![
            Line::from(pane_title("", focused, t)),
            Line::from(""),
            Line::from(Span::styled("  Nothing selected", Style::default().fg(t.subtle))),
            Line::from(Span::styled("  t today · h/l day · H/L week · [ ] month", Style::default().fg(t.subtle))),
        ];
        f.render_widget(Paragraph::new(lines), r);
        return;
    };
    let cal = &app.cal.calendars[o.calendar];
    let col = calview::color(cal, o.calendar, t);
    let mut lines: Vec<Line> = Vec::new();
    let st = Style::default().fg(if focused { t.accent } else { t.fg }).add_modifier(Modifier::BOLD);
    for l in textwrap::wrap(&o.summary, w.saturating_sub(2).max(10)).into_iter().take(3) {
        lines.push(Line::from(Span::styled(format!(" {l}"), st)));
    }
    let mins = (o.end - o.start).num_minutes();
    let when = if o.all_day {
        let days = (o.end.date_naive() - o.start.date_naive()).num_days().max(1);
        if days > 1 {
            format!("{} – {} · {days} days", o.start.format("%a %e %b"), (o.end - chrono::Duration::days(1)).format("%a %e %b"))
        } else {
            format!("{} · all day", o.start.format("%a %e %b %Y"))
        }
    } else {
        let len = match mins {
            0 => String::new(),
            m if m < 60 => format!(" · {m} min"),
            m if m % 60 == 0 => format!(" · {} h", m / 60),
            m => format!(" · {}h {}m", m / 60, m % 60),
        };
        if o.start.date_naive() == o.end.date_naive() || mins == 0 {
            format!("{} · {}–{}{len}", o.start.format("%a %e %b %Y"), o.start.format("%H:%M"), o.end.format("%H:%M"))
        } else {
            format!("{} – {}{len}", o.start.format("%a %e %b %H:%M"), o.end.format("%a %e %b %H:%M"))
        }
    };
    lines.push(Line::from(Span::styled(format!(" {when}"), Style::default().fg(t.fg))));
    let mut calline = vec![
        Span::styled(" ● ", Style::default().fg(col)),
        Span::styled(format!("{} · {}", cal.name, cal.source), Style::default().fg(t.muted)),
    ];
    if o.recurring {
        calline.push(Span::styled("  ↻ repeats", Style::default().fg(t.subtle)));
    }
    lines.push(Line::from(calline));
    lines.push(Line::from(Span::styled(format!(" {}", "─".repeat(w.saturating_sub(2))), Style::default().fg(mix(t.subtle, t.bg, 0.4)))));
    let mut detail: Vec<Line> = Vec::new();
    let field = |label: &str, value: &str, t: &Theme| -> Line<'static> {
        let mut l = styled_line(&fit(value, w.saturating_sub(12)), Kind::Text, t);
        l.spans.insert(0, Span::styled(format!(" {label:<10}"), Style::default().fg(t.subtle)));
        l
    };
    if !o.location.is_empty() {
        detail.push(field("Where", &o.location, t));
    }
    if !o.url.is_empty() {
        detail.push(field("Link", &o.url, t));
    }
    if !o.organizer.is_empty() {
        detail.push(field("Organizer", &o.organizer, t));
    }
    for (i, a) in o.attendees.iter().enumerate() {
        detail.push(field(if i == 0 { "Guests" } else { "" }, a, t));
    }
    if !o.description.is_empty() {
        detail.push(Line::from(""));
        let text = o.description.replace("\\n", "\n");
        for (s, k) in wrap(&classify(&text), w.saturating_sub(3)) {
            let mut l = styled_line(&s, k, t);
            l.spans.insert(0, Span::raw(" "));
            detail.push(l);
        }
    }
    let head_h = lines.len() as u16;
    f.render_widget(Paragraph::new(lines), Rect { height: head_h.min(r.height), ..r });
    let area = Rect { y: r.y + head_h, height: r.height.saturating_sub(head_h), ..r };
    let max = detail.len().saturating_sub(area.height as usize);
    app.cal.detail_scroll = app.cal.detail_scroll.min(max);
    f.render_widget(Paragraph::new(detail.into_iter().skip(app.cal.detail_scroll).collect::<Vec<_>>()), area);
}

fn draw_settings(f: &mut Frame, app: &mut App, r: Rect, t: &Theme) {
    let st = match (&app.mode, &app.parked_settings) {
        (Mode::Settings(st), _) => st,
        (Mode::Compose(_), Some(st)) => st,
        _ => return,
    };
    let (tab, cursor, editing, filter, typing) = (st.tab, st.cursor, st.editing.clone(), st.filter.clone(), st.typing);
    let side_w = app.cfg.ui.sidebar_width.min(r.width / 4).max(14);
    let [side, sep, body] =
        Layout::horizontal([Constraint::Length(side_w), Constraint::Length(1), Constraint::Min(10)]).areas(r);
    draw_sep(f, sep, t);

    // Tabs down the left, like the folder sidebar.
    let icons = ["\u{f013}", "\u{f007}", "\u{f11c}", "\u{f1fc}"];
    let mut lines = vec![Line::from(pane_title("Settings", true, t))];
    for (i, name) in settings::TABS.iter().enumerate() {
        let on = i == tab;
        let icon = if app.cfg.ui.icons { icons[i] } else { "·" };
        let mut line = Line::from(vec![
            Span::styled(if on { "▎" } else { " " }, Style::default().fg(t.accent)),
            Span::styled(format!("{icon} "), Style::default().fg(if on { t.accent } else { t.subtle })),
            Span::styled(
                pad(name, side.width as usize - 4),
                Style::default().fg(if on { t.fg } else { t.muted }).add_modifier(if on { Modifier::BOLD } else { Modifier::empty() }),
            ),
            Span::styled(format!("{}", i + 1), Style::default().fg(t.subtle)),
        ]);
        if on {
            line = line.style(Style::default().bg(t.sel_bg));
        }
        app.hits.tabs.push((Rect { x: side.x, y: side.y + 1 + i as u16, width: side.width, height: 1 }, i));
        lines.push(line);
    }
    let path = app.cfg_path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "demo — changes not saved".into());
    let free = (side.height as usize).saturating_sub(lines.len() + 3);
    lines.extend(std::iter::repeat_n(Line::from(""), free));
    lines.push(Line::from(Span::styled(" config", Style::default().fg(t.subtle))));
    for chunk in textwrap::wrap(&path, side.width.saturating_sub(2).max(8) as usize).into_iter().take(2) {
        lines.push(Line::from(Span::styled(format!(" {chunk}"), Style::default().fg(t.muted))));
    }
    f.render_widget(Paragraph::new(lines), side);

    let inner = Rect { x: body.x + 1, width: body.width.saturating_sub(2), ..body };
    app.hits.settings_body = inner;
    // An account form opens over the tab's content.
    if let Mode::Compose(c) = &mut app.mode {
        if c.account.is_some() {
            return draw_composer(f, c, true, "", inner, t);
        }
    }
    match tab {
        settings::GENERAL => draw_settings_general(f, app, inner, t, cursor, editing),
        settings::ACCOUNTS => draw_settings_accounts(f, app, inner, t),
        settings::KEYS => draw_settings_keys(f, app, inner, t, &filter, typing),
        settings::THEME => draw_settings_theme(f, app, inner, t),
        _ => {}
    }
}

fn section_line<'a>(title: &str, t: &Theme) -> Line<'a> {
    Line::from(Span::styled(title.to_uppercase(), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)))
}

fn draw_settings_general(f: &mut Frame, app: &mut App, r: Rect, t: &Theme, cursor: usize, editing: Option<String>) {
    let w = r.width as usize;
    let label_w = 28.min(w / 2);
    let mut lines = vec![Line::from("")];
    for (ri, row) in settings::general_rows().iter().enumerate() {
        match row {
            GeneralRow::Header(h) => {
                if ri > 0 {
                    lines.push(Line::from(""));
                }
                lines.push(section_line(h, t));
            }
            GeneralRow::Item(i) => {
                let it = &settings::ITEMS[*i];
                let sel = ri == cursor;
                let raw = settings::get(&app.cfg.ui, it.key);
                let value: Vec<Span> = match (&editing, sel) {
                    (Some(buf), true) => vec![
                        Span::styled(buf.clone(), Style::default().fg(t.fg).add_modifier(Modifier::BOLD)),
                        Span::styled("▏", Style::default().fg(t.accent)),
                    ],
                    _ => match it.kind {
                        settings::Kind::Bool if raw == "true" => vec![Span::styled("● on", Style::default().fg(t.green))],
                        settings::Kind::Bool => vec![Span::styled("○ off", Style::default().fg(t.subtle))],
                        settings::Kind::Num { unit, .. } => vec![
                            Span::styled(raw, Style::default().fg(t.fg)),
                            Span::styled(format!(" {unit}"), Style::default().fg(t.subtle)),
                        ],
                        settings::Kind::Text if raw.is_empty() => vec![Span::styled("default", Style::default().fg(t.subtle))],
                        settings::Kind::Text => vec![Span::styled(fit(&raw, w.saturating_sub(label_w + 4)), Style::default().fg(t.fg))],
                    },
                };
                let mut spans = vec![Span::styled(
                    pad(&format!("  {}", it.label), label_w),
                    Style::default().fg(if sel { t.fg } else { t.muted }),
                )];
                spans.extend(value);
                let mut line = Line::from(spans);
                if sel {
                    line = line.style(Style::default().bg(t.sel_bg));
                }
                app.hits.settings_rows.push((r.y + lines.len() as u16, ri));
                lines.push(line);
            }
        }
    }
    // What the selected setting does, and how to change it.
    if let Some(GeneralRow::Item(i)) = settings::general_rows().get(cursor) {
        let it = &settings::ITEMS[*i];
        lines.push(Line::from(""));
        for l in textwrap::wrap(it.help, w.saturating_sub(4).max(10)) {
            lines.push(Line::from(Span::styled(format!("  {l}"), Style::default().fg(t.subtle))));
        }
        let how = match (it.kind, editing.is_some()) {
            (_, true) => "⏎ save · esc cancel · ^u clear",
            (settings::Kind::Bool, _) => "⏎ / space / click toggles",
            (settings::Kind::Num { .. }, _) => "+ / − adjust · ⏎ type a value",
            (settings::Kind::Text, _) => "⏎ edit",
        };
        lines.push(Line::from(Span::styled(format!("  {how}"), Style::default().fg(t.yellow))));
    }
    f.render_widget(Paragraph::new(lines), r);
}

fn kv<'a>(k: &str, v: String, t: &Theme) -> Line<'a> {
    Line::from(vec![Span::styled(format!("  {k:<12}"), Style::default().fg(t.subtle)), Span::styled(v, Style::default().fg(t.fg))])
}

fn draw_settings_accounts(f: &mut Frame, app: &mut App, r: Rect, t: &Theme) {
    let tls = |x: crate::config::Tls| match x {
        crate::config::Tls::Implicit => "TLS",
        crate::config::Tls::Starttls => "STARTTLS",
        crate::config::Tls::None => "no encryption",
    };
    let (cursor, picking, confirm) = match &app.mode {
        Mode::Settings(st) => (st.acct_cursor, st.picking, st.confirm),
        _ => (0, None, None),
    };
    let w = r.width as usize;
    let mut lines = vec![Line::from("")];

    // The provider chooser for a new account.
    if let Some(p) = picking {
        lines.push(section_line("New account · choose a provider", t));
        lines.push(Line::from(""));
        for (i, pr) in crate::accounts::PRESETS.iter().enumerate() {
            let sel = i == p;
            let extra = match (pr.caldav.is_empty(), pr.oauth.is_empty()) {
                (false, true) => "mail · calendar · contacts",
                (false, false) => "mail · calendar · contacts · OAuth",
                (true, false) => "mail · OAuth",
                (true, true) => "mail",
            };
            let mut l = Line::from(vec![
                Span::styled(format!("  {}", pad(pr.label, 28)), Style::default().fg(if sel { t.fg } else { t.muted }).add_modifier(if sel { Modifier::BOLD } else { Modifier::empty() })),
                Span::styled(extra, Style::default().fg(t.subtle)),
            ]);
            if sel {
                l = l.style(Style::default().bg(t.sel_bg));
            }
            lines.push(l);
        }
        lines.push(Line::from(""));
        for l in textwrap::wrap(crate::accounts::PRESETS[p].note, w.saturating_sub(4).max(10)) {
            lines.push(Line::from(Span::styled(format!("  {l}"), Style::default().fg(t.subtle))));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("  ⏎ continue · esc cancel", Style::default().fg(t.yellow))));
        return scroll_render(f, app, r, lines);
    }

    lines.push(section_line("Accounts", t));
    let n = app.cfg.accounts.len();
    for (i, a) in app.cfg.accounts.iter().enumerate() {
        let sel = i == cursor;
        let live = app.accts.iter().find(|s| s.cfg.name == a.name);
        let (dot, dc) = match live {
            Some(s) if s.connected => ("●", t.green),
            Some(_) => ("○", t.subtle),
            None => ("◌", t.yellow),
        };
        let kind = if let Some(o) = &a.oauth { format!("{} · OAuth", o.provider) } else { "password".into() };
        let mut l = Line::from(vec![
            Span::styled(format!("  {dot} "), Style::default().fg(dc)),
            Span::styled(pad(&a.name, 16), Style::default().fg(t.fg).add_modifier(Modifier::BOLD)),
            Span::styled(pad(&a.email, 34), Style::default().fg(t.muted)),
            Span::styled(kind, Style::default().fg(t.subtle)),
        ]);
        if sel {
            l = l.style(Style::default().bg(t.sel_bg));
        }
        lines.push(l);
    }
    let mut add = Line::from(Span::styled("  + add an account", Style::default().fg(t.accent)));
    if cursor >= n {
        add = add.style(Style::default().bg(t.sel_bg));
    }
    lines.push(add);
    lines.push(Line::from(""));

    if let Some(i) = confirm {
        lines.push(Line::from(Span::styled(
            format!("  Remove {} and its calendar and contacts from config.toml?  y yes · anything else no", app.cfg.accounts[i].name),
            Style::default().fg(t.red).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
    }

    // Details of the selected account.
    if let Some(a) = app.cfg.accounts.get(cursor) {
        lines.push(kv("IMAP", format!("{}:{} · {} · {}", a.imap.host, a.imap.port, tls(a.imap.tls), a.imap.username), t));
        lines.push(kv("SMTP", format!("{}:{} · {} · {}", a.smtp.host, a.smtp.port, tls(a.smtp.tls), a.smtp.username), t));
        if let Some(s) = app.accts.iter().find(|s| s.cfg.name == a.name) {
            let unread: u32 = s.folders.iter().map(|f| f.unread).sum();
            lines.push(kv("Folders", format!("{} · {unread} unread", s.folders.len()), t));
        }
        lines.push(kv("Sent copies", if a.save_sent { "saved to Sent".into() } else { "left to the server".into() }, t));
        let owned: Vec<_> = app.cfg.dav_sources().into_iter().filter(|d| d.account.as_deref() == Some(a.name.as_str())).collect();
        for d in &owned {
            lines.push(kv(d.name.trim_start_matches(&a.name).trim(), d.url.clone(), t));
        }
        if owned.is_empty() {
            lines.push(kv("Calendar", "none — edit to add a CalDAV URL".into(), t));
        }
    }
    let other: Vec<_> = app.cfg.dav_sources().into_iter().filter(|d| d.account.is_none()).collect();
    if !other.is_empty() {
        lines.push(Line::from(""));
        lines.push(section_line("Other calendar & contacts sources", t));
        for d in other {
            lines.push(kv(&d.name, format!("{} · {}", d.url, d.username), t));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(format!("  {} contacts cached", app.contacts.len()), Style::default().fg(t.subtle))));
    lines.push(Line::from(Span::styled("  a add · ⏎ edit · d remove", Style::default().fg(t.yellow))));
    scroll_render(f, app, r, lines);
}

fn draw_settings_keys(f: &mut Frame, app: &mut App, r: Rect, t: &Theme, filter: &str, typing: bool) {
    let q = filter.to_lowercase();
    let key_w = 16;
    // Lay sections out in as many columns as fit, balancing their heights.
    let col_w = 60u16;
    let cols = (r.width / col_w).max(1) as usize;
    let sections: Vec<(&str, Vec<(&str, &str)>)> = crate::keys::KEYMAP
        .iter()
        .map(|s| {
            let keys: Vec<(&str, &str)> = s
                .keys
                .iter()
                .copied()
                .filter(|(k, d)| q.is_empty() || k.to_lowercase().contains(&q) || d.to_lowercase().contains(&q) || s.title.to_lowercase().contains(&q))
                .collect();
            (s.title, keys)
        })
        .filter(|(_, k)| !k.is_empty())
        .collect();
    let total: usize = sections.iter().map(|(_, k)| k.len() + 2).sum();
    let per_col = total.div_ceil(cols).max(1);
    let mut columns: Vec<Vec<Line>> = vec![vec![]; cols];
    let mut c = 0;
    for (title, keys) in &sections {
        if columns[c].len() >= per_col && c + 1 < cols {
            c += 1;
        }
        let col = &mut columns[c];
        if !col.is_empty() {
            col.push(Line::from(""));
        }
        col.push(section_line(title, t));
        for (k, d) in keys {
            col.push(Line::from(vec![
                Span::styled(format!("  {}", pad(k, key_w - 2)), Style::default().fg(t.yellow)),
                Span::styled(fit(d, col_w as usize - key_w - 2), Style::default().fg(t.fg)),
            ]));
        }
    }
    let header = if typing || !filter.is_empty() {
        Line::from(vec![
            Span::styled(" / ", Style::default().fg(t.yellow)),
            Span::styled(filter.to_string(), Style::default().fg(t.fg)),
            Span::styled(if typing { "▏" } else { "" }, Style::default().fg(t.accent)),
            Span::styled("   esc clears", Style::default().fg(t.subtle)),
        ])
    } else {
        Line::from(Span::styled(" / to filter · these are the defaults", Style::default().fg(t.subtle)))
    };
    f.render_widget(Paragraph::new(header), Rect { height: 1, ..r });
    let area = Rect { y: r.y + 2, height: r.height.saturating_sub(2), ..r };
    let tallest = columns.iter().map(|c| c.len()).max().unwrap_or(0);
    let Mode::Settings(st) = &mut app.mode else { return };
    st.scroll = st.scroll.min(tallest.saturating_sub(area.height as usize));
    let scroll = st.scroll;
    for (i, col) in columns.into_iter().enumerate() {
        let x = area.x + i as u16 * col_w;
        if x >= area.x + area.width {
            break;
        }
        let rect = Rect { x, y: area.y, width: col_w.min(area.x + area.width - x), height: area.height };
        f.render_widget(Paragraph::new(col.into_iter().skip(scroll).collect::<Vec<_>>()), rect);
    }
}

fn draw_settings_theme(f: &mut Frame, app: &mut App, r: Rect, t: &Theme) {
    let dir = app
        .cfg
        .ui
        .theme_dir
        .clone()
        .or_else(crate::theme::default_dir)
        .and_then(|d| std::fs::canonicalize(&d).ok().or(Some(d)))
        .map(|d| d.display().to_string())
        .unwrap_or_else(|| "none found — using the built-in Tokyo Night".into());
    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(t.name.clone(), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled("  follows omarchy-theme-set live", Style::default().fg(t.subtle)),
        ]),
        kv("From", dir, t),
        Line::from(""),
        section_line("Palette", t),
    ];
    let sw = [
        ("background", t.bg),
        ("foreground", t.fg),
        ("accent", t.accent),
        ("muted", t.muted),
        ("subtle", t.subtle),
        ("selection", t.sel_bg),
        ("red", t.red),
        ("green", t.green),
        ("yellow", t.yellow),
        ("blue", t.blue),
        ("magenta", t.magenta),
        ("cyan", t.cyan),
    ];
    for pair in sw.chunks(3) {
        let mut spans = vec![Span::raw("  ")];
        for (name, c) in pair {
            spans.push(Span::styled("████", Style::default().fg(*c)));
            spans.push(Span::styled(format!(" {name:<14}"), Style::default().fg(t.muted)));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(""));
    lines.push(section_line("Preview", t));
    lines.push(Line::from(vec![
        Span::styled("  ● ", Style::default().fg(t.accent)),
        Span::styled("Maya Okafor   ", Style::default().fg(t.fg).add_modifier(Modifier::BOLD)),
        Span::styled("Unread conversation", Style::default().fg(t.fg).add_modifier(Modifier::BOLD)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("    Jonas Berg    ", Style::default().fg(t.muted)),
        Span::styled("Read message", Style::default().fg(t.muted)),
    ]).style(Style::default().bg(t.sel_bg)));
    for (d, text) in [(1, "> quoted once"), (2, ">> quoted twice"), (3, ">>> and again")] {
        lines.push(Line::from(Span::styled(format!("  {text}"), Style::default().fg(t.quote[d - 1]))));
    }
    lines.push(Line::from(Span::styled("  https://example.com/link", Style::default().fg(t.cyan).add_modifier(Modifier::UNDERLINED))));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Pick colors with Omarchy's theme menu; tern reads alacritty.toml, kitty.conf, ghostty.conf or colors.toml.",
        Style::default().fg(t.subtle),
    )));
    scroll_render(f, app, r, lines);
}

fn scroll_render(f: &mut Frame, app: &mut App, r: Rect, lines: Vec<Line>) {
    let Mode::Settings(st) = &mut app.mode else { return };
    st.scroll = st.scroll.min(lines.len().saturating_sub(r.height as usize));
    let skip = st.scroll;
    f.render_widget(Paragraph::new(lines.into_iter().skip(skip).collect::<Vec<_>>()), r);
}
