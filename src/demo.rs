//! `tern --demo`: an in-memory account so the UI can be explored (and screenshotted)
//! without a mail server.

use crate::model::{Attachment, Body, Cmd, Envelope, Event, Folder, Role};
use chrono::{Duration, Local};
use std::collections::HashMap;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

const PEOPLE: &[(&str, &str)] = &[
    ("Maya Okafor", "maya@northwind.io"),
    ("GitHub", "notifications@github.com"),
    ("Linear", "updates@linear.app"),
    ("Jonas Berg", "jonas@berg.dev"),
    ("Priya Raman", "priya@finch.co"),
    ("Stripe", "receipts@stripe.com"),
    ("Hacker Newsletter", "kale@hackernewsletter.com"),
    ("Sam Whitaker", "sam@whitaker.studio"),
    ("Fastmail", "support@fastmail.com"),
    ("Lena Fischer", "lena@kleinwerk.de"),
];

const SUBJECTS: &[&str] = &[
    "Re: Q4 roadmap — final cut before Friday",
    "[omarchy] feat: theme hot-reload for terminal apps (#1204)",
    "ENG-482 Sync engine drops flags on reconnect",
    "Coffee next week?",
    "Contract draft v3 attached",
    "Your receipt from Northwind #2291-0042",
    "Hacker Newsletter #712",
    "Re: Re: Logo directions",
    "Scheduled maintenance: Oct 18, 02:00 UTC",
    "Fwd: Berlin offsite logistics",
    "Invoice overdue — gentle nudge",
    "Design review notes",
];

fn body_for(e: &Envelope) -> String {
    let s = &e.subject;
    let kind = if s.contains("[omarchy]") || s.starts_with("ENG-") {
        1
    } else if s.contains("Coffee") || s.contains("Logo") || s.contains("Berlin") {
        2
    } else if s.contains("Contract") || s.contains("Invoice") || s.contains("receipt") {
        3
    } else {
        0
    };
    match kind {
        0 => format!(
            "Hey,\n\nPulled the numbers together — short version: we can ship the sync rewrite and the new compose flow, but not both alongside the billing migration. I'd cut billing to January.\n\nThree things I need from you before Friday:\n\n  1. Sign-off on dropping the calendar beta\n  2. Whether Lena's team can absorb the CardDAV work\n  3. A yes/no on the offsite\n\nFull doc: https://docs.northwind.io/roadmap/q4-final\n\nOn Mon, Oct 13, 2026 at 9:14 AM, you wrote:\n> Can we get a one-pager on what actually fits? I don't need the\n> spreadsheet, just the call.\n>\n> On Sun, Oct 12, Jonas Berg wrote:\n>> Flagging that sync is still the riskiest piece. Two weeks of\n>> buffer minimum.\n\nThanks,\n{}\n\n-- \n{}\nNorthwind · https://northwind.io",
            e.from_name, e.from_name
        ),
        1 => "@tedmartin requested your review on this pull request.\n\nThis adds an inotify watch on ~/.config/omarchy/current/theme so terminal apps can re-read colors without a restart.\n\nCommit summary\n  * watch the theme symlink, not the directory\n  * debounce 150ms so omarchy-theme-set's burst of writes is one reload\n  * fall back to polling on filesystems without inotify\n\nFile changes\n  M bin/omarchy-theme-set (12)\n  A lib/theme-watch.sh (48)\n\n— \nReply to this email directly, view it on GitHub:\nhttps://github.com/basecamp/omarchy/pull/1204".into(),
        2 => "Hi!\n\nAre you around next Tuesday or Wednesday? I'm in town for the conference and would love to catch up properly — it's been way too long.\n\nThere's a place near the river that does a ridiculous cortado. Morning works best for me, anything before 11.\n\nL.".into(),
        _ => "Please find the revised contract attached. Changes from v2:\n\n- Payment terms moved to net-30\n- Clause 7.2 (IP assignment) narrowed to deliverables only\n- Added the termination-for-convenience language you asked for\n\nIf this looks right, sign where flagged and send it back. If not, mark it up and we'll do one more pass.\n\nBest,\nPriya".into(),
    }
}

const REPLIES: &[&str] = &[
    "Works for me. Let's lock it in and revisit after the sync milestone.",
    "Agree with Maya — billing can wait. I'll update the plan tonight.",
    "Can we get 15 minutes tomorrow to settle this? Morning is best.",
    "Sent the revised numbers. Short version: it fits if we drop the beta.",
    "+1. Also flagging that Lena's team is out the week of the 20th.",
];

fn demo_body(v: &[Envelope], uid: u32) -> Option<Body> {
    let (i, e) = v.iter().enumerate().find(|(_, e)| e.uid == uid)?;
    let attachments = if e.has_attachment || i % 4 == 3 {
        vec![Attachment { name: "contract-v3.pdf".into(), size: 248_000 }]
    } else {
        vec![]
    };
    let text = match v.iter().find(|p| p.message_id == e.in_reply_to) {
        Some(p) => format!(
            "{}\n\n{}\n\nOn {}, {} wrote:\n{}",
            REPLIES[uid as usize % REPLIES.len()],
            e.from_name.split(' ').next().unwrap_or(""),
            p.date.map(|d| d.format("%a %b %e").to_string()).unwrap_or_default(),
            p.from_name,
            body_for(p).lines().take(6).map(|l| format!("> {l}")).collect::<Vec<_>>().join("\n")
        ),
        None => body_for(e),
    };
    Some(Body {
        uid,
        from: format!("{} <{}>", e.from_name, e.from_addr),
        reply_to: String::new(),
        to: e.to.clone(),
        cc: if i % 3 == 0 { "Jonas Berg <jonas@berg.dev>, Lena Fischer <lena@kleinwerk.de>".into() } else { String::new() },
        subject: e.subject.clone(),
        date: e.date.map(|d| d.format("%a %e %b %Y %H:%M").to_string()).unwrap_or_default(),
        message_id: format!("<{}>", e.message_id),
        references: String::new(),
        text,
        html: None,
        attachments,
        list_unsubscribe: None,
        invite: None,
        raw: vec![],
    })
}

pub fn spawn(idx: usize, mut rx: UnboundedReceiver<Cmd>, tx: UnboundedSender<Event>) {
    tokio::spawn(async move {
        let now = Local::now();
        let mut boxes: HashMap<String, Vec<Envelope>> = HashMap::new();
        let names = ["INBOX", "Archive", "Sent", "Drafts", "Trash", "Junk", "Clients", "Clients/Northwind", "Receipts"];
        let mut uid = 1000;
        for (bi, name) in names.iter().enumerate() {
            let n = match bi {
                0 => 48,
                1 => 120,
                2 => 30,
                _ => 6 + bi * 2,
            };
            let mut v = Vec::new();
            for i in 0..n {
                uid += 1;
                let (pn, pe) = PEOPLE[(i * 7 + bi) % PEOPLE.len()];
                v.push(Envelope {
                    uid,
                    from_name: pn.into(),
                    from_addr: pe.into(),
                    to: "Ted Martin <ted@example.com>".into(),
                    subject: SUBJECTS[(i * 5 + bi) % SUBJECTS.len()].into(),
                    date: Some(now - Duration::minutes((i as i64).pow(2) * 23 + 7)),
                    seen: bi != 0 || i > 6,
                    flagged: i % 9 == 2,
                    answered: i % 5 == 3,
                    has_attachment: i % 6 == 4,
                    message_id: format!("{uid}@demo"),
                    acct: 0,
                    in_reply_to: String::new(),
                    references: vec![],
                });
            }
            if bi == 0 {
                // Conversations: message i answers i+12, which answers i+24 (same subject,
                // older), and you wrote some of the replies.
                for i in 0..24 {
                    let parent = v[i + 12].message_id.clone();
                    let base = v[i + 12].subject.trim_start_matches("Re: ").to_string();
                    let e = &mut v[i];
                    e.in_reply_to = parent.clone();
                    e.references = vec![parent];
                    e.subject = format!("Re: {base}");
                    if i % 4 == 1 {
                        e.from_name = "Ted Martin".into();
                        e.from_addr = "ted@example.com".into();
                        e.seen = true;
                    }
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
                    if let Some(body) = boxes.get(&folder).and_then(|v| demo_body(v, uid)) {
                        let _ = tx.send(Event::Body { acct: idx, folder, body: Box::new(body) });
                    }
                }
                Cmd::Bodies { folder, uids } => {
                    for uid in uids {
                        if let Some(body) = boxes.get(&folder).and_then(|v| demo_body(v, uid)) {
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
