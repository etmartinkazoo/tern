//! Snoozed messages: moved to a Snoozed folder and remembered here (by Message-ID, since
//! moving changes UIDs) until it's time to put them back in the inbox.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snoozed {
    pub account: String,
    pub message_id: String,
    pub subject: String,
    pub from: String,
    /// Unix seconds.
    pub until: i64,
    /// Where it goes back to.
    pub back_to: String,
}

pub const FOLDER: &str = "Snoozed";

fn path() -> PathBuf {
    dirs::data_dir().unwrap_or_default().join("tern/snoozed.json")
}

pub fn load() -> Vec<Snoozed> {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save(v: &[Snoozed]) {
    let p = path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec_pretty(v).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(tmp, p);
    }
}
