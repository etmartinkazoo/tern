//! Calendar events: iCalendar objects from CalDAV, expanded into dated occurrences for
//! the agenda. Parsing, recurrence (RRULE/RDATE/EXDATE/RECURRENCE-ID) and time zones are
//! ical-rs's (Pimalaya); this file turns its civil times into local ones and picks out
//! what the agenda shows.

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use ical::component::{IcalComponent, IcalComponentKind, IcalComponentName};
use ical::param::IcalParam;
use ical::prop::{IcalPropKind, IcalPropName};
use ical::recur::set::{IcalRecurSet, IcalRecurZone};
use ical::recur::IcalRecurDateTime;
use ical::tree::cst::IcalCst;
use ical::tz::IcalTz;
use ical::value::IcalValue;

/// One occurrence on the agenda.
#[derive(Debug, Clone)]
pub struct Occurrence {
    pub calendar: usize,
    /// The CalDAV resource the event lives in.
    pub item: String,
    pub uid: String,
    pub summary: String,
    pub location: String,
    pub description: String,
    pub organizer: String,
    pub attendees: Vec<String>,
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
    pub all_day: bool,
    pub recurring: bool,
    pub url: String,
}

fn kind(c: &IcalComponent<'_>) -> Option<IcalComponentKind> {
    match &c.name {
        IcalComponentName::Kind(k) => Some(*k),
        _ => None,
    }
}

/// The text of a value, whatever type it was decoded as.
fn text(v: &IcalValue<'_>) -> String {
    match v {
        IcalValue::Text(t) => t.0.to_string(),
        IcalValue::DateTime(t) => t.0.to_string(),
        IcalValue::Date(t) => t.0.to_string(),
        IcalValue::Duration(t) => t.0.to_string(),
        IcalValue::CalAddress(t) => t.0.to_string(),
        other => format!("{other:?}"),
    }
}

struct Props {
    uid: String,
    summary: String,
    location: String,
    description: String,
    organizer: String,
    attendees: Vec<String>,
    url: String,
    start: Option<(String, Option<String>)>,
    end: Option<(String, Option<String>)>,
    duration: Option<String>,
    recurrence_id: Option<String>,
    recurring: bool,
    cancelled: bool,
}

fn props(c: &IcalComponent<'_>) -> Props {
    let mut p = Props {
        uid: String::new(),
        summary: String::new(),
        location: String::new(),
        description: String::new(),
        organizer: String::new(),
        attendees: vec![],
        url: String::new(),
        start: None,
        end: None,
        duration: None,
        recurrence_id: None,
        recurring: false,
        cancelled: false,
    };
    for prop in &c.props {
        let IcalPropName::Kind(k) = &prop.name else { continue };
        let tzid = prop.params.iter().find_map(|x| match x {
            IcalParam::TzId(t) => Some(t.to_string()),
            _ => None,
        });
        let cn = prop.params.iter().find_map(|x| match x {
            IcalParam::Cn(n) => Some(n.to_string()),
            _ => None,
        });
        let v = text(&prop.value);
        let person = |v: &str| {
            let addr = v.trim_start_matches("mailto:").trim_start_matches("MAILTO:").to_string();
            match &cn {
                Some(n) if !n.is_empty() => format!("{n} <{addr}>"),
                _ => addr,
            }
        };
        match k {
            IcalPropKind::Uid => p.uid = v,
            IcalPropKind::Summary => p.summary = v,
            IcalPropKind::Location => p.location = v,
            IcalPropKind::Description => p.description = v,
            IcalPropKind::Url => p.url = v,
            IcalPropKind::Organizer => p.organizer = person(&v),
            IcalPropKind::Attendee => p.attendees.push(person(&v)),
            IcalPropKind::DtStart => p.start = Some((v, tzid)),
            IcalPropKind::DtEnd => p.end = Some((v, tzid)),
            IcalPropKind::Duration => p.duration = Some(v),
            IcalPropKind::RecurrenceId => p.recurrence_id = Some(v),
            IcalPropKind::RRule | IcalPropKind::RDate => p.recurring = true,
            IcalPropKind::Status => p.cancelled = v.eq_ignore_ascii_case("CANCELLED"),
            _ => {}
        }
    }
    p
}

/// `YYYYMMDD` or `YYYYMMDDTHHMMSS[Z]` as a civil time.
fn civil(v: &str) -> Option<IcalRecurDateTime> {
    let v = v.trim().trim_end_matches(['Z', 'z']);
    let d = |a: usize, b: usize| v.get(a..b).and_then(|s| s.parse::<u32>().ok());
    let (y, m, day) = (v.get(0..4)?.parse::<i32>().ok()?, d(4, 6)? as u8, d(6, 8)? as u8);
    let mut t = IcalRecurDateTime::date(y, m, day);
    if v.len() >= 15 {
        t.hour = d(9, 11)? as u8;
        t.minute = d(11, 13)? as u8;
        t.second = d(13, 15)? as u8;
    }
    Some(t)
}

/// `PT1H30M`, `P1D`, `-PT15M` in seconds.
pub fn duration_secs(v: &str) -> Option<i64> {
    let v = v.trim();
    let (neg, v) = match v.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, v.strip_prefix('+').unwrap_or(v)),
    };
    let v = v.strip_prefix('P')?;
    let mut total = 0i64;
    let mut num = String::new();
    for ch in v.chars() {
        match ch {
            'T' => {}
            '0'..='9' => num.push(ch),
            unit => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += n * match unit {
                    'W' => 604_800,
                    'D' => 86_400,
                    'H' => 3_600,
                    'M' => 60,
                    'S' => 1,
                    _ => return None,
                };
            }
        }
    }
    Some(if neg { -total } else { total })
}

/// A civil time on `zone` as a local date-time. Zones come from the calendar's own
/// VTIMEZONEs first, then the IANA database; anything unknown is read as local time.
fn to_local(t: IcalRecurDateTime, zone: &IcalRecurZone, zones: &[IcalTz]) -> Option<DateTime<Local>> {
    let naive = NaiveDate::from_ymd_opt(t.year, t.month as u32, t.day as u32)?
        .and_hms_opt(t.hour as u32, t.minute as u32, t.second as u32)?;
    let utc = |n: NaiveDateTime| Some(Utc.from_utc_datetime(&n).with_timezone(&Local));
    match zone {
        IcalRecurZone::Utc => utc(naive),
        IcalRecurZone::Floating => Local.from_local_datetime(&naive).earliest(),
        IcalRecurZone::Tz(id) => {
            if let Some(z) = zones.iter().find(|z| &z.id == id) {
                if let Some(secs) = z.resolve(t).instant(t) {
                    return DateTime::from_timestamp(secs, 0).map(|d| d.with_timezone(&Local));
                }
            }
            match id.parse::<chrono_tz::Tz>() {
                Ok(tz) => tz.from_local_datetime(&naive).earliest().map(|d| d.with_timezone(&Local)),
                Err(_) => Local.from_local_datetime(&naive).earliest(),
            }
        }
    }
}

/// Every occurrence of the events in one iCalendar object that overlaps `from..to`.
pub fn expand(ics: &str, item: &str, calendar: usize, from: DateTime<Local>, to: DateTime<Local>) -> Vec<Occurrence> {
    let Ok(cst) = IcalCst::parse(ics) else { return vec![] };
    let cal = cst.decode();
    let comps = &cal.components;
    let zones: Vec<IcalTz> = comps
        .iter()
        .filter(|c| kind(c) == Some(IcalComponentKind::VTimezone))
        .filter_map(IcalTz::of_component)
        .collect();
    let events: Vec<&IcalComponent<'_>> = comps.iter().filter(|c| kind(c) == Some(IcalComponentKind::VEvent)).collect();

    let mut out = Vec::new();
    for master in events.iter().copied().filter(|c| props(c).recurrence_id.is_none()) {
        let mp = props(master);
        if mp.cancelled {
            continue;
        }
        let Some((start_raw, _)) = mp.start.clone() else { continue };
        let all_day = start_raw.trim().len() == 8;
        let set = IcalRecurSet::of_uid(comps, &mp.uid);
        let zone = set.zone.clone();
        // Length of the master: DTEND − DTSTART on the same clock, else DURATION.
        let length = |p: &Props| -> i64 {
            let s = p.start.as_ref().and_then(|(v, _)| civil(v));
            let e = p.end.as_ref().and_then(|(v, _)| civil(v));
            match (s, e, &p.duration) {
                (Some(s), Some(e), _) => e.seconds() - s.seconds(),
                (_, _, Some(d)) => duration_secs(d).unwrap_or(0),
                _ if all_day => 86_400,
                _ => 0,
            }
        };
        let master_len = length(&mp);
        // Overrides, found by the RECURRENCE-ID they carry.
        let overrides: Vec<(IcalRecurDateTime, Props)> = events
            .iter()
            .map(|c| props(c))
            .filter(|p| p.uid == mp.uid && p.recurrence_id.is_some())
            .filter_map(|p| Some((civil(p.recurrence_id.as_ref()?)?, p)))
            .collect();

        for (n, occ) in set.expand().enumerate() {
            if n > 5000 {
                break;
            }
            let Some(start) = to_local(occ.start, &zone, &zones) else { continue };
            if start > to {
                break;
            }
            let over = overrides.iter().find(|(id, _)| *id == occ.id).map(|(_, p)| p);
            if over.is_some_and(|p| p.cancelled) {
                continue;
            }
            let p = over.unwrap_or(&mp);
            let len = over.map(length).unwrap_or(master_len);
            let end = start + Duration::seconds(len.max(0));
            if end < from || (end == from && len > 0) {
                continue;
            }
            out.push(Occurrence {
                calendar,
                item: item.to_string(),
                uid: mp.uid.clone(),
                summary: if p.summary.is_empty() { "(no title)".into() } else { p.summary.clone() },
                location: p.location.clone(),
                description: p.description.clone(),
                organizer: p.organizer.clone(),
                attendees: p.attendees.clone(),
                start,
                end,
                all_day,
                recurring: mp.recurring,
                url: p.url.clone(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    const WEEKLY: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\n\
BEGIN:VEVENT\r\nUID:standup\r\nDTSTAMP:20260101T000000Z\r\nDTSTART:20260105T090000\r\nDTEND:20260105T091500\r\n\
RRULE:FREQ=WEEKLY;COUNT=5\r\nEXDATE:20260119T090000\r\nSUMMARY:Standup\r\nLOCATION:Room 4\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:standup\r\nDTSTAMP:20260101T000000Z\r\nRECURRENCE-ID:20260126T090000\r\nDTSTART:20260126T140000\r\n\
DTEND:20260126T150000\r\nSUMMARY:Standup (afternoon)\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn expands_rules_exdates_and_overrides() {
        let o = expand(WEEKLY, "i", 0, local(2026, 1, 1, 0, 0), local(2026, 3, 1, 0, 0));
        let days: Vec<String> = o.iter().map(|o| o.start.format("%m-%d %H:%M").to_string()).collect();
        // Five Mondays, minus the 19th, with the 26th moved to the afternoon.
        assert_eq!(days, ["01-05 09:00", "01-12 09:00", "01-26 14:00", "02-02 09:00"]);
        assert_eq!(o[2].summary, "Standup (afternoon)");
        assert_eq!((o[2].end - o[2].start).num_minutes(), 60);
        assert_eq!((o[0].end - o[0].start).num_minutes(), 15);
        assert_eq!(o[0].location, "Room 4");
        assert!(o[0].recurring);
    }

    #[test]
    fn window_and_all_day() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTAMP:20260101T000000Z\r\n\
DTSTART;VALUE=DATE:20260310\r\nDTEND;VALUE=DATE:20260312\r\nSUMMARY:Offsite\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let o = expand(ics, "i", 0, local(2026, 3, 11, 0, 0), local(2026, 3, 20, 0, 0));
        assert_eq!(o.len(), 1);
        assert!(o[0].all_day);
        assert_eq!((o[0].end - o[0].start).num_days(), 2);
        assert!(expand(ics, "i", 0, local(2026, 4, 1, 0, 0), local(2026, 5, 1, 0, 0)).is_empty());
    }

    #[test]
    fn utc_and_named_zones() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\nBEGIN:VEVENT\r\nUID:z\r\nDTSTAMP:20260101T000000Z\r\n\
DTSTART;TZID=America/New_York:20260715T120000\r\nDURATION:PT45M\r\nSUMMARY:Call\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:u\r\nDTSTAMP:20260101T000000Z\r\nDTSTART:20260715T160000Z\r\nSUMMARY:Same instant\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let o = expand(ics, "i", 0, local(2026, 7, 1, 0, 0), local(2026, 8, 1, 0, 0));
        assert_eq!(o.len(), 2);
        // Noon in New York in July is 16:00 UTC.
        assert_eq!(o[0].start, o[1].start);
        assert_eq!((o[0].end - o[0].start).num_minutes(), 45);
    }

    #[test]
    fn durations() {
        assert_eq!(duration_secs("PT1H30M"), Some(5400));
        assert_eq!(duration_secs("P1D"), Some(86400));
        assert_eq!(duration_secs("-PT15M"), Some(-900));
        assert_eq!(duration_secs("P1W"), Some(604800));
    }
}

// ------------------------------------------------------------------- writing

use crate::when::Draft;
use ical::component::vevent::VEVENT;
use ical::prop::{
    description::DESCRIPTION, dtend::DTEND, dtstamp::DTSTAMP, dtstart::DTSTART, duration::DURATION,
    location::LOCATION, sequence::SEQUENCE, summary::SUMMARY,
};

/// Escape a TEXT value (RFC 5545 3.3.11).
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace('\n', "\\n")
}

/// Fold a content line at 75 octets without splitting a UTF-8 character.
fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut n = 0;
    for ch in line.chars() {
        let w = ch.len_utf8();
        if n + w > 75 {
            out.push_str("\r\n ");
            n = 1;
        }
        out.push(ch);
        n += w;
    }
    out
}

fn stamp() -> String {
    Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

/// DTSTART/DTEND lines for a draft. Timed events go out in `tzid` when the series
/// already lived in a named zone (so DST keeps the wall-clock time), else in UTC.
fn time_lines(d: &Draft, tzid: Option<&str>) -> (String, String) {
    match (d.start, d.end) {
        (Some(s), Some(e)) => {
            let at = |t: chrono::NaiveTime| -> String {
                let local = Local.from_local_datetime(&d.date.and_time(t)).earliest().unwrap_or_else(Local::now);
                match tzid.and_then(|z| z.parse::<chrono_tz::Tz>().ok()) {
                    Some(tz) => format!(";TZID={}:{}", tz.name(), local.with_timezone(&tz).format("%Y%m%dT%H%M%S")),
                    None => format!(":{}", local.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ")),
                }
            };
            // An end before the start means it runs past midnight.
            let end_date_shift = if e <= s { 1 } else { 0 };
            let end = Draft { date: d.date + Duration::days(end_date_shift), ..d.clone() };
            let end_at = {
                let local = Local.from_local_datetime(&end.date.and_time(e)).earliest().unwrap_or_else(Local::now);
                match tzid.and_then(|z| z.parse::<chrono_tz::Tz>().ok()) {
                    Some(tz) => format!(";TZID={}:{}", tz.name(), local.with_timezone(&tz).format("%Y%m%dT%H%M%S")),
                    None => format!(":{}", local.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ")),
                }
            };
            (format!("DTSTART{}", at(s)), format!("DTEND{end_at}"))
        }
        _ => (
            format!("DTSTART;VALUE=DATE:{}", d.date.format("%Y%m%d")),
            format!("DTEND;VALUE=DATE:{}", (d.date + Duration::days(d.days.max(1))).format("%Y%m%d")),
        ),
    }
}

/// A new event as a complete iCalendar object.
pub fn new_ics(d: &Draft, uid: &str) -> String {
    let (start, end) = time_lines(d, None);
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".into(),
        "PRODID:-//tern//EN".into(),
        "BEGIN:VEVENT".into(),
        format!("UID:{uid}"),
        format!("DTSTAMP:{}", stamp()),
        start,
        end,
        format!("SUMMARY:{}", esc(&d.title)),
    ];
    if !d.location.is_empty() {
        lines.push(format!("LOCATION:{}", esc(&d.location)));
    }
    if !d.description.trim().is_empty() {
        lines.push(format!("DESCRIPTION:{}", esc(d.description.trim_end())));
    }
    lines.extend(["SEQUENCE:0".into(), "END:VEVENT".into(), "END:VCALENDAR".into()]);
    lines.iter().map(|l| fold(l)).collect::<Vec<_>>().join("\r\n") + "\r\n"
}

/// The series master (or the only event) of an object, as a draft for the edit form.
pub fn draft_of(ics: &str) -> Option<Draft> {
    let cst = IcalCst::parse(ics).ok()?;
    let cal = cst.decode();
    let zones: Vec<IcalTz> = cal
        .components
        .iter()
        .filter(|c| kind(c) == Some(IcalComponentKind::VTimezone))
        .filter_map(IcalTz::of_component)
        .collect();
    let master = cal
        .components
        .iter()
        .filter(|c| kind(c) == Some(IcalComponentKind::VEvent))
        .find(|c| props(c).recurrence_id.is_none())?;
    let p = props(master);
    let (sv, stz) = p.start.clone()?;
    let all_day = sv.trim().len() == 8;
    let zone_of = |v: &str, tz: &Option<String>| {
        if v.trim().ends_with(['Z', 'z']) {
            IcalRecurZone::Utc
        } else if let Some(t) = tz {
            IcalRecurZone::Tz(t.clone())
        } else {
            IcalRecurZone::Floating
        }
    };
    let start = to_local(civil(&sv)?, &zone_of(&sv, &stz), &zones)?;
    let end = match (&p.end, &p.duration) {
        (Some((ev, etz)), _) => civil(ev).and_then(|c| to_local(c, &zone_of(ev, etz), &zones)),
        (None, Some(d)) => duration_secs(d).map(|s| start + Duration::seconds(s)),
        _ => None,
    };
    Some(Draft {
        title: p.summary,
        date: start.date_naive(),
        start: (!all_day).then(|| start.time()),
        end: if all_day { None } else { end.map(|e| e.time()) },
        days: if all_day { end.map(|e| (e.date_naive() - start.date_naive()).num_days().max(1)).unwrap_or(1) } else { 1 },
        location: p.location,
        description: p.description,
    })
}

/// Apply a draft to an existing object, changing only the series master's title,
/// times, place and notes. Everything else (attendees, alarms, rules, exceptions,
/// properties tern doesn't know) is kept byte for byte.
pub fn edit_ics(ics: &str, d: &Draft) -> anyhow::Result<String> {
    let mut cst = IcalCst::parse(ics).map_err(|e| anyhow::anyhow!("parsing event: {e}"))?;
    let tzid = {
        let cal = cst.decode();
        cal.components
            .iter()
            .filter(|c| kind(c) == Some(IcalComponentKind::VEvent))
            .find_map(|c| props(c).start.and_then(|(_, tz)| tz))
    };
    let ev = cst.component_mut::<VEVENT>().ok_or_else(|| anyhow::anyhow!("no event in object"))?;
    let seq = ev
        .prop::<SEQUENCE>()
        .and_then(|p| p.0.trim().parse::<u32>().ok())
        .unwrap_or(0);
    let (start, end) = time_lines(d, tzid.as_deref());
    ev.remove::<SUMMARY>().remove::<LOCATION>().remove::<DESCRIPTION>();
    ev.remove::<DTSTART>().remove::<DTEND>().remove::<DURATION>();
    ev.remove::<SEQUENCE>().remove::<DTSTAMP>();
    let mut push = |line: String| -> anyhow::Result<()> {
        ev.push_raw(&line).map_err(|e| anyhow::anyhow!("writing {line}: {e}"))?;
        Ok(())
    };
    push(format!("DTSTAMP:{}", stamp()))?;
    push(start)?;
    push(end)?;
    push(format!("SUMMARY:{}", esc(&d.title)))?;
    if !d.location.is_empty() {
        push(format!("LOCATION:{}", esc(&d.location)))?;
    }
    if !d.description.trim().is_empty() {
        push(format!("DESCRIPTION:{}", esc(d.description.trim_end())))?;
    }
    push(format!("SEQUENCE:{}", seq + 1))?;
    Ok(cst.to_string())
}

#[cfg(test)]
mod write_tests {
    use super::*;
    use chrono::NaiveTime;

    fn draft() -> Draft {
        Draft {
            title: "Lunch, with Maya".into(),
            date: chrono::NaiveDate::from_ymd_opt(2026, 10, 16).unwrap(),
            start: NaiveTime::from_hms_opt(12, 30, 0),
            end: NaiveTime::from_hms_opt(13, 30, 0),
            days: 1,
            location: "Osteria".into(),
            description: String::new(),
        }
    }

    #[test]
    fn new_event_round_trips() {
        let ics = new_ics(&draft(), "x@tern");
        let back = draft_of(&ics).unwrap();
        assert_eq!(back, draft());
        let o = expand(&ics, "i", 0, Local::now() - Duration::days(3650), Local::now() + Duration::days(3650));
        assert_eq!(o[0].summary, "Lunch, with Maya");
    }

    #[test]
    fn edit_keeps_everything_else() {
        let orig = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Other//EN\r\nBEGIN:VEVENT\r\nUID:k\r\nDTSTAMP:20260101T000000Z\r\n\
DTSTART;TZID=Europe/Berlin:20261016T090000\r\nDTEND;TZID=Europe/Berlin:20261016T100000\r\nRRULE:FREQ=WEEKLY\r\nSUMMARY:Old\r\n\
ATTENDEE;CN=Jo:mailto:jo@x.y\r\nX-CUSTOM:keep me\r\nSEQUENCE:3\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT10M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let mut d = draft_of(orig).unwrap();
        assert_eq!(d.title, "Old");
        d.title = "New title".into();
        let out = edit_ics(orig, &d).unwrap();
        for keep in ["RRULE:FREQ=WEEKLY", "ATTENDEE;CN=Jo:mailto:jo@x.y", "X-CUSTOM:keep me", "TRIGGER:-PT10M", "PRODID:-//Other//EN"] {
            assert!(out.contains(keep), "lost {keep}:\n{out}");
        }
        assert!(out.contains("SUMMARY:New title") && out.contains("SEQUENCE:4") && !out.contains("SUMMARY:Old"));
        // The series stays in its own zone, at the same wall-clock time.
        assert!(out.contains("DTSTART;TZID=Europe/Berlin:20261016T090000"), "{out}");
        assert_eq!(draft_of(&out).unwrap().start, d.start);
    }
}
