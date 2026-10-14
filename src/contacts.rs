//! Contacts: synced from CardDAV (see `dav`) into a local cache so completion is instant
//! and works offline.

use crate::config::cache_dir;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Contact {
    pub name: String,
    pub emails: Vec<String>,
    #[serde(default)]
    pub org: String,
    #[serde(default)]
    pub phones: Vec<String>,
    #[serde(default)]
    pub note: String,
    /// Where it lives: [[dav]] source, address book, card resource, its ETag, and the
    /// card as stored (edits keep everything tern doesn't touch).
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub book: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub raw: String,
}

/// What the contact form edits.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Spec {
    pub name: String,
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub org: String,
    pub note: String,
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace(',', "\\,").replace(';', "\\;").replace('\n', "\\n")
}

fn spec_lines(s: &Spec) -> Vec<String> {
    let (given, family) = s.name.rsplit_once(' ').unwrap_or((s.name.as_str(), ""));
    let mut v = vec![format!("FN:{}", esc(&s.name)), format!("N:{};{};;;", esc(family), esc(given))];
    for e in &s.emails {
        v.push(format!("EMAIL;TYPE=INTERNET:{e}"));
    }
    for p in &s.phones {
        v.push(format!("TEL:{p}"));
    }
    if !s.org.is_empty() {
        v.push(format!("ORG:{}", esc(&s.org)));
    }
    if !s.note.trim().is_empty() {
        v.push(format!("NOTE:{}", esc(s.note.trim())));
    }
    v
}

/// A new vCard 3.0.
pub fn vcard_new(s: &Spec, uid: &str) -> String {
    let mut lines = vec!["BEGIN:VCARD".to_string(), "VERSION:3.0".into(), format!("UID:{uid}")];
    lines.extend(spec_lines(s));
    lines.push("END:VCARD".into());
    lines.join("\r\n") + "\r\n"
}

/// The stored card with tern's fields replaced and everything else kept.
pub fn vcard_edit(raw: &str, s: &Spec) -> String {
    let mut lines: Vec<String> = Vec::new();
    for l in raw.replace("\r\n", "\n").split('\n') {
        if (l.starts_with(' ') || l.starts_with('\t')) && !lines.is_empty() {
            lines.last_mut().unwrap().push_str(&l[1..]);
        } else if !l.is_empty() {
            lines.push(l.to_string());
        }
    }
    let ours = ["FN", "N", "EMAIL", "TEL", "ORG", "NOTE"];
    let prop = |l: &str| l.split([';', ':']).next().unwrap_or("").rsplit('.').next().unwrap_or("").to_uppercase();
    let mut out: Vec<String> = lines.into_iter().filter(|l| !ours.contains(&prop(l).as_str())).collect();
    let end = out.iter().rposition(|l| l.eq_ignore_ascii_case("END:VCARD")).unwrap_or(out.len());
    for (i, l) in spec_lines(s).into_iter().enumerate() {
        out.insert(end + i, l);
    }
    out.join("\r\n") + "\r\n"
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
                cur = Some(Contact::default());
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
            "TEL" => {
                if let Some(c) = cur.as_mut() {
                    let t = value.trim().trim_start_matches("tel:").to_string();
                    if !t.is_empty() {
                        c.phones.push(t);
                    }
                }
            }
            "NOTE" => {
                if let Some(c) = cur.as_mut() {
                    c.note = value.trim().replace("\\n", "\n").replace("\\,", ",").replace("\\;", ";");
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
    fn edits_keep_other_fields() {
        let raw = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:u1\r\nFN:Ada\r\nitem1.EMAIL;TYPE=work:a@x\r\nitem1.X-ABLabel:work\r\nBDAY:1815-12-10\r\nPHOTO;VALUE=uri:https://x/p.jpg\r\nEND:VCARD\r\n";
        let s = Spec { name: "Ada Lovelace".into(), emails: vec!["ada@x".into(), "a@x".into()], phones: vec!["+1 555".into()], org: "Analytical".into(), note: "met at, the club".into() };
        let out = vcard_edit(raw, &s);
        for keep in ["UID:u1", "BDAY:1815-12-10", "PHOTO;VALUE=uri:https://x/p.jpg", "END:VCARD"] {
            assert!(out.contains(keep), "{out}");
        }
        let c = &parse_vcards(&out)[0];
        assert_eq!(c.name, "Ada Lovelace");
        assert_eq!(c.emails, vec!["ada@x", "a@x"]);
        assert_eq!(c.phones, vec!["+1 555"]);
        assert_eq!(c.note, "met at, the club");
        assert!(out.trim_end().ends_with("END:VCARD"));
        let n = &parse_vcards(&vcard_new(&s, "u2"))[0];
        assert_eq!((n.name.as_str(), n.org.as_str()), ("Ada Lovelace", "Analytical"));
    }

    #[test]
    fn ranks_prefix_first() {
        let cs = vec![
            Contact { name: "Bob Annie".into(), emails: vec!["b@x".into()], ..Default::default() },
            Contact { name: "Ann Lee".into(), emails: vec!["ann@x".into()], ..Default::default() },
            Contact { name: "Joanna".into(), emails: vec!["j@x".into()], ..Default::default() },
        ];
        let r = query(&cs, "ann");
        assert_eq!(r[0].0.name, "Ann Lee");
        assert_eq!(r.len(), 3);
    }
}
