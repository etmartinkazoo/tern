//! Contacts: synced from CardDAV (see `dav`) into a local cache so completion is instant
//! and works offline.

use crate::config::cache_dir;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Contact {
    pub name: String,
    pub emails: Vec<String>,
    #[serde(default)]
    pub org: String,
}

impl Contact {
    pub fn mailbox(&self, i: usize) -> String {
        let e = &self.emails[i];
        if self.name.is_empty() {
            e.clone()
        } else if self.name.contains([',', '"', '<', '>', '@', ';']) {
            format!("\"{}\" <{e}>", self.name.replace('"', ""))
        } else {
            format!("{} <{e}>", self.name)
        }
    }
}

fn cache_file() -> std::path::PathBuf {
    cache_dir().join("contacts.json")
}

pub fn load_cache() -> Vec<Contact> {
    std::fs::read(cache_file())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn cache_age() -> Option<std::time::Duration> {
    std::fs::metadata(cache_file()).ok()?.modified().ok()?.elapsed().ok()
}

pub fn save_cache(c: &[Contact]) -> Result<()> {
    std::fs::create_dir_all(cache_dir())?;
    std::fs::write(cache_file(), serde_json::to_vec(c)?)?;
    Ok(())
}

/// Every (contact, email) pair matching `q`, best first: prefix matches on name or address
/// beat substring matches.
pub fn query<'a>(contacts: &'a [Contact], q: &str) -> Vec<(&'a Contact, usize)> {
    let q = q.trim().to_lowercase();
    let mut hits: Vec<(u8, &Contact, usize)> = Vec::new();
    for c in contacts {
        let name = c.name.to_lowercase();
        for (i, e) in c.emails.iter().enumerate() {
            let e = e.to_lowercase();
            let score = if q.is_empty() {
                2
            } else if name.starts_with(&q) || e.starts_with(&q) || name.split(' ').any(|w| w.starts_with(&q)) {
                0
            } else if name.contains(&q) || e.contains(&q) || c.org.to_lowercase().contains(&q) {
                1
            } else {
                continue;
            };
            hits.push((score, c, i));
        }
    }
    hits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    hits.into_iter().map(|(_, c, i)| (c, i)).collect()
}

pub fn parse_vcards(text: &str) -> Vec<Contact> {
    // Unfold continuation lines first (RFC 6350 §3.2).
    let mut lines: Vec<String> = Vec::new();
    for raw in text.replace("\r\n", "\n").split('\n') {
        if (raw.starts_with(' ') || raw.starts_with('\t')) && !lines.is_empty() {
            lines.last_mut().unwrap().push_str(&raw[1..]);
        } else {
            lines.push(raw.to_string());
        }
    }
    let unesc = |s: &str| s.replace("\\,", ",").replace("\\;", ";").replace("\\n", " ").replace("\\\\", "\\");
    let mut out = Vec::new();
    let mut cur: Option<Contact> = None;
    let mut n_field = String::new();
    for line in lines {
        let Some((key, value)) = line.split_once(':') else { continue };
        let prop = key.split(';').next().unwrap_or("").rsplit('.').next().unwrap_or("").to_uppercase();
        match prop.as_str() {
            "BEGIN" if value.eq_ignore_ascii_case("VCARD") => {
                cur = Some(Contact { name: String::new(), emails: vec![], org: String::new() });
                n_field.clear();
            }
            "END" if value.eq_ignore_ascii_case("VCARD") => {
                if let Some(mut c) = cur.take() {
                    if c.name.is_empty() && !n_field.is_empty() {
                        let parts: Vec<&str> = n_field.split(';').collect();
                        c.name = format!("{} {}", parts.get(1).unwrap_or(&""), parts[0]).trim().to_string();
                    }
                    out.push(c);
                }
            }
            "FN" => {
                if let Some(c) = cur.as_mut() {
                    c.name = unesc(value.trim());
                }
            }
            "N" => n_field = value.to_string(),
            "ORG" => {
                if let Some(c) = cur.as_mut() {
                    c.org = unesc(value.split(';').next().unwrap_or("").trim());
                }
            }
            "EMAIL" => {
                if let Some(c) = cur.as_mut() {
                    let e = value.trim().trim_start_matches("mailto:").to_string();
                    if !e.is_empty() && !c.emails.contains(&e) {
                        c.emails.push(e);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_folded_vcard() {
        let v = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Ada\r\n  Lovelace\r\nitem1.EMAIL;TYPE=work:ada@ex.com\r\nEMAIL:a2@ex.com\r\nEND:VCARD\r\n";
        let c = parse_vcards(v);
        assert_eq!(c[0].name, "Ada Lovelace");
        assert_eq!(c[0].emails, vec!["ada@ex.com", "a2@ex.com"]);
    }

    #[test]
    fn ranks_prefix_first() {
        let cs = vec![
            Contact { name: "Bob Annie".into(), emails: vec!["b@x".into()], org: "".into() },
            Contact { name: "Ann Lee".into(), emails: vec!["ann@x".into()], org: "".into() },
            Contact { name: "Joanna".into(), emails: vec!["j@x".into()], org: "".into() },
        ];
        let r = query(&cs, "ann");
        assert_eq!(r[0].0.name, "Ann Lee");
        assert_eq!(r.len(), 3);
    }
}
