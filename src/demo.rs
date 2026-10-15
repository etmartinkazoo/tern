//! `tern --demo`: an in-memory account so the UI can be explored (and screenshotted)
//! without a mail server.

use crate::model::{Attachment, Body, Cmd, Envelope, Event, Folder, Img, Role};
use chrono::{Duration, Local};
use std::collections::HashMap;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

const PEOPLE_ADDR: &[(&str, &str)] = &[
    ("Maya Okafor", "maya@northwind.io"),
    ("Priya Raman", "priya@finch.co"),
    ("Sam Whitaker", "sam@whitaker.studio"),
    ("Lena Fischer", "lena@kleinwerk.de"),
];

/// Who writes what: (name, address, subject, body). The body's `{me}` is the reader.
const CATALOG: &[(&str, &str, &str, &str)] = &[
    ("Maya Okafor", "maya@northwind.io", "Q4 roadmap — final cut before Friday",
     "Hey,\n\nPulled the numbers together — short version: we can ship the sync rewrite and the new compose flow, but not both alongside the billing migration. I'd cut billing to January.\n\nThree things I need from you before Friday:\n\n  1. Sign-off on dropping the calendar beta\n  2. Whether Lena's team can absorb the CardDAV work\n  3. A yes/no on the offsite\n\nFull doc: https://docs.northwind.io/roadmap/q4-final\n\nThanks,\nMaya\n\n-- \nMaya Okafor · Head of Product\nNorthwind · https://northwind.io"),
    ("GitHub", "notifications@github.com", "[omarchy] feat: theme hot-reload for terminal apps (#1204)",
     "@tedmartin requested your review on this pull request.\n\nThis adds an inotify watch on ~/.config/omarchy/current/theme so terminal apps can re-read colors without a restart.\n\nCommit summary:\n  * watch the theme symlink, not its target\n  * debounce to 200ms\n  * docs: document the reload hook\n\nFile changes: 6 files, +214 −38\n\n— Reply to this email directly or view it on GitHub:\nhttps://github.com/basecamp/omarchy/pull/1204"),
    ("Linear", "updates@linear.app", "ENG-482 Sync engine drops flags on reconnect",
     "Jonas Berg moved ENG-482 to In Progress.\n\nWhen the IMAP connection drops mid-STORE, the \\Flagged change is applied locally but never retried. After reconnect the server state wins and the flag disappears.\n\nPriority: High · Cycle 14 · Estimate: 3\n\nhttps://linear.app/northwind/issue/ENG-482"),
    ("Lena Fischer", "lena@kleinwerk.de", "Coffee next week?",
     "Hi!\n\nAre you around next Tuesday or Wednesday? I'm in town for the conference and would love to catch up properly — it's been way too long.\n\nThere's a place near the river that does a great flat white. Morning works best for me.\n\nLena"),
    ("Priya Raman", "priya@finch.co", "Contract draft v3 attached",
     "Hi Ted,\n\nPlease find the revised contract attached. Changes from v2:\n\n- Payment terms moved to net-30\n- Clause 7.2 (IP assignment) narrowed to deliverables only\n- Added the termination-for-convenience clause you asked for\n\nIf this looks right I'll send it for signature on Thursday.\n\nBest,\nPriya"),
    ("Stripe", "receipts@stripe.com", "Your receipt from Northwind #2291-0042",
     "Receipt from Northwind\n\nAmount paid      $1,240.00\nDate paid        Oct 13, 2026\nPayment method   Visa ending in 4242\n\nTeam plan — 10 seats         $1,200.00\nAdditional storage (50 GB)      $40.00\n\nQuestions? Contact billing@northwind.io"),
    ("Hacker Newsletter", "kale@hackernewsletter.com", "Hacker Newsletter #712",
     "Hacker Newsletter #712\n\n#Favorites\n\n  The terminal is the IDE (again)\n  Why we moved our mail stack back to IMAP\n  A tour of CalDAV in 2026\n\n#Code\n\n  Writing a TUI that doesn't flicker\n  Rust async without the tears\n\nUnsubscribe: https://hackernewsletter.com/unsubscribe"),
    ("Sam Whitaker", "sam@whitaker.studio", "Logo directions",
     "Hey Ted,\n\nThree directions for the mark, attached as one PDF:\n\n  A — the bird, geometric, one weight\n  B — wordmark only, tight tracking\n  C — monogram for the app icon\n\nMy vote is A for the site and C for the dock. Happy to jump on a call.\n\nSam"),
    ("Fastmail", "support@fastmail.com", "Scheduled maintenance: Oct 18, 02:00 UTC",
     "We'll be performing scheduled maintenance on Saturday, October 18 from 02:00 to 03:00 UTC.\n\nIMAP, SMTP and CalDAV may be briefly unavailable. Mail sent during the window is queued and delivered afterwards — nothing is lost.\n\nThe Fastmail team"),
    ("Jonas Berg", "jonas@berg.dev", "Fwd: Berlin offsite logistics",
     "FYI — forwarding the hotel details. Flights are on you, everything else is booked.\n\n---------- Forwarded message ----------\nFrom: Kleinwerk Events <events@kleinwerk.de>\nSubject: Berlin offsite logistics\n\nCheck-in: Nov 4 · Hotel Oderberger, Oderberger Str. 57\nDinner: Nov 4, 19:30 · Prater Garten\nWorkshop: Nov 5–6, 09:30 · Kleinwerk studio"),
    ("Finch Accounts", "accounts@finch.co", "Invoice overdue — gentle nudge",
     "Hi Ted,\n\nA friendly reminder that invoice FIN-0342 ($4,800.00) was due on October 1.\n\nIf it's already on its way, please ignore this. Otherwise you can pay here: https://pay.finch.co/FIN-0342\n\nThanks,\nFinch Accounts"),
    ("Sam Whitaker", "sam@whitaker.studio", "Design review notes",
     "Notes from today:\n\n- Sidebar density: keep it, it's the point\n- Unread dot reads well in every theme we tried\n- Compose pane: move the field hints to the status line\n- Next review: Thursday 14:00\n\nSam"),
    ("Dad", "martin.r@icloud.com", "Photos from the weekend",
     "Here are the ones from the lake. Your mother says the one of the dog is going on the fridge.\n\nCall Sunday?\n\nDad"),
    ("Kestrel Travel", "trips@kestrel.travel", "Booking confirmed: Berlin, Nov 4–7",
     "Your trip is confirmed.\n\nOutbound   Tue Nov 4   07:55 → 10:05\nReturn     Fri Nov 7   18:40 → 20:50\nHotel      Hotel Oderberger, 3 nights\n\nBooking reference: KT-88Q2LM"),
    ("Northwind Status", "status@northwind.io", "Weekly uptime report — 99.98%",
     "API          99.99%\nSync         99.97%\nWeb app     100.00%\n\nOne incident: 14 minutes of delayed sync on Tuesday (ENG-482).\n\nhttps://status.northwind.io"),
    ("Ana Souza", "ana@kleinwerk.de", "Intro: Ana ↔ Ted",
     "Hi Ted,\n\nLena suggested I reach out — we're moving our team's calendars to CalDAV and I'd love twenty minutes of your time to hear how you set it up.\n\nAny afternoon next week works.\n\nAna"),
];

/// The inbox, newest first: (catalog entry, minutes ago, replying as, seen, flagged, text).
/// `replying as` None is the original; Some(name) answers the previous message in the
/// same conversation ("" is you).
const INBOX: &[(usize, i64, Option<&str>, bool, bool, &str)] = &[
    (0, 18, Some("Maya Okafor"), false, false, "Agree with Priya — billing waits until January. I'll update the plan tonight and send the one-pager in the morning."),
    (1, 33, None, false, false, ""),
    (4, 95, Some("Priya Raman"), false, true, "Signed copy attached. Thanks for turning this around so fast."),
    (2, 140, None, true, true, ""),
    (0, 190, Some("Priya Raman"), true, false, "Sent the revised numbers. Short version: it fits if we drop the beta."),
    (8, 260, None, true, false, ""),
    (7, 410, Some("Sam Whitaker"), true, false, "Great — going with A. I'll send final files Friday."),
    (5, 520, None, true, false, ""),
    (3, 52, Some("Lena Fischer"), false, false, "Perfect, Wednesday 9:30 it is. See you there!"),
    (6, 980, None, true, false, ""),
    (3, 1250, Some(""), true, false, "Yes! Wednesday morning works. Send me the place?"),
    (3, 1300, None, true, false, ""),
    (0, 1500, Some(""), true, false, "Can we get a one-pager on what actually fits? I don't need the spreadsheet, just the call."),
    (9, 2200, None, true, false, ""),
    (0, 2900, None, true, false, ""),
    (10, 3500, None, true, true, ""),
    (4, 4000, Some(""), true, false, "Looks good. One question on 7.2 — does \"deliverables\" include the design files?"),
    (4, 4300, None, true, false, ""),
    (7, 5800, Some(""), true, false, "Leaning A. Can you try it at 16px?"),
    (7, 6000, None, true, false, ""),
    (11, 7200, None, true, false, ""),
    (12, 8400, None, true, true, ""),
    (13, 9900, None, true, false, ""),
    (14, 11500, None, true, false, ""),
    (15, 13000, None, true, false, ""),
];

fn find(subject: &str) -> Option<&'static (&'static str, &'static str, &'static str, &'static str)> {
    let base = subject.trim_start_matches("Re: ");
    CATALOG.iter().find(|c| c.2 == base)
}

fn demo_body(v: &[Envelope], texts: &HashMap<u32, String>, uid: u32) -> Option<Body> {
    let (i, e) = v.iter().enumerate().find(|(_, e)| e.uid == uid)?;
    let attachments = if e.has_attachment {
        let name = match find(&e.subject).map(|c| c.2) {
            Some(s) if s.starts_with("Logo") => "logo-directions.pdf",
            Some(s) if s.starts_with("Fwd: Berlin") => "offsite-itinerary.pdf",
            Some(s) if s.starts_with("Photos") => "lake-2026.zip",
            _ => "contract-v3.pdf",
        };
        vec![Attachment { name: name.into(), size: 248_000 }]
    } else {
        vec![]
    };
    let mut own = texts.get(&uid).cloned().unwrap_or_else(|| find(&e.subject).map(|c| c.3.to_string()).unwrap_or_default());
    let mut images = vec![];
    if e.subject.starts_with("Photos") {
        own = own.replacen("\n\nCall Sunday?", "\n\n[image: lake.png]\n\nCall Sunday?", 1);
        let data = lake();
        images.push(Img { id: crate::images::hash(&*data), cid: String::new(), name: "lake.png".into(), data });
    }
    let text = match v.iter().find(|p| !e.in_reply_to.is_empty() && p.message_id == e.in_reply_to) {
        Some(p) => {
            let prev = texts.get(&p.uid).cloned().unwrap_or_else(|| find(&p.subject).map(|c| c.3.to_string()).unwrap_or_default());
            format!(
                "{own}\n\n{}\n\nOn {}, {} wrote:\n{}",
                e.from_name.split(' ').next().unwrap_or(""),
                p.date.map(|d| d.format("%a %b %e").to_string()).unwrap_or_default(),
                p.from_name,
                prev.lines().take(6).map(|l| format!("> {l}")).collect::<Vec<_>>().join("\n")
            )
        }
        None => own,
    };
    Some(Body {
        uid,
        from: format!("{} <{}>", e.from_name, e.from_addr),
        reply_to: String::new(),
        to: e.to.clone(),
        cc: if i % 5 == 0 && e.from_addr.ends_with("northwind.io") { "Jonas Berg <jonas@berg.dev>, Priya Raman <priya@finch.co>".into() } else { String::new() },
        subject: e.subject.clone(),
        date: e.date.map(|d| d.format("%a %e %b %Y %H:%M").to_string()).unwrap_or_default(),
        message_id: format!("<{}>", e.message_id),
        references: String::new(),
        text,
        html: None,
        attachments,
        images,
        list_unsubscribe: None,
        invite: None,
        raw: vec![],
    })
}

/// A sunset over a lake, drawn rather than shipped.
fn lake() -> std::sync::Arc<Vec<u8>> {
    static PNG: std::sync::OnceLock<std::sync::Arc<Vec<u8>>> = std::sync::OnceLock::new();
    PNG.get_or_init(|| {
        let (w, h) = (640u32, 400u32);
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let sun = ((fx - 0.62).powi(2) + ((fy - 0.42) * 1.6).powi(2)).sqrt();
            let hill = 0.5 + 0.04 * (fx * 9.0).sin() + 0.03 * (fx * 23.0).cos();
            let px = |r: f32, g: f32, b: f32| image::Rgb([r.min(255.0) as u8, g.min(255.0) as u8, b.min(255.0) as u8]);
            if fy < 0.58 {
                if sun < 0.07 {
                    return px(255.0, 214.0, 140.0);
                }
                if fy > hill {
                    return px(28.0, 40.0, 38.0);
                }
                let t = fy / 0.58;
                let glow = (1.0 - sun * 2.5).max(0.0) * 70.0;
                px(40.0 + 200.0 * t + glow, 50.0 + 90.0 * t + glow * 0.6, 120.0 + 30.0 * t)
            } else {
                let t = (fy - 0.58) / 0.42;
                let streak = (fx - 0.62).abs() < 0.03 + 0.06 * t && (y / 5) % 2 == 0;
                if streak {
                    px(240.0, 170.0, 110.0)
                } else {
                    px(60.0 - 30.0 * t, 70.0 - 30.0 * t, 110.0 - 40.0 * t)
                }
            }
        });
        let mut out = Vec::new();
        let _ = image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png);
        std::sync::Arc::new(out)
    })
    .clone()
}

pub fn spawn(idx: usize, mut rx: UnboundedReceiver<Cmd>, tx: UnboundedSender<Event>) {
    tokio::spawn(async move {
        let now = Local::now();
        let mut boxes: HashMap<String, Vec<Envelope>> = HashMap::new();
        let names = ["INBOX", "Archive", "Sent", "Drafts", "Trash", "Junk", "Clients", "Clients/Northwind", "Receipts"];
        let mut uid = 1000;
        let mut texts: HashMap<u32, String> = HashMap::new();
        for (bi, name) in names.iter().enumerate() {
            let n = match bi {
                0 => 48,
                1 => 120,
                2 => 30,
                _ => 6 + bi * 2,
            };
            let mut v = Vec::new();
            if bi == 0 {
                let mut last: HashMap<usize, String> = HashMap::new();
                // Oldest first, so each reply can point at the message before it.
                for &(c, ago, reply, seen, flagged, text) in INBOX.iter().rev() {
                    uid += 1;
                    let (cn, ca, subject, _) = CATALOG[c];
                    let (from_name, from_addr) = match reply {
                        Some("") => ("Ted Martin", "ted@example.com"),
                        Some(n) => (n, PEOPLE_ADDR.iter().find(|p| p.0 == n).map(|p| p.1).unwrap_or(ca)),
                        None => (cn, ca),
                    };
                    let parent = if reply.is_some() { last.get(&c).cloned().unwrap_or_default() } else { String::new() };
                    let message_id = format!("{uid}@demo");
                    last.insert(c, message_id.clone());
                    if !text.is_empty() {
                        texts.insert(uid, text.to_string());
                    }
                    v.push(Envelope {
                        uid,
                        from_name: from_name.into(),
                        from_addr: from_addr.into(),
                        to: "Ted Martin <ted@example.com>".into(),
                        subject: if reply.is_some() { format!("Re: {subject}") } else { subject.into() },
                        date: Some(now - Duration::minutes(ago)),
                        seen,
                        flagged,
                        answered: reply.is_none() && INBOX.iter().any(|x| x.0 == c && x.2 == Some("")),
                        has_attachment: reply.is_none() && [4, 7, 9, 12].contains(&c),
                        message_id,
                        acct: 0,
                        in_reply_to: parent.clone(),
                        references: if parent.is_empty() { vec![] } else { vec![parent] },
                    });
                }
                v.sort_by(|a, b| b.date.cmp(&a.date));
            } else {
                for i in 0..n {
                    uid += 1;
                    let (pn, pe, subject, _) = CATALOG[(i * 7 + bi) % CATALOG.len()];
                    v.push(Envelope {
                        uid,
                        from_name: if bi == 2 { "Ted Martin".into() } else { pn.into() },
                        from_addr: if bi == 2 { "ted@example.com".into() } else { pe.into() },
                        to: if bi == 2 { format!("{pn} <{pe}>") } else { "Ted Martin <ted@example.com>".into() },
                        subject: subject.into(),
                        date: Some(now - Duration::minutes((i as i64).pow(2) * 23 + 600 * bi as i64)),
                        seen: true,
                        flagged: i % 9 == 2,
                        answered: i % 5 == 3,
                        has_attachment: false,
                        message_id: format!("{uid}@demo"),
                        acct: 0,
                        in_reply_to: String::new(),
                        references: vec![],
                    });
                }
            }
            boxes.insert(name.to_string(), v);
        }
        let _ = tx.send(Event::Connected(idx));
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Cmd::Folders | Cmd::Poll { .. } => {
                    let folders = names
                        .iter()
                        .map(|n| {
                            let v = &boxes[*n];
                            let role = match *n {
                                "INBOX" => Role::Inbox,
                                "Archive" => Role::Archive,
                                "Sent" => Role::Sent,
                                "Drafts" => Role::Drafts,
                                "Trash" => Role::Trash,
                                "Junk" => Role::Junk,
                                _ => Role::Other,
                            };
                            Folder {
                                name: n.to_string(),
                                label: if *n == "INBOX" { "Inbox".into() } else { n.rsplit('/').next().unwrap().into() },
                                depth: n.matches('/').count(),
                                role,
                                unread: v.iter().filter(|e| !e.seen).count() as u32,
                                selectable: true,
                            }
                        })
                        .collect();
                    let _ = tx.send(Event::Folders(idx, folders));
                }
                Cmd::Select { folder } => {
                    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                    let items = boxes.get(&folder).cloned().unwrap_or_default();
                    let total = items.len() as u32;
                    let _ = tx.send(Event::Envelopes { acct: idx, folder, items, total });
                }
                Cmd::Body { folder, uid } => {
                    if let Some(body) = boxes.get(&folder).and_then(|v| demo_body(v, &texts, uid)) {
                        let _ = tx.send(Event::Body { acct: idx, folder, body: Box::new(body) });
                    }
                }
                Cmd::Bodies { folder, uids } => {
                    for uid in uids {
                        if let Some(body) = boxes.get(&folder).and_then(|v| demo_body(v, &texts, uid)) {
                            let _ = tx.send(Event::Body { acct: idx, folder: folder.clone(), body: Box::new(body) });
                        }
                    }
                }
                Cmd::SetFlag { folder, uids, flag, on } => {
                    if let Some(v) = boxes.get_mut(&folder) {
                        for e in v.iter_mut().filter(|e| uids.contains(&e.uid)) {
                            match flag {
                                "\\Seen" => e.seen = on,
                                "\\Flagged" => e.flagged = on,
                                "\\Answered" => e.answered = on,
                                _ => {}
                            }
                        }
                    }
                }
                Cmd::Move { folder, uids, to } => {
                    let moved: Vec<Envelope> = boxes
                        .get_mut(&folder)
                        .map(|v| {
                            let (m, keep): (Vec<_>, Vec<_>) = v.drain(..).partition(|e| uids.contains(&e.uid));
                            *v = keep;
                            m
                        })
                        .unwrap_or_default();
                    if let Some(dst) = boxes.get_mut(&to) {
                        dst.extend(moved);
                        dst.sort_by(|a, b| b.date.cmp(&a.date));
                    }
                    let _ = tx.send(Event::Removed { acct: idx, folder, uids });
                }
                Cmd::Expunge { folder, uids } => {
                    if let Some(v) = boxes.get_mut(&folder) {
                        v.retain(|e| !uids.contains(&e.uid));
                    }
                    let _ = tx.send(Event::Removed { acct: idx, folder, uids });
                }
                Cmd::Append { .. } | Cmd::Ensure { .. } | Cmd::Unsnooze { .. } => {}
                Cmd::Search { folder, query } => {
                    let q = query.to_lowercase();
                    let items = boxes
                        .get(&folder)
                        .map(|v| v.iter().filter(|e| e.subject.to_lowercase().contains(&q)).cloned().collect())
                        .unwrap_or_default();
                    let _ = tx.send(Event::SearchResult { acct: idx, folder, items, query });
                }
            }
        }
    });
}

/// Demo calendars: a work calendar with a recurring standup (one instance moved, one
/// skipped), a personal one, and holidays, all relative to today.
pub fn calendars() -> Vec<crate::dav::CachedCalendar> {
    use chrono::{Datelike, Duration, Local};
    let today = Local::now().date_naive();
    let monday = today - Duration::days(today.weekday().num_days_from_monday() as i64);
    let d = |off: i64| (today + Duration::days(off)).format("%Y%m%d").to_string();
    let ev = |uid: &str, body: &str| {
        format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//tern demo//EN\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:20260101T000000Z\r\n{body}END:VEVENT\r\nEND:VCALENDAR\r\n")
    };
    let at = |off: i64, hm: &str| format!("{}T{hm}00", d(off));
    let standup_moved = (monday + Duration::days(9)).format("%Y%m%d").to_string();
    let standup_skipped = (monday + Duration::days(3)).format("%Y%m%d").to_string();
    let work = vec![
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//tern demo//EN\r\nBEGIN:VEVENT\r\nUID:standup\r\nDTSTAMP:20260101T000000Z\r\n\
DTSTART:{m}T093000\r\nDTEND:{m}T094500\r\nRRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR\r\nEXDATE:{skip}T093000\r\nSUMMARY:Standup\r\n\
LOCATION:https://meet.example.com/standup\r\nORGANIZER;CN=Maya Okafor:mailto:maya@northwind.io\r\n\
ATTENDEE;CN=Jonas Berg:mailto:jonas@berg.dev\r\nATTENDEE;CN=Ted Martin:mailto:ted@example.com\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:standup\r\nDTSTAMP:20260101T000000Z\r\nRECURRENCE-ID:{moved}T093000\r\nDTSTART:{moved}T130000\r\nDTEND:{moved}T131500\r\n\
SUMMARY:Standup (after the all-hands)\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            m = monday.format("%Y%m%d"),
            skip = standup_skipped,
            moved = standup_moved
        ),
        ev("roadmap", &format!("DTSTART:{}\r\nDTEND:{}\r\nSUMMARY:Q4 roadmap review\r\nLOCATION:Room 4 · Northwind HQ\r\nORGANIZER;CN=Maya Okafor:mailto:maya@northwind.io\r\nATTENDEE;CN=Priya Raman:mailto:priya@finch.co\r\nDESCRIPTION:Final cut before Friday. Bring the one-pager: what fits\\, what slips to January\\, and why.\r\n", at(1, "1400"), at(1, "1530"))),
        ev("one-on-one", &format!("DTSTART:{}\r\nDTEND:{}\r\nRRULE:FREQ=WEEKLY;INTERVAL=2\r\nSUMMARY:1:1 with Jonas\r\n", at(-2, "1100"), at(-2, "1130"))),
        ev("design", &format!("DTSTART:{}\r\nDTEND:{}\r\nSUMMARY:Design review — compose flow\r\nLOCATION:https://meet.example.com/design\r\n", at(0, "1600"), at(0, "1700"))),
        ev("offsite", &format!("DTSTART;VALUE=DATE:{}\r\nDTEND;VALUE=DATE:{}\r\nSUMMARY:Berlin offsite\r\nLOCATION:Kleinwerk\\, Berlin\r\n", d(12), d(15))),
    ];
    let personal = vec![
        ev("gym", &format!("DTSTART:{}\r\nDTEND:{}\r\nRRULE:FREQ=WEEKLY;BYDAY=MO,WE,FR\r\nSUMMARY:Gym\r\n", at(0, "0700"), at(0, "0800"))),
        ev("dinner", &format!("DTSTART:{}\r\nDTEND:{}\r\nSUMMARY:Dinner with Lena\r\nLOCATION:Osteria Morini\r\n", at(2, "1930"), at(2, "2130"))),
        ev("coffee", &format!("DTSTART;TZID=Europe/Berlin:{}\r\nDURATION:PT45M\r\nSUMMARY:Coffee with Lena (Berlin time)\r\n", at(13, "1000"))),
    ];
    let holidays = vec![ev("hol", &format!("DTSTART;VALUE=DATE:{}\r\nDTEND;VALUE=DATE:{}\r\nSUMMARY:Public holiday\r\n", d(6), d(7)))];
    let mk = |source: &str, id: &str, name: &str, color: Option<&str>, items: Vec<String>| crate::dav::CachedCalendar {
        source: source.into(),
        id: id.into(),
        name: name.into(),
        color: color.map(String::from),
        sync_token: None,
        items: items.into_iter().enumerate().map(|(i, s)| (format!("{i}.ics"), s)).collect(),
        etags: Default::default(),
        home: String::new(),
        remote: String::new(),
    };
    vec![
        mk("Fastmail", "work", "Work", None, work),
        mk("Fastmail", "personal", "Personal", None, personal),
        mk("iCloud", "holidays", "Holidays", Some("#e0af68"), holidays),
    ]
}
