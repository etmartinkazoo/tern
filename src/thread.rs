//! Conversations: grouping messages by Message-ID / In-Reply-To / References, falling back
//! to "Re:" subjects for mailers that drop the headers.

use crate::model::Envelope;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct Thread {
    /// UID of the root message; stable key for remembering folds.
    pub root: u32,
    /// Indices into the message list, depth-first (the order the tree is drawn in).
    pub members: Vec<usize>,
    pub depths: Vec<usize>,
    /// Tree glyphs per member (`├─`, `└─`, `│ `), empty for the root.
    pub trees: Vec<String>,
    /// Members oldest first, for the conversation view.
    pub chrono: Vec<usize>,
    pub latest: usize,
    pub unread: usize,
    pub flagged: bool,
    pub attachment: bool,
    /// "Maya, Jonas, me"
    pub participants: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// A message on its own.
    Single,
    /// A whole conversation folded into one row.
    Collapsed,
    /// First message of an unfolded conversation.
    Head,
    /// A reply inside an unfolded conversation.
    Child,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub msg: usize,
    /// Index into the thread list (`usize::MAX` when not threading).
    pub thread: usize,
    pub member: usize,
    pub kind: RowKind,
}

/// Lower-cased subject without Re:/Fwd:/[tags] noise, and whether a reply prefix was there.
pub fn normalize_subject(s: &str) -> (String, bool) {
    let mut s = s.trim();
    let mut replied = false;
    loop {
        let lower = s.to_lowercase();
        let cut = ["re:", "fwd:", "fw:", "aw:", "sv:", "antw:", "re :"]
            .iter()
            .find(|p| lower.starts_with(*p))
            .map(|p| p.len());
        if let Some(n) = cut {
            replied = true;
            s = s[n..].trim_start();
            continue;
        }
        // "Re[2]:" style
        if lower.starts_with("re[") {
            if let Some(i) = s.find("]:") {
                replied = true;
                s = s[i + 2..].trim_start();
                continue;
            }
        }
        break;
    }
    (s.to_lowercase(), replied)
}

fn clean_id(s: &str) -> String {
    s.trim().trim_start_matches('<').trim_end_matches('>').to_string()
}

fn first_name(e: &Envelope, me: &str) -> String {
    if !me.is_empty() && e.from_addr.eq_ignore_ascii_case(me) {
        return "me".into();
    }
    let n = e.sender().trim().trim_matches('"');
    if n.contains('@') {
        n.split('@').next().unwrap_or(n).to_string()
    } else {
        n.split_whitespace().next().unwrap_or(n).to_string()
    }
}

/// Group `subset` (indices into `all`) into threads, newest activity first.
pub fn build(all: &[Envelope], subset: &[usize], me: &str) -> Vec<Thread> {
    let date = |i: usize| all[i].date;
    let mut by_id: HashMap<String, usize> = HashMap::new();
    for &i in subset {
        let id = clean_id(&all[i].message_id);
        if !id.is_empty() {
            by_id.entry(id).or_insert(i);
        }
    }

    let mut parent: HashMap<usize, usize> = HashMap::new();
    // Walking up must never loop, whatever broken headers say.
    let creates_cycle = |parent: &HashMap<usize, usize>, child: usize, mut p: usize| {
        let mut steps = 0;
        loop {
            if p == child {
                return true;
            }
            match parent.get(&p) {
                Some(&next) if steps < 1000 => {
                    p = next;
                    steps += 1
                }
                _ => return false,
            }
        }
    };
    for &i in subset {
        let e = &all[i];
        let candidates = e.references.iter().rev().chain(std::iter::once(&e.in_reply_to));
        for id in candidates {
            let id = clean_id(id);
            if id.is_empty() {
                continue;
            }
            if let Some(&p) = by_id.get(&id) {
                if p != i && !creates_cycle(&parent, i, p) {
                    parent.insert(i, p);
                    break;
                }
            }
        }
    }

    // Subject fallback: a "Re:" with no known parent joins the oldest root of that subject.
    let mut roots_by_subject: HashMap<String, usize> = HashMap::new();
    let mut oldest_first: Vec<usize> = subset.to_vec();
    oldest_first.sort_by_key(|&i| (date(i), all[i].uid));
    for &i in &oldest_first {
        if parent.contains_key(&i) {
            continue;
        }
        let (subj, replied) = normalize_subject(&all[i].subject);
        if subj.is_empty() {
            continue;
        }
        match roots_by_subject.get(&subj) {
            Some(&root) if replied && root != i => {
                parent.insert(i, root);
            }
            Some(_) => {}
            None => {
                roots_by_subject.insert(subj, i);
            }
        }
    }

    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
    for (&c, &p) in &parent {
        children.entry(p).or_default().push(c);
    }
    for v in children.values_mut() {
        v.sort_by_key(|&i| (date(i), all[i].uid));
    }

    let mut threads = Vec::new();
    for &root in oldest_first.iter().filter(|i| !parent.contains_key(i)) {
        let mut t = Thread {
            root: all[root].uid,
            members: vec![],
            depths: vec![],
            trees: vec![],
            chrono: vec![],
            latest: root,
            unread: 0,
            flagged: false,
            attachment: false,
            participants: String::new(),
        };
        // Depth-first walk carrying, per ancestor level, whether more siblings follow.
        let mut stack: Vec<(usize, usize, Vec<bool>, bool)> = vec![(root, 0, vec![], true)];
        while let Some((i, depth, rails, last)) = stack.pop() {
            let mut tree = String::new();
            if depth > 0 {
                for more in rails.iter().take(depth - 1).take(6) {
                    tree.push_str(if *more { "│ " } else { "  " });
                }
                tree.push_str(if last { "└─" } else { "├─" });
            }
            t.members.push(i);
            t.depths.push(depth);
            t.trees.push(tree);
            if let Some(kids) = children.get(&i) {
                let mut next_rails = rails.clone();
                if depth > 0 {
                    next_rails.push(!last);
                }
                for (k, &c) in kids.iter().enumerate().rev() {
                    stack.push((c, depth + 1, next_rails.clone(), k == kids.len() - 1));
                }
            }
        }
        t.chrono = t.members.clone();
        t.chrono.sort_by_key(|&i| (date(i), all[i].uid));
        t.latest = *t.chrono.last().unwrap();
        t.unread = t.members.iter().filter(|&&i| !all[i].seen).count();
        t.flagged = t.members.iter().any(|&i| all[i].flagged);
        t.attachment = t.members.iter().any(|&i| all[i].has_attachment);
        let mut seen = HashSet::new();
        let names: Vec<String> = t
            .chrono
            .iter()
            .map(|&i| first_name(&all[i], me))
            .filter(|n| seen.insert(n.clone()))
            .collect();
        t.participants = names.join(", ");
        threads.push(t);
    }
    threads.sort_by(|a, b| date(b.latest).cmp(&date(a.latest)).then(all[b.latest].uid.cmp(&all[a.latest].uid)));
    threads
}

/// Rows for the list. `collapsed(root)` says whether a conversation is folded.
pub fn rows(threads: &[Thread], collapsed: impl Fn(u32) -> bool) -> Vec<Row> {
    let mut out = Vec::new();
    for (ti, t) in threads.iter().enumerate() {
        if t.members.len() == 1 {
            out.push(Row { msg: t.members[0], thread: ti, member: 0, kind: RowKind::Single });
        } else if collapsed(t.root) {
            out.push(Row { msg: t.latest, thread: ti, member: 0, kind: RowKind::Collapsed });
        } else {
            for (m, &i) in t.members.iter().enumerate() {
                let kind = if m == 0 { RowKind::Head } else { RowKind::Child };
                out.push(Row { msg: i, thread: ti, member: m, kind });
            }
        }
    }
    out
}

/// Hide the quoted tail of a reply ("On … wrote:" and everything quoted after it) for the
/// conversation view, where the earlier messages are already on screen.
pub fn strip_quoted(text: &str) -> (String, bool) {
    let lines: Vec<&str> = text.lines().collect();
    let is_quote = |l: &str| l.trim_start().starts_with('>');
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim();
        let outlook = t.starts_with("-----Original Message") || t.starts_with("________________________________");
        let attribution = t.ends_with("wrote:") || t.ends_with("schrieb:") || t.ends_with("a écrit :");
        if outlook || (attribution && lines[i + 1..].iter().any(|l| is_quote(l))) {
            let rest_quoted = outlook || lines[i + 1..].iter().all(|l| l.trim().is_empty() || is_quote(l));
            if rest_quoted {
                return (lines[..i].join("\n").trim_end().to_string(), true);
            }
        }
    }
    // A bare quoted block at the end with no attribution line.
    let mut end = lines.len();
    while end > 0 && (lines[end - 1].trim().is_empty() || is_quote(lines[end - 1])) {
        end -= 1;
    }
    if end < lines.len() && lines[end..].iter().any(|l| is_quote(l)) && end > 0 {
        return (lines[..end].join("\n").trim_end().to_string(), true);
    }
    (text.to_string(), false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};

    fn env(uid: u32, id: &str, reply_to: &str, subject: &str, min: i64) -> Envelope {
        Envelope {
            uid,
            from_name: format!("P{uid}"),
            from_addr: format!("p{uid}@x"),
            to: String::new(),
            subject: subject.into(),
            date: Some(Local.timestamp_opt(1_700_000_000 + min * 60, 0).unwrap()),
            seen: uid != 3,
            flagged: false,
            answered: false,
            has_attachment: false,
            message_id: id.into(),
            in_reply_to: reply_to.into(),
            references: if reply_to.is_empty() { vec![] } else { vec![reply_to.into()] },
        }
    }

    #[test]
    fn builds_tree_and_orders_by_activity() {
        let all = vec![
            env(1, "a", "", "Plan", 0),
            env(2, "b", "a", "Re: Plan", 5),
            env(3, "c", "b", "Re: Plan", 10),
            env(4, "d", "a", "Re: Plan", 7),
            env(5, "e", "", "Other", 8),
        ];
        let subset: Vec<usize> = (0..all.len()).collect();
        let t = build(&all, &subset, "");
        assert_eq!(t.len(), 2);
        // The Plan thread had the latest activity (minute 10).
        assert_eq!(t[0].root, 1);
        assert_eq!(t[0].members, vec![0, 1, 2, 3]);
        assert_eq!(t[0].trees, vec!["", "├─", "│ └─", "└─"]);
        assert_eq!(t[0].unread, 1);
        assert_eq!(t[0].latest, 2);
        assert_eq!(t[0].participants, "P1, P2, P4, P3");
    }

    #[test]
    fn falls_back_to_subject_for_replies_only() {
        let all = vec![env(1, "a", "", "Lunch", 0), env(2, "", "", "RE: lunch", 3), env(3, "", "", "Lunch", 4)];
        let t = build(&all, &[0, 1, 2], "");
        // The Re: joins; the second plain "Lunch" stays separate.
        assert_eq!(t.len(), 2);
        assert!(t.iter().any(|t| t.members == vec![0, 1]));
    }

    #[test]
    fn survives_reference_cycles() {
        let all = vec![env(1, "a", "b", "x", 0), env(2, "b", "a", "x", 1)];
        let t = build(&all, &[0, 1], "");
        assert_eq!(t.iter().map(|t| t.members.len()).sum::<usize>(), 2);
    }

    #[test]
    fn collapsed_rows() {
        let all = vec![env(1, "a", "", "Plan", 0), env(2, "b", "a", "Re: Plan", 5)];
        let t = build(&all, &[0, 1], "");
        let r = rows(&t, |_| true);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].kind, RowKind::Collapsed);
        assert_eq!(r[0].msg, 1);
        assert_eq!(rows(&t, |_| false).len(), 2);
    }

    #[test]
    fn strips_quoted_tail() {
        let (s, hid) = strip_quoted("Sounds good.\n\nOn Mon, Maya wrote:\n> earlier\n>> older\n");
        assert_eq!(s, "Sounds good.");
        assert!(hid);
        let (s, hid) = strip_quoted("> inline\nmy answer\n");
        assert!(!hid);
        assert_eq!(s, "> inline\nmy answer\n");
    }

    #[test]
    fn normalizes() {
        assert_eq!(normalize_subject("Re: FW: Re[2]: Hello"), ("hello".into(), true));
        assert_eq!(normalize_subject("Hello"), ("hello".into(), false));
    }
}
