//! The outbox: every message waits here before it goes, so a send can be undone for a
//! few seconds, and a send that fails (offline, server down) is kept and retried
//! instead of lost. Each message is two files under ~/.local/share/tern/outbox/: the
//! RFC 822 bytes and a small JSON record, written before anything is attempted.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Queued {
    pub id: String,
    /// The account it is sent from, by name.
    pub account: String,
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    /// Unix seconds; nothing is attempted before this (the undo window, or a retry).
    pub send_after: i64,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub last_error: Option<String>,
    /// The message this answers, to flag \Answered once sent: (account, folder, uid).
    #[serde(default)]
    pub answers: Option<(String, String, u32)>,
    /// A server draft to delete once sent: (account, folder, uid).
    #[serde(default)]
    pub replaces_draft: Option<(String, String, u32)>,
}

pub fn dir() -> PathBuf {
    dirs::data_dir().unwrap_or_default().join("tern/outbox")
}

fn meta_path(id: &str) -> PathBuf {
    dir().join(format!("{id}.json"))
}

pub fn eml_path(id: &str) -> PathBuf {
    dir().join(format!("{id}.eml"))
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Save a message to the outbox (bytes first, then the record that makes it visible).
pub fn put(q: &Queued, raw: &[u8]) -> Result<()> {
    std::fs::create_dir_all(dir())?;
    std::fs::write(eml_path(&q.id), raw).context("writing to the outbox")?;
    save(q)
}

pub fn save(q: &Queued) -> Result<()> {
    let tmp = meta_path(&q.id).with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(q)?)?;
    std::fs::rename(tmp, meta_path(&q.id))?;
    Ok(())
}

pub fn raw(id: &str) -> Result<Vec<u8>> {
    std::fs::read(eml_path(id)).context("reading a queued message")
}

pub fn remove(id: &str) {
    let _ = std::fs::remove_file(meta_path(id));
    let _ = std::fs::remove_file(eml_path(id));
}

/// Everything queued, oldest first.
pub fn list() -> Vec<Queued> {
    let Ok(rd) = std::fs::read_dir(dir()) else { return vec![] };
    let mut v: Vec<Queued> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
        .filter(|q: &Queued| eml_path(&q.id).exists())
        .collect();
    v.sort_by_key(|q| q.send_after);
    v
}

/// Wait before the next attempt: 30s, 2m, 10m, then every 30m.
pub fn backoff(attempts: u32) -> i64 {
    match attempts {
        0 | 1 => 30,
        2 => 120,
        3 => 600,
        _ => 1800,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows() {
        assert!(backoff(1) < backoff(2) && backoff(2) < backoff(3) && backoff(3) < backoff(9));
    }
}
