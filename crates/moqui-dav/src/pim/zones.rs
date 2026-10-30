/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! Time zones of iCalendar files: a TZID read as an IANA name, an Outlook (Windows) name, a path that ends in an IANA
//! name (`/mozilla.org/20050126_1/America/New_York`), or the file's own VTIMEZONE, which becomes a POSIX rule and,
//! when one has the same offsets, the IANA zone; and the VTIMEZONE a written file gives for each zone it uses.

use std::collections::HashMap;

use jiff::civil::{Date, DateTime};
use jiff::tz::{Offset, TimeZone};
use jiff::{Timestamp, ToSpan};

use super::line::Property;
use super::rrule::{self, Rule, Until};

/// Windows time zone names (Outlook and Exchange TZIDs) to IANA names: CLDR windowsZones, territory 001.
const WINDOWS_ZONES: &[(&str, &str)] = &[
    ("Dateline Standard Time", "Etc/GMT+12"),
    ("UTC-11", "Etc/GMT+11"),
    ("Aleutian Standard Time", "America/Adak"),
    ("Hawaiian Standard Time", "Pacific/Honolulu"),
    ("Marquesas Standard Time", "Pacific/Marquesas"),
    ("Alaskan Standard Time", "America/Anchorage"),
    ("UTC-09", "Etc/GMT+9"),
    ("Pacific Standard Time (Mexico)", "America/Tijuana"),
    ("UTC-08", "Etc/GMT+8"),
    ("Pacific Standard Time", "America/Los_Angeles"),
    ("US Mountain Standard Time", "America/Phoenix"),
    ("Mountain Standard Time (Mexico)", "America/Mazatlan"),
    ("Mountain Standard Time", "America/Denver"),
    ("Yukon Standard Time", "America/Whitehorse"),
    ("Central America Standard Time", "America/Guatemala"),
    ("Central Standard Time", "America/Chicago"),
    ("Easter Island Standard Time", "Pacific/Easter"),
    ("Central Standard Time (Mexico)", "America/Mexico_City"),
    ("Canada Central Standard Time", "America/Regina"),
    ("SA Pacific Standard Time", "America/Bogota"),
    ("Eastern Standard Time (Mexico)", "America/Cancun"),
    ("Eastern Standard Time", "America/New_York"),
    ("Haiti Standard Time", "America/Port-au-Prince"),
    ("Cuba Standard Time", "America/Havana"),
    ("US Eastern Standard Time", "America/Indiana/Indianapolis"),
    ("Turks And Caicos Standard Time", "America/Grand_Turk"),
    ("Paraguay Standard Time", "America/Asuncion"),
    ("Atlantic Standard Time", "America/Halifax"),
    ("Venezuela Standard Time", "America/Caracas"),
    ("Central Brazilian Standard Time", "America/Cuiaba"),
    ("SA Western Standard Time", "America/La_Paz"),
    ("Pacific SA Standard Time", "America/Santiago"),
    ("Newfoundland Standard Time", "America/St_Johns"),
    ("Tocantins Standard Time", "America/Araguaina"),
    ("E. South America Standard Time", "America/Sao_Paulo"),
    ("SA Eastern Standard Time", "America/Cayenne"),
    ("Argentina Standard Time", "America/Argentina/Buenos_Aires"),
    ("Greenland Standard Time", "America/Nuuk"),
    ("Montevideo Standard Time", "America/Montevideo"),
    ("Magallanes Standard Time", "America/Punta_Arenas"),
    ("Saint Pierre Standard Time", "America/Miquelon"),
    ("Bahia Standard Time", "America/Bahia"),
    ("UTC-02", "Etc/GMT+2"),
    ("Azores Standard Time", "Atlantic/Azores"),
    ("Cape Verde Standard Time", "Atlantic/Cape_Verde"),
    ("UTC", "Etc/UTC"),
    ("GMT Standard Time", "Europe/London"),
    ("Greenwich Standard Time", "Atlantic/Reykjavik"),
    ("Sao Tome Standard Time", "Africa/Sao_Tome"),
    ("Morocco Standard Time", "Africa/Casablanca"),
    ("W. Europe Standard Time", "Europe/Berlin"),
    ("Central Europe Standard Time", "Europe/Budapest"),
    ("Romance Standard Time", "Europe/Paris"),
    ("Central European Standard Time", "Europe/Warsaw"),
    ("W. Central Africa Standard Time", "Africa/Lagos"),
    ("Jordan Standard Time", "Asia/Amman"),
    ("GTB Standard Time", "Europe/Bucharest"),
    ("Middle East Standard Time", "Asia/Beirut"),
    ("Egypt Standard Time", "Africa/Cairo"),
    ("E. Europe Standard Time", "Europe/Chisinau"),
    ("Syria Standard Time", "Asia/Damascus"),
    ("West Bank Standard Time", "Asia/Hebron"),
    ("South Africa Standard Time", "Africa/Johannesburg"),
    ("FLE Standard Time", "Europe/Kiev"),
    ("Israel Standard Time", "Asia/Jerusalem"),
    ("South Sudan Standard Time", "Africa/Juba"),
    ("Kaliningrad Standard Time", "Europe/Kaliningrad"),
    ("Sudan Standard Time", "Africa/Khartoum"),
    ("Libya Standard Time", "Africa/Tripoli"),
    ("Namibia Standard Time", "Africa/Windhoek"),
    ("Arabic Standard Time", "Asia/Baghdad"),
    ("Turkey Standard Time", "Europe/Istanbul"),
    ("Arab Standard Time", "Asia/Riyadh"),
    ("Belarus Standard Time", "Europe/Minsk"),
    ("Russian Standard Time", "Europe/Moscow"),
    ("E. Africa Standard Time", "Africa/Nairobi"),
    ("Volgograd Standard Time", "Europe/Volgograd"),
    ("Iran Standard Time", "Asia/Tehran"),
    ("Arabian Standard Time", "Asia/Dubai"),
    ("Astrakhan Standard Time", "Europe/Astrakhan"),
    ("Azerbaijan Standard Time", "Asia/Baku"),
    ("Russia Time Zone 3", "Europe/Samara"),
    ("Mauritius Standard Time", "Indian/Mauritius"),
    ("Saratov Standard Time", "Europe/Saratov"),
    ("Georgian Standard Time", "Asia/Tbilisi"),
    ("Caucasus Standard Time", "Asia/Yerevan"),
    ("Afghanistan Standard Time", "Asia/Kabul"),
    ("West Asia Standard Time", "Asia/Tashkent"),
    ("Ekaterinburg Standard Time", "Asia/Yekaterinburg"),
    ("Pakistan Standard Time", "Asia/Karachi"),
    ("Qyzylorda Standard Time", "Asia/Qyzylorda"),
    ("India Standard Time", "Asia/Kolkata"),
    ("Sri Lanka Standard Time", "Asia/Colombo"),
    ("Nepal Standard Time", "Asia/Kathmandu"),
    ("Central Asia Standard Time", "Asia/Bishkek"),
    ("Bangladesh Standard Time", "Asia/Dhaka"),
    ("Omsk Standard Time", "Asia/Omsk"),
    ("Myanmar Standard Time", "Asia/Yangon"),
    ("SE Asia Standard Time", "Asia/Bangkok"),
    ("Altai Standard Time", "Asia/Barnaul"),
    ("W. Mongolia Standard Time", "Asia/Hovd"),
    ("North Asia Standard Time", "Asia/Krasnoyarsk"),
    ("N. Central Asia Standard Time", "Asia/Novosibirsk"),
    ("Tomsk Standard Time", "Asia/Tomsk"),
    ("China Standard Time", "Asia/Shanghai"),
    ("North Asia East Standard Time", "Asia/Irkutsk"),
    ("Singapore Standard Time", "Asia/Singapore"),
    ("W. Australia Standard Time", "Australia/Perth"),
    ("Taipei Standard Time", "Asia/Taipei"),
    ("Ulaanbaatar Standard Time", "Asia/Ulaanbaatar"),
    ("Aus Central W. Standard Time", "Australia/Eucla"),
    ("Transbaikal Standard Time", "Asia/Chita"),
    ("Tokyo Standard Time", "Asia/Tokyo"),
    ("North Korea Standard Time", "Asia/Pyongyang"),
    ("Korea Standard Time", "Asia/Seoul"),
    ("Yakutsk Standard Time", "Asia/Yakutsk"),
    ("Cen. Australia Standard Time", "Australia/Adelaide"),
    ("AUS Central Standard Time", "Australia/Darwin"),
    ("E. Australia Standard Time", "Australia/Brisbane"),
    ("AUS Eastern Standard Time", "Australia/Sydney"),
    ("West Pacific Standard Time", "Pacific/Port_Moresby"),
    ("Tasmania Standard Time", "Australia/Hobart"),
    ("Vladivostok Standard Time", "Asia/Vladivostok"),
    ("Lord Howe Standard Time", "Australia/Lord_Howe"),
    ("Bougainville Standard Time", "Pacific/Bougainville"),
    ("Russia Time Zone 10", "Asia/Srednekolymsk"),
    ("Magadan Standard Time", "Asia/Magadan"),
    ("Norfolk Standard Time", "Pacific/Norfolk"),
    ("Sakhalin Standard Time", "Asia/Sakhalin"),
    ("Central Pacific Standard Time", "Pacific/Guadalcanal"),
    ("Russia Time Zone 11", "Asia/Kamchatka"),
    ("New Zealand Standard Time", "Pacific/Auckland"),
    ("UTC+12", "Etc/GMT-12"),
    ("Fiji Standard Time", "Pacific/Fiji"),
    ("Chatham Islands Standard Time", "Pacific/Chatham"),
    ("UTC+13", "Etc/GMT-13"),
    ("Tonga Standard Time", "Pacific/Tongatapu"),
    ("Samoa Standard Time", "Pacific/Apia"),
    ("Line Islands Standard Time", "Pacific/Kiritimati"),
];

/// An IANA zone by name, and the name to keep for it.
fn iana(name: &str) -> Option<(String, TimeZone)> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    if name.eq_ignore_ascii_case("Z") || name.eq_ignore_ascii_case("GMT") || name.eq_ignore_ascii_case("UTC") {
        return Some(("UTC".to_string(), TimeZone::UTC));
    }
    let zone = TimeZone::get(name).ok()?;
    let kept = zone.iana_name().unwrap_or(name).to_string();
    Some((kept, zone))
}

/// A zone name as a script gives it: an IANA name, a Windows name, a POSIX TZ rule as this module writes for a
/// VTIMEZONE without an IANA match (`<+0130>-1:30`), or a fixed offset (`+05:30`).
pub(crate) fn zone_by_name(name: &str) -> Option<(String, TimeZone)> {
    let name = name.trim().trim_matches('"');
    if let Some(found) = iana(name) {
        return Some(found);
    }
    if let Some((_, iana_name)) = WINDOWS_ZONES
        .iter()
        .find(|(windows, _)| windows.eq_ignore_ascii_case(name))
    {
        if let Some(found) = iana(iana_name) {
            return Some(found);
        }
    }
    // a path that ends in an IANA name
    let parts: Vec<&str> = name.split('/').filter(|part| !part.is_empty()).collect();
    for from in 0..parts.len().saturating_sub(1) {
        if let Some(found) = iana(&parts[from..].join("/")) {
            return Some(found);
        }
    }
    if let Some(offset) = parse_offset(name) {
        return Some((name.to_string(), TimeZone::fixed(offset)));
    }
    if name.contains(',') || name.starts_with('<') {
        if let Ok(zone) = TimeZone::posix(name) {
            return Some((name.to_string(), zone));
        }
    }
    None
}

/// `+0530`, `-05:00`, `+053000`: a UTC offset (TZOFFSETFROM, TZOFFSETTO).
pub(crate) fn parse_offset(text: &str) -> Option<Offset> {
    let text = text.trim();
    let (sign, rest) = match text.chars().next()? {
        '+' => (1, &text[1..]),
        '-' => (-1, &text[1..]),
        _ => return None,
    };
    let digits: String = rest.chars().filter(|ch| *ch != ':').collect();
    if !(digits.len() == 4 || digits.len() == 6) || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let hours: i32 = digits[0..2].parse().ok()?;
    let minutes: i32 = digits[2..4].parse().ok()?;
    let seconds: i32 = if digits.len() == 6 {
        digits[4..6].parse().ok()?
    } else {
        0
    };
    Offset::from_seconds(sign * (hours * 3600 + minutes * 60 + seconds)).ok()
}

fn offset_text(offset: Offset) -> String {
    let seconds = offset.seconds();
    let sign = if seconds < 0 { '-' } else { '+' };
    let seconds = seconds.abs();
    if seconds % 60 == 0 {
        format!("{sign}{:02}{:02}", seconds / 3600, seconds % 3600 / 60)
    } else {
        format!(
            "{sign}{:02}{:02}{:02}",
            seconds / 3600,
            seconds % 3600 / 60,
            seconds % 60
        )
    }
}

/// One STANDARD or DAYLIGHT observance of a VTIMEZONE.
#[derive(Debug, Clone)]
pub(crate) struct Observance {
    pub(crate) daylight: bool,
    /// The onset on the wall clock before it.
    pub(crate) start: DateTime,
    pub(crate) offset_to: Offset,
    pub(crate) rrule: Option<String>,
}

impl Observance {
    pub(crate) fn from_properties(daylight: bool, properties: &[Property]) -> Option<Self> {
        let value = |name: &str| {
            properties
                .iter()
                .find(|property| property.name == name)
                .map(|property| property.value.as_str())
        };
        let start = match rrule::parse_date_value(value("DTSTART")?)? {
            Until::Local(local) => local,
            Until::Date(date) => date.to_datetime(jiff::civil::Time::midnight()),
            Until::Utc(at) => at.to_zoned(TimeZone::UTC).datetime(),
        };
        Some(Observance {
            daylight,
            start,
            offset_to: parse_offset(value("TZOFFSETTO")?)?,
            rrule: value("RRULE").map(str::to_string),
        })
    }
}

/// The zone of a VTIMEZONE as a name `zone_by_name` reads back: a POSIX TZ rule for the observances' latest standard
/// and daylight rules (`<-0500>5:00:00<-0400>4:00:00,M3.2.0/2:00:00,M11.1.0/2:00:00`), or the fixed offset of the
/// latest standard time (`+05:30`).
fn posix_rule(observances: &[Observance]) -> Option<String> {
    let latest = |daylight: bool| {
        observances
            .iter()
            .filter(|observance| observance.daylight == daylight)
            .max_by_key(|observance| (observance.rrule.is_some(), observance.start))
    };
    let standard = latest(false).or_else(|| latest(true))?;
    let fixed = || {
        let seconds = standard.offset_to.seconds();
        let sign = if seconds < 0 { '-' } else { '+' };
        let seconds = seconds.abs();
        Some(format!("{sign}{:02}:{:02}", seconds / 3600, seconds % 3600 / 60))
    };
    let Some(daylight) =
        latest(true).filter(|daylight| daylight.rrule.is_some() && standard.rrule.is_some() && !standard.daylight)
    else {
        return fixed();
    };
    let posix_offset = |offset: Offset| {
        // POSIX counts west of UTC as positive
        let seconds = -offset.seconds();
        let sign = if seconds < 0 { "-" } else { "" };
        let seconds = seconds.abs();
        format!(
            "{sign}{}:{:02}:{:02}",
            seconds / 3600,
            seconds % 3600 / 60,
            seconds % 60
        )
    };
    let name = |offset: Offset| format!("<{}>", offset_text(offset));
    let transition = |observance: &Observance| -> Option<String> {
        let rule = Rule::parse(observance.rrule.as_deref()?).ok()?;
        let month = *rule.by_month.first()?;
        let (ordinal, day) = *rule.by_day.first()?;
        let week = if ordinal < 0 { 5 } else { ordinal.clamp(1, 5) };
        let weekday = day.to_sunday_zero_offset();
        let time = observance.start.time();
        Some(format!(
            "M{month}.{week}.{weekday}/{}:{:02}:{:02}",
            time.hour(),
            time.minute(),
            time.second()
        ))
    };
    match (transition(daylight), transition(standard)) {
        (Some(to_daylight), Some(to_standard)) => Some(format!(
            "{}{}{}{},{to_daylight},{to_standard}",
            name(standard.offset_to),
            posix_offset(standard.offset_to),
            name(daylight.offset_to),
            posix_offset(daylight.offset_to)
        )),
        _ => fixed(),
    }
}

/// The IANA zone with the same offsets as `zone` over two years from `year`, preferring one whose city is named in
/// `tzid` (Outlook's "(UTC+01:00) Amsterdam, Berlin, Bern, Rome").
fn matching_iana(zone: &TimeZone, tzid: &str, year: i16) -> Option<(String, TimeZone)> {
    let start = Date::new(year, 1, 1).ok()?.to_zoned(TimeZone::UTC).ok()?.timestamp();
    let samples: Vec<Timestamp> = (0..105)
        .filter_map(|week| start.checked_add((week * 7 * 24).hours()).ok())
        .collect();
    let wanted: Vec<Offset> = samples.iter().map(|at| zone.to_offset(*at)).collect();
    let tzid_words = tzid.to_ascii_lowercase();
    let mut best: Option<(i32, String, TimeZone)> = None;
    for name in jiff::tz::db().available() {
        let name = name.as_str();
        let region = [
            "Africa/",
            "America/",
            "Antarctica/",
            "Asia/",
            "Atlantic/",
            "Australia/",
            "Europe/",
            "Indian/",
            "Pacific/",
        ]
        .iter()
        .any(|prefix| name.starts_with(prefix));
        if !region && !name.starts_with("Etc/") {
            continue;
        }
        let Ok(candidate) = TimeZone::get(name) else { continue };
        if samples
            .iter()
            .zip(&wanted)
            .any(|(at, offset)| candidate.to_offset(*at) != *offset)
        {
            continue;
        }
        let city = name
            .rsplit('/')
            .next()
            .unwrap_or(name)
            .replace('_', " ")
            .to_ascii_lowercase();
        let score = if tzid_words.contains(&city) { 10 } else { 0 } + i32::from(region);
        if best.as_ref().is_none_or(|(have, _, _)| score > *have) {
            best = Some((score, name.to_string(), candidate));
        }
    }
    best.map(|(_, name, zone)| (name, zone))
}

/// The zones of one file: its VTIMEZONEs by TZID, and each TZID resolved once.
#[derive(Default)]
pub(crate) struct Zones {
    pub(crate) definitions: HashMap<String, Vec<Observance>>,
    resolved: std::cell::RefCell<HashMap<String, Option<(String, TimeZone)>>>,
    /// The year whose offsets a VTIMEZONE is matched against IANA zones on.
    pub(crate) year: i16,
}

impl Zones {
    pub(crate) fn new(year: i16) -> Self {
        Zones {
            year,
            ..Default::default()
        }
    }

    /// A TZID as a zone and the name to keep for it (IANA when there is one), or None.
    pub(crate) fn resolve(&self, tzid: &str) -> Option<(String, TimeZone)> {
        let tzid = tzid.trim().trim_matches('"');
        if let Some(found) = self.resolved.borrow().get(tzid) {
            return found.clone();
        }
        let found = zone_by_name(tzid).or_else(|| {
            let rule = posix_rule(self.definitions.get(tzid)?)?;
            let (_, zone) = zone_by_name(&rule)?;
            // no IANA zone has these offsets: keep the rule itself
            matching_iana(&zone, tzid, self.year).or(Some((rule, zone)))
        });
        if found.is_none() {
            tracing::warn!("iCalendar TZID {tzid} is not a known time zone and has no usable VTIMEZONE; its times are read in the calendar's zone");
        }
        self.resolved.borrow_mut().insert(tzid.to_string(), found.clone());
        found
    }
}

/// The VTIMEZONE of an IANA zone for events from `from_year` to `to_year`: its transitions in those years as yearly
/// rules where they follow one (the same month, weekday and time each year), else as dated onsets.
pub(crate) fn vtimezone(name: &str, zone: &TimeZone, from_year: i16, to_year: i16) -> Vec<String> {
    let mut lines = vec!["BEGIN:VTIMEZONE".to_string(), format!("TZID:{name}")];
    let begin = Date::new(from_year, 1, 1)
        .ok()
        .and_then(|date| date.to_zoned(TimeZone::UTC).ok())
        .map(|zoned| zoned.timestamp());
    let end = Date::new(to_year.saturating_add(1), 12, 31)
        .ok()
        .and_then(|date| date.to_zoned(TimeZone::UTC).ok())
        .map(|zoned| zoned.timestamp());
    struct Onset {
        daylight: bool,
        from: Offset,
        to: Offset,
        local: DateTime,
        name: String,
    }
    let mut onsets = Vec::new();
    if let (Some(begin), Some(end)) = (begin, end) {
        for transition in zone.following(begin) {
            let at = transition.timestamp();
            if at > end || onsets.len() > 400 {
                break;
            }
            let from = zone.to_offset(at.checked_sub(1.second()).unwrap_or(at));
            onsets.push(Onset {
                daylight: transition.dst().is_dst(),
                from,
                to: transition.offset(),
                local: at.to_zoned(TimeZone::fixed(from)).datetime(),
                name: transition.abbreviation().to_string(),
            });
        }
    }
    if onsets.is_empty() {
        let at = begin.unwrap_or_else(Timestamp::now);
        let offset = zone.to_offset(at);
        let info = zone.to_offset_info(at);
        lines.extend([
            "BEGIN:STANDARD".to_string(),
            "DTSTART:19700101T000000".to_string(),
            format!("TZOFFSETFROM:{}", offset_text(offset)),
            format!("TZOFFSETTO:{}", offset_text(offset)),
            format!("TZNAME:{}", info.abbreviation()),
            "END:STANDARD".to_string(),
            "END:VTIMEZONE".to_string(),
        ]);
        return lines;
    }
    // group the onsets that share their kind, offsets and name
    let mut groups: Vec<Vec<&Onset>> = Vec::new();
    for onset in &onsets {
        match groups.iter_mut().find(|group| {
            let first = group[0];
            first.daylight == onset.daylight
                && first.from == onset.from
                && first.to == onset.to
                && first.name == onset.name
        }) {
            Some(group) => group.push(onset),
            None => groups.push(vec![onset]),
        }
    }
    let local_text = |local: &DateTime| local.strftime("%Y%m%dT%H%M%S").to_string();
    for group in groups {
        let first = group[0];
        let kind = if first.daylight { "DAYLIGHT" } else { "STANDARD" };
        lines.push(format!("BEGIN:{kind}"));
        lines.push(format!("DTSTART:{}", local_text(&first.local)));
        let rule = yearly_rule(&group.iter().map(|onset| onset.local).collect::<Vec<_>>());
        match rule {
            Some(rule) => lines.push(format!("RRULE:{rule}")),
            None if group.len() > 1 => lines.push(format!(
                "RDATE:{}",
                group[1..]
                    .iter()
                    .map(|onset| local_text(&onset.local))
                    .collect::<Vec<_>>()
                    .join(",")
            )),
            None => {}
        }
        lines.push(format!("TZOFFSETFROM:{}", offset_text(first.from)));
        lines.push(format!("TZOFFSETTO:{}", offset_text(first.to)));
        if !first.name.is_empty() {
            lines.push(format!("TZNAME:{}", super::line::escape(&first.name)));
        }
        lines.push(format!("END:{kind}"));
    }
    lines.push("END:VTIMEZONE".to_string());
    lines
}

/// `FREQ=YEARLY;BYMONTH=3;BYDAY=2SU` when the onsets are one a year, in consecutive years, on the same nth (or last)
/// weekday of the same month at the same time.
fn yearly_rule(onsets: &[DateTime]) -> Option<String> {
    if onsets.len() < 2 {
        return None;
    }
    let first = onsets[0];
    let consecutive = onsets.windows(2).all(|pair| pair[1].year() == pair[0].year() + 1);
    let same = onsets.iter().all(|onset| {
        onset.month() == first.month() && onset.weekday() == first.weekday() && onset.time() == first.time()
    });
    if !consecutive || !same {
        return None;
    }
    let last = onsets
        .iter()
        .all(|onset| i32::from(onset.day()) + 7 > i32::from(onset.date().days_in_month()));
    let nth = (first.day() - 1) / 7 + 1;
    let ordinal = if onsets.iter().all(|onset| (onset.day() - 1) / 7 + 1 == nth) && nth <= 4 {
        nth.to_string()
    } else if last {
        "-1".to_string()
    } else {
        return None;
    };
    Some(format!(
        "FREQ=YEARLY;BYMONTH={};BYDAY={ordinal}{}",
        first.month(),
        rrule::weekday_code(first.weekday())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tzids_resolve_by_iana_windows_and_path_names() {
        assert_eq!(zone_by_name("America/Chicago").unwrap().0, "America/Chicago");
        assert_eq!(zone_by_name("Central Standard Time").unwrap().0, "America/Chicago");
        assert_eq!(zone_by_name("W. Europe Standard Time").unwrap().0, "Europe/Berlin");
        assert_eq!(
            zone_by_name("/mozilla.org/20050126_1/America/New_York").unwrap().0,
            "America/New_York"
        );
        assert_eq!(
            zone_by_name("+05:30")
                .unwrap()
                .1
                .to_offset(Timestamp::UNIX_EPOCH)
                .seconds(),
            19_800
        );
        assert!(zone_by_name("Somewhere Standard Time").is_none());
    }

    #[test]
    fn a_vtimezone_becomes_the_iana_zone_with_its_offsets() {
        let observances = vec![
            Observance {
                daylight: false,
                start: "1601-10-28T03:00:00".parse().unwrap(),
                offset_to: parse_offset("+0100").unwrap(),
                rrule: Some("FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10".to_string()),
            },
            Observance {
                daylight: true,
                start: "1601-03-25T02:00:00".parse().unwrap(),
                offset_to: parse_offset("+0200").unwrap(),
                rrule: Some("FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3".to_string()),
            },
        ];
        let mut zones = Zones::new(2026);
        zones.definitions.insert(
            "(UTC+01:00) Amsterdam, Berlin, Bern, Rome, Stockholm, Vienna".to_string(),
            observances,
        );
        let (name, _) = zones
            .resolve("(UTC+01:00) Amsterdam, Berlin, Bern, Rome, Stockholm, Vienna")
            .unwrap();
        assert!(
            [
                "Europe/Amsterdam",
                "Europe/Berlin",
                "Europe/Rome",
                "Europe/Stockholm",
                "Europe/Vienna"
            ]
            .contains(&name.as_str()),
            "{name}"
        );
    }

    #[test]
    fn written_vtimezones_use_yearly_rules() {
        let zone = TimeZone::get("America/New_York").unwrap();
        let text = vtimezone("America/New_York", &zone, 2026, 2027).join("\n");
        assert!(text.contains("BEGIN:DAYLIGHT\nDTSTART:20260308T020000\nRRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=2SU\nTZOFFSETFROM:-0500\nTZOFFSETTO:-0400\nTZNAME:EDT"), "{text}");
        assert!(text.contains("BEGIN:STANDARD\nDTSTART:20261101T020000\nRRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=1SU\nTZOFFSETFROM:-0400\nTZOFFSETTO:-0500"), "{text}");
        let berlin = vtimezone("Europe/Berlin", &TimeZone::get("Europe/Berlin").unwrap(), 2026, 2027).join("\n");
        assert!(berlin.contains("RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU"), "{berlin}");
        let tokyo = vtimezone("Asia/Tokyo", &TimeZone::get("Asia/Tokyo").unwrap(), 2026, 2026).join("\n");
        assert!(tokyo.contains("TZOFFSETFROM:+0900\nTZOFFSETTO:+0900"), "{tokyo}");
    }
}
