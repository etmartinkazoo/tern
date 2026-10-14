//! Reading dates and times the way people type them: "fri 12:30 1h", "tomorrow 3pm",
//! "oct 20 all day", "2026-11-02 9-10:30am @Room 4".

use chrono::{Datelike, Duration, NaiveDate, NaiveTime, Weekday};

/// An event as typed, before it becomes iCalendar.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Draft {
    pub title: String,
    pub date: NaiveDate,
    /// None for an all-day event.
    pub start: Option<NaiveTime>,
    pub end: Option<NaiveTime>,
    /// For all-day events: how many days.
    pub days: i64,
    pub location: String,
    pub description: String,
    /// A link for the event (video call, document): the iCalendar URL property.
    pub url: String,
}

const WEEKDAYS: [(&str, Weekday); 7] = [
    ("mon", Weekday::Mon),
    ("tue", Weekday::Tue),
    ("wed", Weekday::Wed),
    ("thu", Weekday::Thu),
    ("fri", Weekday::Fri),
    ("sat", Weekday::Sat),
    ("sun", Weekday::Sun),
];
const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

fn weekday(w: &str) -> Option<Weekday> {
    let w = w.to_lowercase();
    if w.len() < 3 {
        return None;
    }
    WEEKDAYS.iter().find(|(p, _)| w.starts_with(p) && "mondaytuesdaywednesdaythursdayfridaysaturdaysunday".contains(w.as_str())).map(|(_, d)| *d)
}

fn month(w: &str) -> Option<u32> {
    let w = w.to_lowercase();
    if w.len() < 3 {
        return None;
    }
    MONTHS.iter().position(|m| w.starts_with(m)).map(|i| i as u32 + 1)
}

/// The next `day` on or after `from`.
fn next_weekday(from: NaiveDate, day: Weekday) -> NaiveDate {
    let ahead = (day.num_days_from_monday() as i64 - from.weekday().num_days_from_monday() as i64).rem_euclid(7);
    from + Duration::days(ahead)
}

/// One date word (or two, for "oct 20"); returns the date and how many words it used.
pub fn date_at(words: &[&str], today: NaiveDate) -> Option<(NaiveDate, usize)> {
    let w = words.first()?.to_lowercase();
    let w = w.trim_end_matches(',');
    match w {
        "today" => return Some((today, 1)),
        "tomorrow" | "tmrw" | "tmr" => return Some((today + Duration::days(1), 1)),
        "next" => {
            // "next fri": that weekday in the following week.
            let d = weekday(words.get(1)?)?;
            return Some((next_weekday(today, d) + Duration::days(7), 2));
        }
        _ => {}
    }
    if let Some(d) = weekday(w) {
        return Some((next_weekday(today, d), 1));
    }
    if let Ok(d) = NaiveDate::parse_from_str(w, "%Y-%m-%d") {
        return Some((d, 1));
    }
    // "oct 20" / "20 oct"
    let num = |s: &str| s.trim_end_matches(',').trim_end_matches(|c: char| c.is_alphabetic()).parse::<u32>().ok();
    let pick = |m: u32, d: u32| {
        let this = NaiveDate::from_ymd_opt(today.year(), m, d)?;
        // A date that has passed this year means next year.
        Some(if this < today { NaiveDate::from_ymd_opt(today.year() + 1, m, d)? } else { this })
    };
    if let Some(m) = month(w) {
        if let Some(d) = words.get(1).and_then(|s| num(s)) {
            return Some((pick(m, d)?, 2));
        }
    }
    if let (Some(d), Some(m)) = (num(w), words.get(1).and_then(|s| month(s))) {
        if w.chars().next()?.is_ascii_digit() {
            return Some((pick(m, d)?, 2));
        }
    }
    None
}

/// "12:30", "9am", "3:15pm", "15h". Bare numbers are not times (they may be a date).
pub fn time(w: &str) -> Option<NaiveTime> {
    let w = w.to_lowercase();
    let (body, pm, am) = if let Some(b) = w.strip_suffix("pm") {
        (b.to_string(), true, false)
    } else if let Some(b) = w.strip_suffix("am") {
        (b.to_string(), false, true)
    } else if let Some(b) = w.strip_suffix('h') {
        if b.chars().all(|c| c.is_ascii_digit()) && !b.is_empty() {
            (format!("{b}:00"), false, false)
        } else {
            return None;
        }
    } else {
        (w.clone(), false, false)
    };
    if !pm && !am && !body.contains(':') {
        return None;
    }
    let (h, m) = match body.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (body.parse::<u32>().ok()?, 0),
    };
    let h = match (pm, am, h) {
        (true, _, 12) => 12,
        (true, _, h) => h + 12,
        (_, true, 12) => 0,
        (_, _, h) => h,
    };
    NaiveTime::from_hms_opt(h, m, 0)
}

/// "1h", "30m", "1h30", "90min", "2 hours" (the unit may be the next word).
pub fn duration(words: &[&str]) -> Option<(Duration, usize)> {
    let w = words.first()?.to_lowercase();
    let unit_word = words.get(1).map(|s| s.to_lowercase());
    if let Ok(n) = w.parse::<i64>() {
        return match unit_word.as_deref() {
            Some("h" | "hr" | "hrs" | "hour" | "hours") => Some((Duration::hours(n), 2)),
            Some("m" | "min" | "mins" | "minute" | "minutes") => Some((Duration::minutes(n), 2)),
            _ => None,
        };
    }
    let mut total = 0;
    let mut num = String::new();
    let mut any = false;
    let mut chars = w.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            num.push(c);
        } else if c == 'h' {
            total += num.parse::<i64>().ok()? * 60;
            num.clear();
            any = true;
            while chars.peek().is_some_and(|c| c.is_alphabetic()) {
                chars.next();
            }
        } else if c == 'm' {
            total += num.parse::<i64>().ok()?;
            num.clear();
            any = true;
            while chars.peek().is_some_and(|c| c.is_alphabetic()) {
                chars.next();
            }
        } else {
            return None;
        }
    }
    if !num.is_empty() {
        // "1h30": trailing minutes.
        if !any {
            return None;
        }
        total += num.parse::<i64>().ok()?;
    }
    (any && total > 0).then(|| (Duration::minutes(total), 1))
}

/// Quick add: everything that isn't a date, time, duration or `@location` is the title.
#[cfg(test)]
pub fn parse(input: &str, today: NaiveDate) -> Option<Draft> {
    parse_on(input, today, today)
}

/// As `parse`, with `default` as the day when the text names none.
pub fn parse_on(input: &str, today: NaiveDate, default: NaiveDate) -> Option<Draft> {
    let (main, location) = match input.find(" @").or_else(|| input.starts_with('@').then_some(0)) {
        Some(i) => (&input[..i], input[i..].trim().trim_start_matches('@').trim().to_string()),
        None => (input, String::new()),
    };
    let words: Vec<&str> = main.split_whitespace().collect();
    let mut title = Vec::new();
    let mut date = None;
    let mut start = None;
    let mut end = None;
    let mut length = None;
    let mut all_day = false;
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let lw = w.to_lowercase();
        if (lw == "all" && words.get(i + 1).is_some_and(|n| n.eq_ignore_ascii_case("day"))) || lw == "allday" || lw == "all-day" {
            all_day = true;
            i += if lw == "all" { 2 } else { 1 };
            continue;
        }
        if lw == "for" || lw == "at" || lw == "on" {
            // Glue words before a time/date/duration are dropped; otherwise they're title.
            let rest = &words[i + 1..];
            if rest.first().is_some_and(|n| time(n).is_some() || duration(rest).is_some() || date_at(rest, today).is_some()) {
                i += 1;
                continue;
            }
        }
        if date.is_none() {
            if let Some((d, n)) = date_at(&words[i..], today) {
                date = Some(d);
                i += n;
                continue;
            }
        }
        // "9-10am", "14:00-15:30", "9am–11"
        if let Some((a, b)) = w.split_once(['-', '–']) {
            let suffix = if b.ends_with("pm") { "pm" } else if b.ends_with("am") { "am" } else { "" };
            let a_full = if a.ends_with("am") || a.ends_with("pm") || a.contains(':') { a.to_string() } else { format!("{a}{suffix}") };
            let b_full = if b.contains(':') || !suffix.is_empty() { b.to_string() } else { format!("{b}:00") };
            let a_full = if !a_full.contains(':') && suffix.is_empty() { format!("{a_full}:00") } else { a_full };
            if let (Some(s), Some(e)) = (time(&a_full), time(&b_full)) {
                start = Some(s);
                end = Some(e);
                i += 1;
                continue;
            }
        }
        if start.is_none() {
            if let Some(t) = time(w) {
                start = Some(t);
                i += 1;
                continue;
            }
        }
        if let Some((d, n)) = duration(&words[i..]) {
            length = Some(d);
            i += n;
            continue;
        }
        title.push(w);
        i += 1;
    }
    let title = title.join(" ");
    if title.is_empty() {
        return None;
    }
    let date = date.unwrap_or(default);
    let (start, end) = if all_day || start.is_none() {
        (None, None)
    } else {
        let s = start.unwrap();
        (Some(s), Some(end.unwrap_or_else(|| s + length.unwrap_or(Duration::hours(1)))))
    };
    let days = if start.is_none() { length.map(|l| (l.num_hours() / 24).max(1)).unwrap_or(1) } else { 1 };
    Some(Draft { title, date, start, end, days, location, ..Default::default() })
}

/// The first date (and a time near it) mentioned in free text, like an email asking to
/// "meet Thursday at 2pm" or listing "Oct 20, 10:30–11:30".
pub fn find_in(text: &str, today: NaiveDate) -> (Option<NaiveDate>, Option<NaiveTime>, Option<NaiveTime>) {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| ",.;!?()\"'".contains(c)).to_string())
        .filter(|w| !w.is_empty())
        .collect();
    let refs: Vec<&str> = words.iter().map(String::as_str).collect();
    // Words that are dates only in context ("may", "sun", "mar") need a number or a
    // weekday next to them to count.
    let ambiguous = |w: &str| matches!(w.to_lowercase().as_str(), "may" | "sun" | "mar" | "sat" | "wed" | "next");
    for i in 0..refs.len() {
        let Some((d, n)) = date_at(&refs[i..], today) else { continue };
        if n == 1 && ambiguous(refs[i]) && !refs[i].chars().next().is_some_and(|c| c.is_uppercase()) {
            continue;
        }
        // A time within the next few words: "at 2pm", "10:30-11:30".
        let mut start = None;
        let mut end = None;
        for w in refs.iter().skip(i + n).take(5) {
            if let Some((a, b)) = w.split_once(['-', '–']) {
                let suffix = if b.ends_with("pm") { "pm" } else if b.ends_with("am") { "am" } else { "" };
                let a_full = if a.contains(':') || a.ends_with('m') { a.to_string() } else { format!("{a}{suffix}") };
                if let (Some(s), Some(e)) = (time(&a_full).or_else(|| time(&format!("{a}:00"))), time(b).or_else(|| time(&format!("{b}:00")))) {
                    start = Some(s);
                    end = Some(e);
                    break;
                }
            }
            if let Some(t) = time(w) {
                start = Some(t);
                break;
            }
        }
        return (Some(d), start, end);
    }
    (None, None, None)
}

/// When a snooze should end: "3h", "2d", "30m", "tomorrow", "tomorrow 9am", "mon",
/// "oct 20 14:00", "5pm". A day without a time means 8:00 that morning; a time alone
/// means its next occurrence.
pub fn until(text: &str, now: chrono::NaiveDateTime) -> Option<chrono::NaiveDateTime> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    if let Some((d, n)) = duration(&words) {
        if n == words.len() {
            return Some(now + d);
        }
    }
    let lower = text.to_lowercase();
    if words.len() == 1 && lower.ends_with('d') && lower[..lower.len() - 1].parse::<i64>().is_ok() {
        let days: i64 = lower[..lower.len() - 1].parse().ok()?;
        return Some(now + Duration::days(days));
    }
    let today = now.date();
    let date = date_at(&words, today).map(|(d, _)| d);
    let t = words.iter().find_map(|w| time(w));
    let eight = NaiveTime::from_hms_opt(8, 0, 0)?;
    match (date, t) {
        (Some(d), Some(t)) => Some(d.and_time(t)),
        (Some(d), None) => Some(d.and_time(eight)).filter(|x| *x > now).or(Some(d.and_time(eight) + Duration::days(7))),
        (None, Some(t)) => {
            let at = today.and_time(t);
            Some(if at > now { at } else { at + Duration::days(1) })
        }
        (None, None) => None,
    }
}

/// Snooze choices offered before you type: (label, what it means).
pub fn snooze_presets(now: chrono::NaiveDateTime) -> Vec<(String, chrono::NaiveDateTime)> {
    let today = now.date();
    let eight = NaiveTime::from_hms_opt(8, 0, 0).unwrap();
    let sat = next_weekday(today + Duration::days(1), Weekday::Sat);
    let mon = next_weekday(today + Duration::days(1), Weekday::Mon);
    vec![
        ("later today · 3h".to_string(), now + Duration::hours(3)),
        ("tomorrow morning".to_string(), (today + Duration::days(1)).and_time(eight)),
        ("this weekend".to_string(), sat.and_time(NaiveTime::from_hms_opt(9, 0, 0).unwrap())),
        ("next week".to_string(), mon.and_time(eight)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    // A Wednesday.
    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 14).unwrap()
    }
    fn t(h: u32, m: u32) -> Option<NaiveTime> {
        NaiveTime::from_hms_opt(h, m, 0)
    }

    #[test]
    fn quick_add() {
        let d = parse("Lunch with Maya fri 12:30 1h @Osteria Morini", today()).unwrap();
        assert_eq!(d.title, "Lunch with Maya");
        assert_eq!(d.date, NaiveDate::from_ymd_opt(2026, 10, 16).unwrap());
        assert_eq!((d.start, d.end), (t(12, 30), t(13, 30)));
        assert_eq!(d.location, "Osteria Morini");

        let d = parse("Dentist tomorrow at 3pm for 45m", today()).unwrap();
        assert_eq!(d.title, "Dentist");
        assert_eq!(d.date, today() + Duration::days(1));
        assert_eq!((d.start, d.end), (t(15, 0), t(15, 45)));

        let d = parse("Offsite oct 20 all day", today()).unwrap();
        assert_eq!((d.title.as_str(), d.start), ("Offsite", None));
        assert_eq!(d.date, NaiveDate::from_ymd_opt(2026, 10, 20).unwrap());

        let d = parse("Standup 9-9:15am", today()).unwrap();
        assert_eq!((d.start, d.end, d.date), (t(9, 0), t(9, 15), today()));

        let d = parse("Review 2026-11-02 14:00-15:30", today()).unwrap();
        assert_eq!((d.start, d.end), (t(14, 0), t(15, 30)));

        // Today counts for its own weekday; a past month/day rolls to next year.
        assert_eq!(parse("Gym wed 7am", today()).unwrap().date, today());
        assert_eq!(parse("Taxes mar 1", today()).unwrap().date, NaiveDate::from_ymd_opt(2027, 3, 1).unwrap());
        assert!(parse("tomorrow 9am", today()).is_none());
        assert_eq!(parse("Retro next fri", today()).unwrap().date, NaiveDate::from_ymd_opt(2026, 10, 23).unwrap());
    }

    #[test]
    fn finds_dates_in_mail() {
        let (d, s, e) = find_in("Hi Ted, can we meet Thursday at 2pm to go over the deck?", today());
        assert_eq!(d, NaiveDate::from_ymd_opt(2026, 10, 15));
        assert_eq!((s, e), (t(14, 0), None));
        let (d, s, e) = find_in("Agenda for Oct 20, 10:30-11:30 in room 4", today());
        assert_eq!(d, NaiveDate::from_ymd_opt(2026, 10, 20));
        assert_eq!((s, e), (t(10, 30), t(11, 30)));
        // "may" as a verb isn't a month.
        assert_eq!(find_in("you may want to review this", today()).0, None);
    }

    #[test]
    fn snooze_until() {
        let now = today().and_hms_opt(15, 0, 0).unwrap(); // Wed 15:00
        let at = |d: i64, h: u32| (today() + Duration::days(d)).and_hms_opt(h, 0, 0).unwrap();
        assert_eq!(until("3h", now), Some(now + Duration::hours(3)));
        assert_eq!(until("2d", now), Some(now + Duration::days(2)));
        assert_eq!(until("tomorrow", now), Some(at(1, 8)));
        assert_eq!(until("tomorrow 9am", now), Some(at(1, 9)));
        assert_eq!(until("mon", now), Some(at(5, 8)));
        assert_eq!(until("5pm", now), Some(at(0, 17)));
        assert_eq!(until("9am", now), Some(at(1, 9)));
        assert_eq!(until("whenever", now), None);
    }

    #[test]
    fn times_and_durations() {
        assert_eq!(time("12am"), t(0, 0));
        assert_eq!(time("12pm"), t(12, 0));
        assert_eq!(time("3:15pm"), t(15, 15));
        assert_eq!(time("15h"), t(15, 0));
        assert_eq!(time("30"), None);
        assert_eq!(duration(&["1h30"]), Some((Duration::minutes(90), 1)));
        assert_eq!(duration(&["90min"]), Some((Duration::minutes(90), 1)));
        assert_eq!(duration(&["2", "hours"]), Some((Duration::hours(2), 2)));
        assert_eq!(duration(&["Maya"]), None);
    }
}
