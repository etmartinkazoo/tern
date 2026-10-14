//! The composer's safety net: while you write, the message is saved to disk every few
//! seconds, so a crash, a closed terminal or a quit never loses it. `gr` brings it back.

use crate::compose::Draft;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
pub struct Saved {
    pub title: String,
    /// The sending account, by name.
    pub account: Option<String>,
    pub draft: Draft,
    /// Unix seconds.
    pub saved_at: i64,
}

fn path() -> PathBuf {
    dirs::data_dir().unwrap_or_default().join("tern/draft.json")
}

pub fn save(s: &Saved) {
    let p = path();
    if let Ok(bytes) = serde_json::to_vec(s) {
        let _ = crate::private::write(&p, bytes);
    }
}

pub fn load() -> Option<Saved> {
    serde_json::from_slice(&std::fs::read(path()).ok()?).ok()
}

pub fn clear() {
    let _ = std::fs::remove_file(path());
}
