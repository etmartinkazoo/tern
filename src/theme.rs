//! Colors from the active fedcandor or Omarchy theme.
//!
//! fedcandor renders the current theme's resolved palette to
//! `~/.local/state/fedcandor/theme/rendered/colors.toml` on every `fedcandor-theme set`.
//! Omarchy keeps the current theme at `~/.config/omarchy/current/theme`, a symlink that
//! `omarchy-theme-set` repoints. We read the palette from (in order) `colors.toml`,
//! `alacritty.toml`, `kitty.conf` or `ghostty.conf`, map it onto the roles the UI uses, and
//! re-read whenever the symlink target or file changes so tern follows theme switches live.

use ratatui::style::Color;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub bg: Color,
    pub fg: Color,
    pub muted: Color,
    pub subtle: Color,
    pub accent: Color,
    pub sel_bg: Color,
    pub surface: Color,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub blue: Color,
    pub magenta: Color,
    pub cyan: Color,
    pub quote: [Color; 4],
}

/// Raw palette: background, foreground, accent, selection and the 16 ANSI colors.
#[derive(Default, Debug)]
struct Palette {
    bg: Option<Color>,
    fg: Option<Color>,
    accent: Option<Color>,
    sel_bg: Option<Color>,
    sel_fg: Option<Color>,
    ansi: [Option<Color>; 16],
}

impl Theme {
    /// Tokyo Night, Omarchy's default, for machines without Omarchy.
    pub fn fallback() -> Self {
        let mut p = Palette::default();
        let hex = [
            "#32344a", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#ad8ee6", "#449dab", "#787c99",
            "#444b6a", "#ff7a93", "#b9f27c", "#ff9e64", "#7da6ff", "#bb9af7", "#0db9d7", "#acb0d0",
        ];
        for (i, h) in hex.iter().enumerate() {
            p.ansi[i] = parse_color(h);
        }
        p.bg = parse_color("#1a1b26");
        p.fg = parse_color("#a9b1d6");
        p.sel_bg = parse_color("#7aa2f7");
        Self::from_palette("tokyo-night (built in)", p)
    }

    fn from_palette(name: &str, p: Palette) -> Self {
        let fb = |i: usize, d: Color| p.ansi[i].unwrap_or(d);
        let bg = p.bg.unwrap_or(Color::Black);
        let fg = p.fg.unwrap_or(Color::White);
        let blue = fb(4, Color::Blue);
        let accent = p.accent.unwrap_or(blue);
        let red = fb(1, Color::Red);
        let green = fb(2, Color::Green);
        let yellow = fb(3, Color::Yellow);
        let magenta = fb(5, Color::Magenta);
        let cyan = fb(6, Color::Cyan);
        // Selection: a quiet tint of the accent over the background reads better in a dense
        // list than the theme's full-strength selection color.
        let sel_bg = mix(bg, p.sel_bg.unwrap_or(accent), 0.28);
        let muted = mix(fg, bg, 0.45);
        let subtle = mix(fg, bg, 0.72);
        let surface = mix(bg, fg, 0.06);
        Theme {
            name: name.to_string(),
            bg,
            fg,
            muted,
            subtle,
            accent,
            sel_bg,
            surface,
            red,
            green,
            yellow,
            blue,
            magenta,
            cyan,
            quote: [mix(blue, bg, 0.15), mix(magenta, bg, 0.2), mix(cyan, bg, 0.25), mix(green, bg, 0.3)],
        }
    }

    pub fn load(dir: Option<&Path>) -> Self {
        let Some(dir) = dir.map(PathBuf::from).or_else(default_dir) else {
            return Self::fallback();
        };
        // fedcandor's rendered palette names its theme; Omarchy's is named by its folder.
        let name = std::fs::read_to_string(dir.join("colors.toml"))
            .ok()
            .and_then(|t| t.parse::<toml::Table>().ok())
            .and_then(|t| t.get("name").and_then(|v| v.as_str()).map(str::to_string))
            .or_else(|| {
                std::fs::canonicalize(&dir)
                    .ok()
                    .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
            })
            .unwrap_or_else(|| "custom".into());
        let readers: [(&str, fn(&str) -> Palette); 4] = [
            ("colors.toml", read_colors_toml),
            ("alacritty.toml", read_alacritty),
            ("kitty.conf", read_kitty),
            ("ghostty.conf", read_ghostty),
        ];
        for (file, read) in readers {
            if let Ok(text) = std::fs::read_to_string(dir.join(file)) {
                let p = read(&text);
                if p.bg.is_some() || p.fg.is_some() || p.ansi.iter().any(|c| c.is_some()) {
                    return Self::from_palette(&name, p);
                }
            }
        }
        Self::fallback()
    }
}

pub fn default_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("TERN_THEME_DIR") {
        return Some(expand(&d));
    }
    fedcandor_dir().or_else(|| {
        let d = dirs::config_dir()?.join("omarchy/current/theme");
        d.exists().then_some(d)
    })
}

/// Where `fedcandor-theme` renders the current palette, if fedcandor is installed.
fn fedcandor_dir() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/state")))?;
    let d = state.join("fedcandor/theme/rendered");
    d.join("colors.toml").is_file().then_some(d)
}

pub fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(p),
    }
}

/// Detects theme changes by polling the resolved directory and file mtimes. Cheap enough to
/// run every second, and it catches `omarchy-theme-set` swapping the symlink and
/// `fedcandor-theme` replacing its rendered `colors.toml`.
pub struct Watcher {
    dir: Option<PathBuf>,
    stamp: Option<(PathBuf, Vec<Option<SystemTime>>)>,
}

impl Watcher {
    pub fn new(dir: Option<PathBuf>) -> Self {
        let mut w = Watcher { dir, stamp: None };
        w.stamp = w.current();
        w
    }

    fn current(&self) -> Option<(PathBuf, Vec<Option<SystemTime>>)> {
        let dir = self.dir.clone().or_else(default_dir)?;
        let real = std::fs::canonicalize(&dir).ok()?;
        let times = ["colors.toml", "alacritty.toml", "kitty.conf", "ghostty.conf"]
            .iter()
            .map(|f| std::fs::metadata(real.join(f)).and_then(|m| m.modified()).ok())
            .collect();
        Some((real, times))
    }

    pub fn changed(&mut self) -> bool {
        let now = self.current();
        if now != self.stamp {
            self.stamp = now;
            true
        } else {
            false
        }
    }
}

pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim().trim_matches(|c| c == '"' || c == '\'');
    let hex = s.strip_prefix('#').or_else(|| s.strip_prefix("0x")).unwrap_or(s);
    if hex.len() < 6 || !hex[..6].chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some(Color::Rgb(v(0)?, v(2)?, v(4)?))
}

fn rgb(c: Color) -> Option<(f32, f32, f32)> {
    match c {
        Color::Rgb(r, g, b) => Some((r as f32, g as f32, b as f32)),
        _ => None,
    }
}

/// Blend `a` toward `b` by `t` (0 = a, 1 = b). Non-RGB colors pass through.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    match (rgb(a), rgb(b)) {
        (Some((r1, g1, b1)), Some((r2, g2, b2))) => Color::Rgb(
            (r1 + (r2 - r1) * t).round() as u8,
            (g1 + (g2 - g1) * t).round() as u8,
            (b1 + (b2 - b1) * t).round() as u8,
        ),
        _ => a,
    }
}

const ANSI_NAMES: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];

/// Omarchy's theme-generation file: flat `key = "#hex"` pairs.
fn read_colors_toml(text: &str) -> Palette {
    let mut p = Palette::default();
    let Ok(table) = text.parse::<toml::Table>() else { return p };
    let get = |k: &str| table.get(k).and_then(|v| v.as_str()).and_then(parse_color);
    p.bg = get("background");
    p.fg = get("foreground");
    p.accent = get("accent");
    p.sel_bg = get("selection_background");
    p.sel_fg = get("selection_foreground");
    for i in 0..16 {
        p.ansi[i] = get(&format!("color{i}"));
    }
    p
}

fn read_alacritty(text: &str) -> Palette {
    let mut p = Palette::default();
    let Ok(table) = text.parse::<toml::Table>() else { return p };
    let Some(colors) = table.get("colors").and_then(|c| c.as_table()) else { return p };
    let get = |sec: &str, k: &str| {
        colors
            .get(sec)
            .and_then(|s| s.get(k))
            .and_then(|v| v.as_str())
            .and_then(parse_color)
    };
    p.bg = get("primary", "background");
    p.fg = get("primary", "foreground");
    p.sel_bg = get("selection", "background");
    p.sel_fg = get("selection", "text");
    for (i, n) in ANSI_NAMES.iter().enumerate() {
        p.ansi[i] = get("normal", n);
        p.ansi[i + 8] = get("bright", n);
    }
    p
}

fn kv_lines(text: &str, sep: char) -> HashMap<String, String> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| {
            let l = l.trim();
            let (k, v) = if sep == ' ' { l.split_once(char::is_whitespace)? } else { l.split_once(sep)? };
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

fn read_kitty(text: &str) -> Palette {
    let kv = kv_lines(text, ' ');
    let get = |k: &str| kv.get(k).and_then(|v| parse_color(v));
    let mut p = Palette {
        bg: get("background"),
        fg: get("foreground"),
        accent: get("active_border_color"),
        sel_bg: get("selection_background"),
        sel_fg: get("selection_foreground"),
        ..Default::default()
    };
    for i in 0..16 {
        p.ansi[i] = get(&format!("color{i}"));
    }
    p
}

fn read_ghostty(text: &str) -> Palette {
    let mut p = Palette::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "background" => p.bg = parse_color(v),
            "foreground" => p.fg = parse_color(v),
            "selection-background" => p.sel_bg = parse_color(v),
            "selection-foreground" => p.sel_fg = parse_color(v),
            "palette" => {
                if let Some((i, c)) = v.split_once('=') {
                    if let Ok(i) = i.trim().parse::<usize>() {
                        if i < 16 {
                            p.ansi[i] = parse_color(c);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_alacritty_hex_forms() {
        let p = read_alacritty(
            "[colors.primary]\nbackground = '0x1a1b26'\nforeground = \"#a9b1d6\"\n[colors.normal]\nblue = \"#7aa2f7\"\n",
        );
        assert_eq!(p.bg, Some(Color::Rgb(0x1a, 0x1b, 0x26)));
        assert_eq!(p.fg, Some(Color::Rgb(0xa9, 0xb1, 0xd6)));
        assert_eq!(p.ansi[4], Some(Color::Rgb(0x7a, 0xa2, 0xf7)));
    }

    #[test]
    fn parses_colors_toml_accent() {
        let p = read_colors_toml("accent = \"#ff0000\"\nbackground = \"#000000\"\ncolor4 = \"#0000ff\"\n");
        assert_eq!(p.accent, Some(Color::Rgb(255, 0, 0)));
        assert_eq!(p.ansi[4], Some(Color::Rgb(0, 0, 255)));
    }

    #[test]
    fn names_theme_from_fedcandor_palette() {
        let dir = std::env::temp_dir().join(format!("tern-theme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("colors.toml"), "name = \"candor\"\nbackground = \"#141d2b\"\n").unwrap();
        let t = Theme::load(Some(&dir));
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(t.name, "candor");
        assert_eq!(t.bg, Color::Rgb(0x14, 0x1d, 0x2b));
    }

    #[test]
    fn parses_kitty() {
        let p = read_kitty("# c\nbackground #101010\ncolor1   #ff0000\n");
        assert_eq!(p.bg, Some(Color::Rgb(16, 16, 16)));
        assert_eq!(p.ansi[1], Some(Color::Rgb(255, 0, 0)));
    }
}
