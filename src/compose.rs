//! Writing mail the mutt way: a plain-text file with headers on top, opened in $EDITOR.

use crate::config::{Account, Tls};
use crate::model::Body;
use anyhow::{anyhow, bail, Context, Result};
use lettre::message::header::ContentType;
use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct Draft {
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub body: String,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
    /// Files from disk.
    pub attach_paths: Vec<PathBuf>,
    /// Attachments carried over when forwarding: (name, bytes).
    pub carried: Vec<(String, Vec<u8>)>,
    /// Which flag to set on the source message once sent (`\Answered` for replies).
    pub source: Option<(usize, String, u32)>,
    /// The server draft this message continues, removed once it's sent.
    pub replaces: Option<(usize, String, u32)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Reply,
    ReplyAll,
    Forward,
}

/// Quote for a reply. Paragraphs stay one line each (no re-wrapping), so phones and
/// desktop clients each wrap the quote to their own width.
fn quote(text: &str) -> String {
    text.lines()
        .map(|l| if l.starts_with('>') { format!(">{l}") } else if l.is_empty() { ">".into() } else { format!("> {l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

fn prefixed(prefix: &str, subject: &str) -> String {
    let lower = subject.to_lowercase();
    let p = prefix.to_lowercase();
    if lower.starts_with(&p) || (p == "fwd:" && lower.starts_with("fw:")) {
        subject.to_string()
    } else {
        format!("{prefix} {subject}")
    }
}

fn split_addrs(s: &str) -> Vec<String> {
    // Commas inside quoted display names must not split.
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c)
            }
            ',' if !quoted => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear()
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn bare(addr: &str) -> String {
    match (addr.rfind('<'), addr.rfind('>')) {
        (Some(a), Some(b)) if a < b => addr[a + 1..b].to_lowercase(),
        _ => addr.trim().to_lowercase(),
    }
}

pub fn respond(kind: Kind, b: &Body, me: &Account, acct: usize, folder: &str) -> Draft {
    let sig = me.signature.as_deref().map(|s| format!("\n\n{s}")).unwrap_or_default();
    match kind {
        Kind::Forward => {
            let carried = (0..b.attachments.len())
                .filter_map(|i| crate::parse::attachment_bytes(&b.raw, i))
                .collect();
            Draft {
                subject: prefixed("Fwd:", &b.subject),
                body: format!(
                    "{sig}\n\n---------- Forwarded message ----------\nFrom: {}\nDate: {}\nSubject: {}\nTo: {}\n\n{}",
                    b.from, b.date, b.subject, b.to, b.text
                ),
                carried,
                ..Default::default()
            }
        }
        Kind::Reply | Kind::ReplyAll => {
            let to = if b.reply_to.is_empty() { b.from.clone() } else { b.reply_to.clone() };
            let mut cc = String::new();
            if kind == Kind::ReplyAll {
                let mine = me.email.to_lowercase();
                let skip: Vec<String> = split_addrs(&to).iter().map(|a| bare(a)).collect();
                let rest: Vec<String> = split_addrs(&b.to)
                    .into_iter()
                    .chain(split_addrs(&b.cc))
                    .filter(|a| bare(a) != mine && !skip.contains(&bare(a)))
                    .collect();
                cc = rest.join(", ");
            }
            let who = b.from.split('<').next().unwrap_or(&b.from).trim().trim_matches('"');
            Draft {
                to,
                cc,
                subject: prefixed("Re:", &b.subject),
                body: format!("{sig}\n\nOn {}, {} wrote:\n{}", b.date, who, quote(&b.text)),
                in_reply_to: Some(b.message_id.clone()).filter(|s| !s.is_empty()),
                references: Some(b.references.clone()).filter(|s| !s.is_empty()),
                source: Some((acct, folder.to_string(), b.uid)),
                ..Default::default()
            }
        }
    }
}

impl Draft {
    pub fn to_file_text(&self, from: &str) -> String {
        let mut s = format!(
            "From: {from}\nTo: {}\nCc: {}\nBcc: {}\nSubject: {}\n",
            self.to, self.cc, self.bcc, self.subject
        );
        for p in &self.attach_paths {
            s.push_str(&format!("Attach: {}\n", p.display()));
        }
        s.push('\n');
        s.push_str(&self.body);
        if !self.body.ends_with('\n') {
            s.push('\n');
        }
        s
    }

    /// Read the edited file back. Headers end at the first blank line.
    pub fn update_from_text(&mut self, text: &str) {
        let (head, body) = text.split_once("\n\n").unwrap_or((text, ""));
        self.attach_paths.clear();
        for line in head.lines() {
            let Some((k, v)) = line.split_once(':') else { continue };
            let v = v.trim().to_string();
            match k.trim().to_lowercase().as_str() {
                "to" => self.to = v,
                "cc" => self.cc = v,
                "bcc" => self.bcc = v,
                "subject" => self.subject = v,
                "attach" if !v.is_empty() => self.attach_paths.push(crate::theme::expand(&v)),
                _ => {}
            }
        }
        self.body = body.trim_end().to_string() + "\n";
    }

    pub fn build(&self, me: &Account) -> Result<Message> {
        let from: Mailbox = me.sender_mailbox().parse().context("parsing your From address")?;
        let mut m = Message::builder().from(from).subject(self.subject.clone());
        let mut any = false;
        for (field, list) in [("To", &self.to), ("Cc", &self.cc), ("Bcc", &self.bcc)] {
            for a in split_addrs(list) {
                let mb: Mailbox = a.parse().map_err(|e| anyhow!("{field}: `{a}`: {e}"))?;
                any = true;
                m = match field {
                    "To" => m.to(mb),
                    "Cc" => m.cc(mb),
                    _ => m.bcc(mb),
                };
            }
        }
        if !any {
            bail!("no recipients");
        }
        if let Some(r) = &self.in_reply_to {
            m = m.in_reply_to(r.clone());
        }
        if let Some(r) = &self.references {
            m = m.references(r.clone());
        }
        let text = SinglePart::plain(self.body.clone());
        if self.attach_paths.is_empty() && self.carried.is_empty() {
            return Ok(m.singlepart(text)?);
        }
        let mut mp = MultiPart::mixed().singlepart(text);
        for p in &self.attach_paths {
            let bytes = std::fs::read(p).with_context(|| format!("attaching {}", p.display()))?;
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            mp = mp.singlepart(Attachment::new(name.clone()).body(bytes, mime_for(&name)));
        }
        for (name, bytes) in &self.carried {
            mp = mp.singlepart(Attachment::new(name.clone()).body(bytes.clone(), mime_for(name)));
        }
        Ok(m.multipart(mp)?)
    }
}

impl Draft {
    /// For saving drafts: recipients may still be missing, which `build` refuses.
    pub fn build_lenient(&self, me: &Account) -> Result<Vec<u8>> {
        if let Ok(m) = self.build(me) {
            return Ok(m.formatted());
        }
        let date = chrono::Local::now().to_rfc2822();
        let mut raw = format!(
            "From: {}\r\nDate: {date}\r\nSubject: {}\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n",
            me.sender_mailbox(),
            self.subject
        );
        for (k, v) in [("To", &self.to), ("Cc", &self.cc), ("Bcc", &self.bcc)] {
            if !v.trim().is_empty() {
                raw.push_str(&format!("{k}: {v}\r\n"));
            }
        }
        raw.push_str("\r\n");
        raw.push_str(&self.body.replace('\n', "\r\n"));
        Ok(raw.into_bytes())
    }
}

/// A message's text back as a draft (editing something queued or saved).
pub fn draft_from_raw(raw: &[u8]) -> Draft {
    let Some(m) = mail_parser::MessageParser::default().parse(raw) else { return Draft::default() };
    let list = |a: Option<&mail_parser::Address>| crate::parse::addr_list(a);
    let carried = m
        .attachments()
        .map(|p| {
            use mail_parser::MimeHeaders;
            (p.attachment_name().unwrap_or("attachment").to_string(), p.contents().to_vec())
        })
        .collect();
    Draft {
        to: list(m.to()),
        cc: list(m.cc()),
        bcc: list(m.bcc()),
        subject: m.subject().unwrap_or("").to_string(),
        body: m.body_text(0).map(|t| t.into_owned()).unwrap_or_default(),
        in_reply_to: match m.in_reply_to() {
            mail_parser::HeaderValue::Text(t) => Some(format!("<{t}>")),
            _ => None,
        },
        references: match m.references() {
            mail_parser::HeaderValue::Text(t) => Some(format!("<{t}>")),
            mail_parser::HeaderValue::TextList(l) => Some(l.iter().map(|t| format!("<{t}>")).collect::<Vec<_>>().join(" ")),
            _ => None,
        },
        carried,
        ..Default::default()
    }
}

/// The answer to a meeting invitation: a short note for people, and the iTIP REPLY
/// that calendar software reads (RFC 6047).
pub fn invite_reply(me: &Account, to: &str, subject: &str, text: &str, ics: String) -> Result<Message> {
    let from: Mailbox = me.sender_mailbox().parse().context("parsing your From address")?;
    let to: Mailbox = to.parse().map_err(|e| anyhow!("organizer `{to}`: {e}"))?;
    let cal = SinglePart::builder()
        .header(ContentType::parse("text/calendar; method=REPLY; charset=UTF-8").map_err(|e| anyhow!("{e}"))?)
        .body(ics);
    Ok(Message::builder()
        .from(from)
        .to(to)
        .subject(subject)
        .multipart(MultiPart::alternative().singlepart(SinglePart::plain(text.to_string())).singlepart(cal))?)
}

fn mime_for(name: &str) -> ContentType {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    let t = match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "txt" | "md" | "log" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "zip" => "application/zip",
        "json" => "application/json",
        "ics" => "text/calendar",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    };
    ContentType::parse(t).unwrap_or(ContentType::TEXT_PLAIN)
}

/// Send, returning the formatted message so it can be filed in Sent.
pub async fn send(me: &Account, msg: Message) -> Result<Vec<u8>> {
    let raw = msg.formatted();
    let env = msg.envelope().clone();
    transport(me).await?.send_raw(&env, &raw).await.context("SMTP send")?;
    Ok(raw)
}

/// Send bytes already formatted (from the outbox) to an explicit envelope.
pub async fn send_raw(me: &Account, from: &str, to: &[String], raw: &[u8]) -> Result<()> {
    let from: lettre::Address = from.parse().map_err(|e| anyhow!("sender `{from}`: {e}"))?;
    let to: Vec<lettre::Address> =
        to.iter().map(|t| t.parse().map_err(|e| anyhow!("recipient `{t}`: {e}"))).collect::<Result<_>>()?;
    let env = lettre::address::Envelope::new(Some(from), to).map_err(|e| anyhow!("{e}"))?;
    transport(me).await?.send_raw(&env, raw).await.context("SMTP send")?;
    Ok(())
}

async fn transport(me: &Account) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
    let c = &me.smtp;
    let (creds, mechanisms) = match crate::auth::login(me, crate::auth::Proto::Smtp).await? {
        crate::auth::Login::Password { user, password } => (Credentials::new(user, password), None),
        crate::auth::Login::Token { user, token } => (Credentials::new(user, token), Some(vec![Mechanism::Xoauth2])),
    };
    let builder = match c.tls {
        Tls::Implicit => AsyncSmtpTransport::<Tokio1Executor>::relay(&c.host)?,
        Tls::Starttls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&c.host)?,
        Tls::None => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(c.host.clone()),
    };
    let mut builder = builder.port(c.port).credentials(creds);
    if let Some(m) = mechanisms {
        builder = builder.authentication(m);
    }
    Ok(builder.build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_file() {
        let mut d = Draft { to: "a@b.c".into(), subject: "Hi".into(), body: "yo\n".into(), ..Default::default() };
        let t = d.to_file_text("me <me@x.y>");
        d.to.clear();
        d.update_from_text(&t.replace("Cc: ", "Cc: \"Doe, J\" <j@d.e>"));
        assert_eq!(d.to, "a@b.c");
        assert_eq!(split_addrs(&d.cc), vec!["\"Doe, J\" <j@d.e>"]);
        assert_eq!(d.body, "yo\n");
    }

    #[test]
    fn long_paragraphs_go_out_as_one_line() {
        let me: Account = toml::from_str(
            "name='t'\nemail='me@x.y'\n[imap]\nhost='h'\nusername='u'\npassword='p'\n[smtp]\nhost='h'\nusername='u'\npassword='p'\n",
        )
        .unwrap();
        let para = "This sentence is deliberately long so that it runs well past seventy-two columns and would have been hard-wrapped before. ".repeat(3);
        let body = format!("{}\n\nThanks,\nTed\n", para.trim_end());
        let d = Draft { to: "a@b.c".into(), subject: "s".into(), body: body.clone(), ..Default::default() };
        let raw = d.build(&me).unwrap().formatted();
        let msg = mail_parser::MessageParser::default().parse(&raw[..]).unwrap();
        let text = msg.body_text(0).unwrap().replace("\r\n", "\n");
        assert_eq!(text.trim_end(), body.trim_end());
        assert_eq!(text.lines().next().unwrap(), para.trim_end());
    }

    #[test]
    fn subjects() {
        assert_eq!(prefixed("Re:", "RE: x"), "RE: x");
        assert_eq!(prefixed("Fwd:", "Fw: x"), "Fw: x");
        assert_eq!(prefixed("Re:", "x"), "Re: x");
    }
}
