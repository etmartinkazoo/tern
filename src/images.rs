//! Pictures in messages, drawn with the terminal's graphics protocol: kitty's (kitty,
//! Ghostty, WezTerm), sixel (foot, xterm) or iTerm2's, and half blocks anywhere else.

use crate::model::Event;
use image::imageops::FilterType;
use image::DynamicImage;
use ratatui::layout::Rect;
use ratatui::Frame;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::{Image, Resize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

/// Decoded pictures kept around; a message with more than this many thrashes.
const KEEP: usize = 64;
/// Wider pictures are scaled down once decoded; no pane is this many pixels wide.
const MAX_PX: u32 = 2400;
const MAX_FETCH: usize = 15 << 20;

pub fn hash(data: impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    data.hash(&mut h);
    h.finish()
}

pub enum Source {
    Bytes(Arc<Vec<u8>>),
    Url(String),
}

/// What a picture looks like right now, sized for the room it has.
pub enum State {
    Loading,
    /// Spacers and tracking pixels: nothing to show.
    Hidden,
    Failed,
    Ready { cols: u16, rows: u16 },
}

enum Slot {
    Loading,
    Hidden,
    Failed,
    Ready(Ready),
}

struct Ready {
    img: DynamicImage,
    /// The picture scaled to its cell size, then the encoded slices of it on screen.
    sized: Option<((u16, u16), DynamicImage)>,
    shown: HashMap<(u16, u16, u16, u16), Protocol>,
}

pub struct Images {
    /// None until the terminal has been asked what it can draw.
    pub picker: Option<Picker>,
    slots: HashMap<u64, Slot>,
    order: VecDeque<u64>,
    /// Message-IDs (no angle brackets) whose remote pictures you asked for (I).
    pub remote: HashSet<String>,
}

impl Images {
    pub fn new() -> Self {
        Self { picker: None, slots: HashMap::new(), order: VecDeque::new(), remote: HashSet::new() }
    }

    /// A new terminal reading (after a resize, which is also how a font size change shows
    /// up): slices made for the old cell size are no good.
    pub fn set_picker(&mut self, p: Picker) {
        if self.picker.as_ref().is_some_and(|old| old.font_size() == p.font_size() && old.protocol_type() == p.protocol_type()) {
            return;
        }
        for slot in self.slots.values_mut() {
            if let Slot::Ready(r) = slot {
                r.sized = None;
                r.shown.clear();
            }
        }
        self.picker = Some(p);
    }

    /// Starts loading a picture unless it's loaded or on its way.
    pub fn want(&mut self, id: u64, src: impl FnOnce() -> Option<Source>, tx: &UnboundedSender<Event>) {
        if self.slots.contains_key(&id) {
            return;
        }
        let Some(src) = src() else {
            self.slots.insert(id, Slot::Failed);
            return;
        };
        self.slots.insert(id, Slot::Loading);
        self.order.push_back(id);
        while self.order.len() > KEEP {
            if let Some(old) = self.order.pop_front() {
                self.slots.remove(&old);
            }
        }
        let tx = tx.clone();
        tokio::spawn(async move {
            let bytes = match src {
                Source::Bytes(b) => Some(b),
                Source::Url(u) => fetch(&u).await.map(Arc::new),
            };
            let img = match bytes {
                Some(b) => tokio::task::spawn_blocking(move || decode(&b)).await.ok().flatten(),
                None => None,
            };
            let _ = tx.send(Event::Image(id, img));
        });
    }

    pub fn loaded(&mut self, id: u64, img: Option<DynamicImage>) {
        // Dropped from the cache while it loaded.
        let Some(slot) = self.slots.get_mut(&id) else { return };
        *slot = match img {
            None => Slot::Failed,
            Some(i) if i.width() < 24 || i.height() < 24 => Slot::Hidden,
            Some(img) => Slot::Ready(Ready { img, sized: None, shown: HashMap::new() }),
        };
    }

    /// The picture's state, fitted within `max_cols` × `max_rows` cells and never enlarged.
    pub fn state(&self, id: u64, max_cols: u16, max_rows: u16) -> State {
        let (Some(p), Some(slot)) = (&self.picker, self.slots.get(&id)) else { return State::Loading };
        match slot {
            Slot::Loading => State::Loading,
            Slot::Hidden => State::Hidden,
            Slot::Failed => State::Failed,
            Slot::Ready(r) => {
                let (fw, fh) = p.font_size();
                let (w, h) = (r.img.width() as f64, r.img.height() as f64);
                let s = (max_cols as f64 * fw as f64 / w).min(max_rows as f64 * fh as f64 / h).min(1.0);
                let cols = (w * s / fw as f64).ceil().max(1.0) as u16;
                let rows = (h * s / fh as f64).ceil().max(1.0) as u16;
                State::Ready { cols, rows }
            }
        }
    }

    /// Draws rows `skip..skip + area.height` of a picture laid out `size` cells big, so a
    /// picture half scrolled off the pane shows the half that's still on it.
    pub fn render(&mut self, f: &mut Frame, id: u64, size: (u16, u16), skip: u16, area: Rect) {
        let Some(p) = &self.picker else { return };
        let Some(Slot::Ready(r)) = self.slots.get_mut(&id) else { return };
        let (fw, fh) = (p.font_size().0 as u32, p.font_size().1 as u32);
        let key = (size.0, size.1, skip, area.height);
        if !r.shown.contains_key(&key) {
            if r.sized.as_ref().map(|s| s.0) != Some(size) {
                let s = r.img.resize(size.0 as u32 * fw, size.1 as u32 * fh, FilterType::Triangle);
                r.sized = Some((size, s));
                r.shown.clear();
            }
            let Some((_, s)) = &r.sized else { return };
            let part = s.crop_imm(0, skip as u32 * fh, s.width(), area.height as u32 * fh);
            let Ok(proto) = p.new_protocol(part, Rect::new(0, 0, size.0, area.height), Resize::Fit(None)) else { return };
            // Scrolling makes a new slice every step; keep only a few.
            if r.shown.len() >= 4 {
                r.shown.clear();
            }
            r.shown.insert(key, proto);
        }
        if let Some(proto) = r.shown.get(&key) {
            f.render_widget(Image::new(proto), Rect { width: size.0.min(area.width), ..area });
        }
    }
}

/// How this terminal draws pictures, worked out without asking it: ratatui-image's query
/// leaves a thread reading stdin when a terminal doesn't answer, which eats keys and drops
/// raw mode. TERM and friends name the terminal; the window-size ioctl gives the cell size.
pub fn picker(bg: ratatui::style::Color) -> Picker {
    let font = crossterm::terminal::window_size()
        .ok()
        .filter(|w| w.width > 0 && w.height > 0 && w.columns > 0 && w.rows > 0)
        .map(|w| (w.width / w.columns, w.height / w.rows))
        .unwrap_or((10, 20));
    let mut p = Picker::from_fontsize(font);
    if p.protocol_type() == ProtocolType::Halfblocks {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        let (term, prog) = (var("TERM"), var("TERM_PROGRAM"));
        let kind = if term == "xterm-kitty" || term == "xterm-ghostty" || prog == "ghostty" || !var("KITTY_WINDOW_ID").is_empty() {
            Some(ProtocolType::Kitty)
        } else if term.starts_with("foot") || !var("KONSOLE_VERSION").is_empty() {
            Some(ProtocolType::Sixel)
        } else if prog == "iTerm.app" || prog == "WezTerm" {
            Some(ProtocolType::Iterm2)
        } else {
            None
        };
        if let Some(k) = kind {
            p.set_protocol_type(k);
        }
    }
    if let ratatui::style::Color::Rgb(r, g, b) = bg {
        p.set_background_color([r, g, b, 255]);
    }
    p
}

fn decode(b: &[u8]) -> Option<DynamicImage> {
    let img = image::load_from_memory(b).ok()?;
    Some(if img.width() > MAX_PX { img.resize(MAX_PX, u32::MAX, FilterType::Triangle) } else { img })
}

async fn fetch(url: &str) -> Option<Vec<u8>> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(20)).build().ok()?;
    let resp = client.get(url).send().await.ok()?.error_for_status().ok()?;
    if resp.content_length().is_some_and(|n| n as usize > MAX_FETCH) {
        return None;
    }
    let b = resp.bytes().await.ok()?;
    (b.len() <= MAX_FETCH).then(|| b.to_vec())
}

/// A `data:image/…;base64,…` URI's bytes.
pub fn data_uri(src: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let rest = src.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    if !meta.ends_with(";base64") {
        return None;
    }
    let clean: String = data.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD.decode(clean).ok()
}
