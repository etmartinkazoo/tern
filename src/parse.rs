//! Turning raw RFC 822 bytes into the models the UI draws.

use crate::model::{Attachment, Body, Envelope};
use chrono::{DateTime, Local};
use mail_parser::{Address, HeaderValue, MessageParser, MimeHeaders, PartType};

pub fn addr_list(a: Option<&Address>) -> String {
    let Some(a) = a else { return String::new() };
    a.iter()
        .map(|x| match (x.name(), x.address()) {
            (Some(n), Some(e)) if !n.is_empty() => format!("{n} <{e}>"),
            (_, Some(e)) => e.to_string(),
            (Some(n), None) => n.to_string(),
            _ => String::new(),
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn ids(v: &HeaderValue) -> String {
    match v {
        HeaderValue::Text(t) => format!("<{t}>"),
        HeaderValue::TextList(l) => l.iter().map(|t| format!("<{t}>")).collect::<Vec<_>>().join(" "),
        _ => String::new(),
    }
}

fn id_list(v: &HeaderValue) -> Vec<String> {
    match v {
        HeaderValue::Text(t) => vec![t.to_string()],
        HeaderValue::TextList(l) => l.iter().map(|t| t.to_string()).collect(),
        _ => vec![],
    }
}

fn ts(t: i64) -> Option<DateTime<Local>> {
    DateTime::from_timestamp(t, 0).map(|d| d.with_timezone(&Local))
}

pub fn envelope(
    uid: u32,
    header: &[u8],
    flags: &[String],
    internal: Option<DateTime<Local>>,
) -> Envelope {
    let has = |f: &str| flags.iter().any(|x| x.eq_ignore_ascii_case(f));
    let mut env = Envelope {
        uid,
        from_name: String::new(),
        from_addr: String::new(),
        to: String::new(),
        subject: String::new(),
        date: internal,
        seen: has("\\Seen"),
        flagged: has("\\Flagged"),
        answered: has("\\Answered"),
        has_attachment: false,
        message_id: String::new(),
        acct: 0,
        in_reply_to: String::new(),
        references: Vec::new(),
    };
    let Some(msg) = MessageParser::default().parse_headers(header) else { return env };
    if let Some(a) = msg.from().and_then(|a| a.first()) {
        env.from_name = a.name().unwrap_or("").to_string();
        env.from_addr = a.address().unwrap_or("").to_string();
    }
    env.to = addr_list(msg.to());
    env.subject = clean(msg.subject().unwrap_or("(no subject)"));
    if let Some(d) = msg.date().and_then(|d| ts(d.to_timestamp())) {
        env.date = Some(d);
    }
    env.message_id = msg.message_id().unwrap_or("").to_string();
    env.in_reply_to = id_list(msg.in_reply_to()).into_iter().next().unwrap_or_default();
    env.references = id_list(msg.references());
    if let Some(ct) = msg.root_part().content_type() {
        env.has_attachment = ct.ctype().eq_ignore_ascii_case("multipart")
            && ct.subtype().is_some_and(|s| s.eq_ignore_ascii_case("mixed"));
    }
    env
}

/// Collapse folded whitespace so subjects sit on one line.
fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn body(uid: u32, raw: Vec<u8>) -> Body {
    let parsed = MessageParser::default().parse(&raw[..]);
    let Some(msg) = parsed else {
        return Body {
            uid,
            from: String::new(),
            reply_to: String::new(),
            to: String::new(),
            cc: String::new(),
            subject: String::new(),
            date: String::new(),
            message_id: String::new(),
            references: String::new(),
            text: String::from_utf8_lossy(&raw).into_owned(),
            html: None,
            attachments: vec![],
            list_unsubscribe: None,
            invite: None,
            raw,
        };
    };

    let mut plain = Vec::new();
    let mut html_parts = Vec::new();
    for part in msg.text_bodies() {
        match &part.body {
            PartType::Text(t) => plain.push(t.to_string()),
            PartType::Html(h) => html_parts.push(h.to_string()),
            _ => {}
        }
    }
    let html = msg.body_html(0).map(|h| h.into_owned());
    let text = if !plain.is_empty() {
        plain.join("\n\n")
    } else if let Some(h) = html.as_deref().or(html_parts.first().map(|s| s.as_str())) {
        html_to_text(h)
    } else {
        String::new()
    };

    // Meeting invitations travel as a text/calendar part (sometimes an .ics attachment).
    let invite = msg
        .parts
        .iter()
        .find(|p| {
            let ct = p.content_type();
            let cal = ct.is_some_and(|c| c.ctype().eq_ignore_ascii_case("text") && c.subtype().is_some_and(|s| s.eq_ignore_ascii_case("calendar")))
                || ct.is_some_and(|c| c.subtype().is_some_and(|s| s.eq_ignore_ascii_case("ics")))
                || p.attachment_name().is_some_and(|n| n.to_lowercase().ends_with(".ics"));
            cal && String::from_utf8_lossy(p.contents()).contains("BEGIN:VEVENT")
        })
        .map(|p| String::from_utf8_lossy(p.contents()).into_owned());

    let attachments = msg
        .attachments()
        // The invitation is shown as a card, not as a file.
        .filter(|p| !p.content_type().is_some_and(|c| c.subtype().is_some_and(|s| s.eq_ignore_ascii_case("calendar"))))
        .map(|p| Attachment {
            name: p.attachment_name().unwrap_or("unnamed").to_string(),
            size: p.contents().len(),
        })
        .collect();

    let list_unsubscribe = match msg.list_unsubscribe() {
        HeaderValue::Address(a) => a.first().and_then(|x| x.address()).map(|s| s.to_string()),
        HeaderValue::Text(t) => Some(t.to_string()),
        _ => None,
    };

    let mut references = ids(msg.references());
    let msgid = msg.message_id().map(|m| format!("<{m}>")).unwrap_or_default();
    if !msgid.is_empty() {
        if !references.is_empty() {
            references.push(' ');
        }
        references.push_str(&msgid);
    }

    let b = Body {
        uid,
        from: addr_list(msg.from()),
        reply_to: addr_list(msg.reply_to()),
        to: addr_list(msg.to()),
        cc: addr_list(msg.cc()),
        subject: clean(msg.subject().unwrap_or("")),
        date: msg
            .date()
            .and_then(|d| ts(d.to_timestamp()))
            .map(|d| d.format("%a %e %b %Y %H:%M").to_string())
            .unwrap_or_default(),
        message_id: msgid,
        references,
        text: tidy(&text),
        html,
        attachments,
        list_unsubscribe,
        invite,
        raw: Vec::new(),
    };
    drop(msg);
    Body { raw, ..b }
}

pub fn html_to_text(html: &str) -> String {
    html2text::from_read(html.as_bytes(), 10_000)
}

/// Normalise line endings, strip trailing space and squeeze runs of blank lines that
/// HTML-to-text conversion and marketing mail love to produce.
fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank = 0;
    for line in s.replace("\r\n", "\n").lines() {
        let line = line.trim_end().replace('\u{a0}', " ").replace(['\u{200b}', '\u{200c}', '\u{feff}'], "");
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
            out.push('\n');
        } else {
            blank = 0;
            out.push_str(&line);
            out.push('\n');
        }
    }
    out.trim_matches('\n').to_string()
}

/// The web links in a message, in reading order, without repeats: every `href` in
/// the HTML part, then any bare http(s) URL in the text part.
pub fn links(text: &str, html: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |u: String| {
        if (u.starts_with("http://") || u.starts_with("https://")) && u.len() > 8 && !out.contains(&u) {
            out.push(u);
        }
    };
    if let Some(h) = html {
        let lower = h.to_ascii_lowercase();
        let mut at = 0;
        while let Some(i) = lower[at..].find("href") {
            let rest = &h[at + i + 4..];
            at += i + 4;
            let rest = rest.trim_start();
            let Some(rest) = rest.strip_prefix('=') else { continue };
            let rest = rest.trim_start();
            let (q, rest) = match rest.chars().next() {
                Some(c @ ('"' | '\'')) => (Some(c), &rest[1..]),
                _ => (None, rest),
            };
            let end = match q {
                Some(c) => rest.find(c),
                None => rest.find(|c: char| c.is_whitespace() || c == '>'),
            };
            let Some(end) = end else { continue };
            add(unescape(rest[..end].trim()));
        }
    }
    let starts: std::collections::BTreeSet<usize> =
        ["http://", "https://"].iter().flat_map(|p| text.match_indices(p).map(|(i, _)| i)).collect();
    for start in starts {
        let rest = &text[start..];
        let end = rest.find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '[' | ']')).unwrap_or(rest.len());
        let mut u = &rest[..end];
        // Sentence punctuation after a URL, and a closing bracket it doesn't open.
        loop {
            let t = u.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'']);
            let t = if t.ends_with(')') && t.matches('(').count() < t.matches(')').count() { &t[..t.len() - 1] } else { t };
            if t.len() == u.len() {
                break;
            }
            u = t;
        }
        add(u.to_string());
    }
    out
}

/// The HTML entities that turn up inside an href.
fn unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("&#38;", "&").replace("&quot;", "\"").replace("&#61;", "=").replace("&#x3D;", "=")
}

/// Bytes of the n-th attachment and its file name.
pub fn attachment_bytes(raw: &[u8], n: usize) -> Option<(String, Vec<u8>)> {
    let msg = MessageParser::default().parse(raw)?;
    let p = msg.attachment(n)?;
    Some((p.attachment_name().unwrap_or("attachment").to_string(), p.contents().to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_envelope() {
        let h = b"From: =?UTF-8?Q?J=C3=BCrgen?= <j@x.de>\r\nSubject: Hello\r\n  world\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n";
        let e = envelope(7, h, &["\\Seen".into()], None);
        assert_eq!(e.from_name, "Jürgen");
        assert_eq!(e.subject, "Hello world");
        assert!(e.seen && e.has_attachment);
    }

    #[test]
    fn finds_links() {
        let html = r#"<a href="https://a.com/x?y=1&amp;z=2">A</a> <A HREF='http://b.org'>B</A> <a href="mailto:m@x.com">m</a> <a href=https://c.io/p>c</a> <a href="https://a.com/x?y=1&amp;z=2">again</a>"#;
        assert_eq!(links("", Some(html)), ["https://a.com/x?y=1&z=2", "http://b.org", "https://c.io/p"]);
        let text = "See https://d.dev/docs. Or (https://e.net/a_(b)) and <https://f.com/q>, http://g.com!";
        assert_eq!(links(text, None), ["https://d.dev/docs", "https://e.net/a_(b)", "https://f.com/q", "http://g.com"]);
    }
}
