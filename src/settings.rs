//! The settings view: what can be changed from inside tern, and saving it back to
//! config.toml without disturbing the comments and layout you wrote there.

use crate::config::Ui;
use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};

pub const TABS: [&str; 5] = ["General", "Accounts", "Calendars", "Keys", "Theme"];
pub const GENERAL: usize = 0;
pub const ACCOUNTS: usize = 1;
pub const CALENDARS: usize = 2;
pub const KEYS: usize = 3;
pub const THEME: usize = 4;

#[derive(Clone, Copy)]
pub enum Kind {
    Bool,
    Num { min: i64, max: i64, step: i64, unit: &'static str },
    Text,
}

pub struct Item {
    pub section: &'static str,
    pub label: &'static str,
    /// Key under `[ui]` in config.toml.
    pub key: &'static str,
    pub kind: Kind,
    pub help: &'static str,
}

pub const ITEMS: &[Item] = &[
    Item { section: "Reading", label: "Group into threads", key: "threads", kind: Kind::Bool, help: "One row per conversation, grouped by reply headers. T toggles it for now." },
    Item { section: "Reading", label: "Start threads folded", key: "collapse_threads", kind: Kind::Bool, help: "Show each conversation as one row until za unfolds it." },
    Item { section: "Writing", label: "Undo send window", key: "undo_send_secs", kind: Kind::Num { min: 0, max: 60, step: 5, unit: "s" }, help: "How long a sent message waits in the outbox before going out; ^z takes it back. 0 sends at once." },
    Item { section: "Reading", label: "Mark read after", key: "mark_read_ms", kind: Kind::Num { min: 0, max: 10_000, step: 100, unit: "ms" }, help: "How long a message must stay open before it counts as read. 0 marks it at once." },
    Item { section: "Calendar", label: "Time zone", key: "calendar_timezone", kind: Kind::Text, help: "Calendar times are shown and entered in this zone. An IANA name (America/New_York, Europe/Berlin) or est, cst, mst, pst, utc; daylight time is handled." },
    Item { section: "Calendar", label: "Notifications", key: "notifications", kind: Kind::Bool, help: "Desktop notifications (notify-send) for new mail in your inboxes and before meetings." },
    Item { section: "Calendar", label: "Remind before", key: "reminder_minutes", kind: Kind::Num { min: 0, max: 120, step: 5, unit: "min" }, help: "For meetings without their own alarm. An event's alarm always wins; 0 means only those." },
    Item { section: "Calendar", label: "Zoom link", key: "zoom_link", kind: Kind::Text, help: "Your Personal Meeting Room link (zoom.us/j/…). gz puts it on the selected event; :zoom in the event form, @zoom in quick add." },
    Item { section: "Calendar", label: "Meet account", key: "meet_account", kind: Kind::Text, help: "The Google account gm creates Meet links with. Empty means the first account signing in with Google OAuth." },
    Item { section: "Layout", label: "Sidebar width", key: "sidebar_width", kind: Kind::Num { min: 12, max: 60, step: 2, unit: "cols" }, help: "Width of the folder column." },
    Item { section: "Layout", label: "Message list width", key: "list_percent", kind: Kind::Num { min: 20, max: 80, step: 5, unit: "%" }, help: "Share of the remaining width given to the list; the message gets the rest." },
    Item { section: "Layout", label: "Nerd Font icons", key: "icons", kind: Kind::Bool, help: "Folder and flag glyphs. Omarchy ships JetBrainsMono Nerd Font." },
    Item { section: "Layout", label: "Paint background", key: "paint_background", kind: Kind::Bool, help: "Fill with the theme background. Off keeps the terminal's, and its transparency." },
    Item { section: "Layout", label: "Mouse", key: "mouse", kind: Kind::Bool, help: "Click to select, double-click to open, wheel to scroll. Shift-drag still selects text in most terminals; turn this off to get plain selection back." },
    Item { section: "Layout", label: "Date format", key: "date_format", kind: Kind::Text, help: "strftime format for dates older than a week, e.g. %b %d or %d/%m." },
    Item { section: "Files", label: "Download folder", key: "download_dir", kind: Kind::Text, help: "Where S saves attachments." },
    Item { section: "Files", label: "External editor", key: "editor", kind: Kind::Text, help: "Used by :e in the composer. Empty means $VISUAL, then $EDITOR, then nvim/vim." },
];

#[derive(Default)]
pub struct Settings {
    pub tab: usize,
    pub cursor: usize,
    pub scroll: usize,
    /// Text being typed into a value, while editing one.
    pub editing: Option<String>,
    /// Filter for the Keys tab; `typing` while the `/` prompt is open.
    pub filter: String,
    pub typing: bool,
    pub pending_g: bool,
    /// Accounts tab: the selected row (one past the last account is "add").
    pub acct_cursor: usize,
    /// Choosing a provider for a new account: the highlighted preset.
    pub picking: Option<usize>,
    /// Asking before removing this account.
    pub confirm: Option<usize>,
    /// Calendars tab: selected row, provider chooser, removal question.
    pub dav_cursor: usize,
    pub dav_picking: Option<usize>,
    pub dav_confirm: Option<usize>,
}

pub fn get(ui: &Ui, key: &str) -> String {
    match key {
        "threads" => ui.threads.to_string(),
        "collapse_threads" => ui.collapse_threads.to_string(),
        "mark_read_ms" => ui.mark_read_ms.to_string(),
        "undo_send_secs" => ui.undo_send_secs.to_string(),
        "sidebar_width" => ui.sidebar_width.to_string(),
        "list_percent" => ui.list_percent.to_string(),
        "icons" => ui.icons.to_string(),
        "paint_background" => ui.paint_background.to_string(),
        "mouse" => ui.mouse.to_string(),
        "date_format" => ui.date_format.clone(),
        "calendar_timezone" => ui.calendar_timezone.clone(),
        "zoom_link" => ui.zoom_link.clone().unwrap_or_default(),
        "notifications" => ui.notifications.to_string(),
        "reminder_minutes" => ui.reminder_minutes.to_string(),
        "meet_account" => ui.meet_account.clone().unwrap_or_default(),
        "download_dir" => tilde(&ui.download_dir),
        "editor" => ui.editor.clone().unwrap_or_default(),
        _ => String::new(),
    }
}

fn tilde(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    match dirs::home_dir().map(|h| h.to_string_lossy().into_owned()) {
        Some(h) if s.starts_with(&h) => format!("~{}", &s[h.len()..]),
        _ => s,
    }
}

/// Apply a value typed or toggled in the view. Returns the value as it should be saved.
pub fn set(ui: &mut Ui, item: &Item, raw: &str) -> Result<toml_edit::Item> {
    let raw = raw.trim();
    match item.kind {
        Kind::Bool => {
            let v = matches!(raw, "true" | "on" | "yes" | "1");
            match item.key {
                "threads" => ui.threads = v,
                "collapse_threads" => ui.collapse_threads = v,
                "icons" => ui.icons = v,
                "paint_background" => ui.paint_background = v,
                "mouse" => ui.mouse = v,
                "notifications" => ui.notifications = v,
                _ => bail!("unknown setting"),
            }
            Ok(toml_edit::value(v))
        }
        Kind::Num { min, max, unit, .. } => {
            let v: i64 = raw.trim_end_matches(unit).trim().parse().map_err(|_| anyhow!("{} must be a number", item.label))?;
            if !(min..=max).contains(&v) {
                bail!("{} must be between {min} and {max}", item.label);
            }
            match item.key {
                "mark_read_ms" => ui.mark_read_ms = v as u64,
                "undo_send_secs" => ui.undo_send_secs = v as u64,
                "reminder_minutes" => ui.reminder_minutes = v as u64,
                "sidebar_width" => ui.sidebar_width = v as u16,
                "list_percent" => ui.list_percent = v as u16,
                _ => bail!("unknown setting"),
            }
            Ok(toml_edit::value(v))
        }
        Kind::Text => {
            match item.key {
                "date_format" => {
                    if raw.is_empty() {
                        bail!("date format can't be empty");
                    }
                    ui.date_format = raw.to_string()
                }
                "download_dir" => ui.download_dir = crate::theme::expand(raw),
                "calendar_timezone" => {
                    let tz = crate::calendar::parse_zone(raw).ok_or_else(|| anyhow!("unknown time zone `{raw}` — try America/New_York or est"))?;
                    ui.calendar_timezone = tz.name().to_string();
                    return Ok(toml_edit::value(tz.name()));
                }
                "editor" => ui.editor = (!raw.is_empty()).then(|| raw.to_string()),
                "zoom_link" => {
                    if !raw.is_empty() && !raw.starts_with("https://") {
                        bail!("the Zoom link should start with https://");
                    }
                    ui.zoom_link = (!raw.is_empty()).then(|| raw.to_string())
                }
                "meet_account" => ui.meet_account = (!raw.is_empty()).then(|| raw.to_string()),
                _ => bail!("unknown setting"),
            }
            if raw.is_empty() {
                return Ok(toml_edit::Item::None);
            }
            Ok(toml_edit::value(raw))
        }
    }
}

/// Write one `[ui]` key into the config file, keeping everything else as it was.
pub fn persist(path: &PathBuf, key: &str, value: toml_edit::Item) -> Result<()> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut doc: toml_edit::DocumentMut = text.parse().with_context(|| format!("parsing {}", path.display()))?;
    if !doc.contains_key("ui") {
        doc["ui"] = toml_edit::table();
    }
    let ui = doc["ui"].as_table_mut().ok_or_else(|| anyhow!("[ui] is not a table"))?;
    if value.is_none() {
        ui.remove(key);
    } else {
        ui[key] = value;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Rows of the General tab: section headers interleaved with items.
pub enum GeneralRow {
    Header(&'static str),
    Item(usize),
}

pub fn general_rows() -> Vec<GeneralRow> {
    let mut rows = Vec::new();
    let mut last = "";
    for (i, it) in ITEMS.iter().enumerate() {
        if it.section != last {
            rows.push(GeneralRow::Header(it.section));
            last = it.section;
        }
        rows.push(GeneralRow::Item(i));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_without_losing_comments() {
        let dir = std::env::temp_dir().join(format!("tern-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "# mine\n[[accounts]]\nname = \"x\" # keep\n\n[ui]\nicons = true # glyphs\n").unwrap();
        let mut ui = Ui::default();
        let item = ITEMS.iter().find(|i| i.key == "icons").unwrap();
        persist(&path, "icons", set(&mut ui, item, "off").unwrap()).unwrap();
        let item = ITEMS.iter().find(|i| i.key == "mark_read_ms").unwrap();
        persist(&path, "mark_read_ms", set(&mut ui, item, "1200 ms").unwrap()).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("# mine") && out.contains("# keep"));
        assert!(out.contains("icons = false"));
        assert!(out.contains("mark_read_ms = 1200"));
        assert!(!ui.icons && ui.mark_read_ms == 1200);
        assert!(set(&mut ui, item, "99999").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
