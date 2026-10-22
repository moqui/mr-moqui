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

//! iCalendar files (RFC 5545) read into event maps and written from them, with the iMIP methods of RFC 5546
//! (PUBLISH, REQUEST, REPLY, CANCEL). The event map is the one the module doc of [`super`] gives.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use jiff::civil::{Date, DateTime};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};
use moqui_lib::object::{to_plain_string, Value};

use super::line::{self, Property};
use super::rrule::{self, Until};
use super::zones::{self, Observance, Zones};

/// A component and the components in it.
#[derive(Debug, Default)]
struct Component {
    name: String,
    properties: Vec<Property>,
    children: Vec<Component>,
}

impl Component {
    fn first(&self, name: &str) -> Option<&Property> {
        self.properties.iter().find(|property| property.name == name)
    }

    fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Property> + 'a {
        self.properties.iter().filter(move |property| property.name == name)
    }

    fn text(&self, name: &str) -> Option<String> {
        self.first(name)
            .map(|property| line::unescape(&property.value))
            .filter(|text| !text.trim().is_empty())
    }
}

fn components(text: &str) -> Vec<Component> {
    let mut top = Vec::new();
    let mut stack: Vec<Component> = Vec::new();
    for logical in line::unfold(text) {
        let Some(property) = line::parse_line(&logical) else {
            continue;
        };
        match property.name.as_str() {
            "BEGIN" => stack.push(Component {
                name: property.value.trim().to_ascii_uppercase(),
                ..Default::default()
            }),
            "END" => {
                let name = property.value.trim().to_ascii_uppercase();
                // an END without its BEGIN is left out; one that closes an outer component closes those inside too
                if !stack.iter().any(|component| component.name == name) {
                    continue;
                }
                while let Some(component) = stack.pop() {
                    let done = component.name == name;
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(component),
                        None => top.push(component),
                    }
                    if done {
                        break;
                    }
                }
            }
            _ => {
                if let Some(component) = stack.last_mut() {
                    component.properties.push(property);
                }
            }
        }
    }
    // a file cut short keeps what it has
    while let Some(component) = stack.pop() {
        match stack.last_mut() {
            Some(parent) => parent.children.push(component),
            None => top.push(component),
        }
    }
    top
}

/// A DATE or DATE-TIME property: a day, or an instant and the name of the zone it was given in.
#[derive(Debug, Clone)]
enum When {
    Day(Date),
    At(Timestamp, String),
}

struct Reader<'a> {
    zones: &'a Zones,
    /// The calendar's zone for floating times and days (X-WR-TIMEZONE), UTC without one.
    default: (String, TimeZone),
}

impl Reader<'_> {
    /// The zone of a TZID, the calendar's when it is unknown.
    fn zone(&self, tzid: Option<&str>) -> (String, TimeZone) {
        tzid.and_then(|tzid| self.zones.resolve(tzid))
            .unwrap_or_else(|| self.default.clone())
    }

    fn when_value(&self, property: &Property, value: &str) -> Option<When> {
        let is_date = property
            .param("VALUE")
            .is_some_and(|kind| kind.eq_ignore_ascii_case("DATE"));
        match rrule::parse_date_value(value)? {
            Until::Date(date) => Some(When::Day(date)),
            Until::Utc(at) if !is_date => Some(When::At(at, "UTC".to_string())),
            Until::Utc(at) => Some(When::Day(at.to_zoned(TimeZone::UTC).date())),
            Until::Local(local) if is_date => Some(When::Day(local.date())),
            Until::Local(local) => {
                let (name, zone) = self.zone(property.param("TZID"));
                rrule::instant(&zone, local).map(|at| When::At(at, name))
            }
        }
    }

    fn when(&self, property: Option<&Property>) -> Option<When> {
        let property = property?;
        self.when_value(property, property.value.trim())
    }

    /// The start of a day in a zone, as epoch millis.
    fn day_start(&self, date: Date, zone: &TimeZone) -> Option<i64> {
        rrule::instant(zone, date.to_datetime(jiff::civil::Time::midnight())).map(|at| at.as_millisecond())
    }

    /// EXDATE or RDATE values as text `expand` reads with the event's zone: a day `20261020`, a time in the event's
    /// zone `20261020T090000`, else UTC `20261020T140000Z`.
    fn set_dates(&self, event: &Component, name: &str, event_zone: Option<&str>) -> Vec<Value> {
        let mut out = Vec::new();
        for property in event.all(name) {
            for value in property
                .value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                // a PERIOD's start
                let value = value.split('/').next().unwrap_or(value);
                let text = match self.when_value(property, value) {
                    Some(When::Day(date)) => date.strftime("%Y%m%d").to_string(),
                    Some(When::At(at, zone)) if event_zone == Some(zone.as_str()) && zone != "UTC" => {
                        let zone = self.zone(Some(&zone)).1;
                        at.to_zoned(zone).strftime("%Y%m%dT%H%M%S").to_string()
                    }
                    Some(When::At(at, _)) => at.to_zoned(TimeZone::UTC).strftime("%Y%m%dT%H%M%SZ").to_string(),
                    None => {
                        tracing::warn!("iCalendar {name} {value} is not a date and is left out");
                        continue;
                    }
                };
                out.push(Value::String(text));
            }
        }
        out
    }
}

/// An RFC 5545 duration (`-PT15M`, `P1D`, `P1W`, `-P1DT2H30M`) in seconds.
pub(crate) fn parse_duration(text: &str) -> Option<i64> {
    let text = text.trim().to_ascii_uppercase();
    let (sign, rest) = match text.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, text.strip_prefix('+').unwrap_or(&text)),
    };
    let rest = rest.strip_prefix('P')?;
    let mut seconds = 0i64;
    let mut number = String::new();
    let mut in_time = false;
    let mut any = false;
    for ch in rest.chars() {
        match ch {
            '0'..='9' => number.push(ch),
            'T' => in_time = true,
            'W' | 'D' | 'H' | 'M' | 'S' => {
                let value: i64 = number.parse().ok()?;
                number.clear();
                any = true;
                seconds += value
                    * match (ch, in_time) {
                        ('W', _) => 604_800,
                        ('D', _) => 86_400,
                        ('H', true) => 3600,
                        ('M', true) => 60,
                        ('S', true) => 1,
                        _ => return None,
                    };
            }
            _ => return None,
        }
    }
    (any && number.is_empty()).then_some(sign * seconds)
}

fn day_text(date: Date) -> String {
    date.strftime("%Y-%m-%d").to_string()
}

fn person(property: &Property) -> IndexMap<String, Value> {
    let mut out = IndexMap::new();
    let name = property.param("CN").map(str::trim).filter(|name| !name.is_empty());
    out.insert(
        "name".to_string(),
        name.map_or(Value::Null, |name| Value::String(name.to_string())),
    );
    let address = line::unescape(property.value.trim());
    let email = if address.len() >= 7 && address[..7].eq_ignore_ascii_case("mailto:") {
        &address[7..]
    } else {
        address.as_str()
    };
    let email = email.trim();
    out.insert(
        "email".to_string(),
        if email.is_empty() {
            Value::Null
        } else {
            Value::String(email.to_string())
        },
    );
    out
}

fn text_value(text: Option<String>) -> Value {
    text.map_or(Value::Null, Value::String)
}

/// The event map of a VEVENT.
fn event(reader: &Reader<'_>, component: &Component) -> IndexMap<String, Value> {
    let mut out: IndexMap<String, Value> = IndexMap::new();
    out.insert(
        "uid".into(),
        text_value(component.text("UID").map(|uid| uid.trim().to_string())),
    );
    let sequence = component
        .text("SEQUENCE")
        .and_then(|text| text.trim().parse::<i64>().ok())
        .unwrap_or(0);
    out.insert("sequence".into(), Value::I64(sequence));
    out.insert("summary".into(), text_value(component.text("SUMMARY")));
    out.insert("description".into(), text_value(component.text("DESCRIPTION")));
    out.insert("location".into(), text_value(component.text("LOCATION")));
    out.insert(
        "url".into(),
        text_value(
            component
                .first("URL")
                .map(|url| url.value.trim().to_string())
                .filter(|url| !url.is_empty()),
        ),
    );
    out.insert(
        "status".into(),
        text_value(
            component
                .text("STATUS")
                .map(|status| status.trim().to_ascii_uppercase()),
        ),
    );

    let start = reader.when(component.first("DTSTART"));
    let end = reader
        .when(component.first("DTEND"))
        .or_else(|| reader.when(component.first("DUE")));
    let duration = component.text("DURATION").and_then(|text| parse_duration(&text));
    let all_day = matches!(start, Some(When::Day(_)));
    let mut zone_name: Option<String> = None;
    let (mut start_ms, mut end_ms, mut start_date, mut end_date) = (None, None, None, None);
    match &start {
        Some(When::Day(date)) => {
            start_date = Some(*date);
            end_date = match &end {
                Some(When::Day(end)) if end > date => Some(*end),
                Some(When::At(at, zone)) => {
                    let day = at.to_zoned(reader.zone(Some(zone)).1).date();
                    (day > *date).then_some(day)
                }
                _ => None,
            }
            .or_else(|| {
                let days = duration.map_or(1, |seconds| (seconds / 86_400).max(1));
                date.checked_add(days.days()).ok()
            });
            if reader.default.0 != "UTC" {
                zone_name = Some(reader.default.0.clone());
            }
        }
        Some(When::At(at, zone)) => {
            start_ms = Some(at.as_millisecond());
            zone_name = Some(zone.clone());
            end_ms = match &end {
                Some(When::At(end, _)) => Some(end.as_millisecond()),
                Some(When::Day(day)) => reader.day_start(*day, &reader.zone(Some(zone)).1),
                None => duration.map(|seconds| at.as_millisecond() + seconds * 1000),
            };
        }
        None => {}
    }
    out.insert("start".into(), start_ms.map_or(Value::Null, Value::I64));
    out.insert("end".into(), end_ms.map_or(Value::Null, Value::I64));
    out.insert("allDay".into(), Value::Bool(all_day));
    out.insert(
        "startDate".into(),
        start_date.map_or(Value::Null, |date| Value::String(day_text(date))),
    );
    out.insert(
        "endDate".into(),
        end_date.map_or(Value::Null, |date| Value::String(day_text(date))),
    );
    out.insert("timeZone".into(), zone_name.clone().map_or(Value::Null, Value::String));
    out.insert(
        "rrule".into(),
        text_value(
            component
                .first("RRULE")
                .map(|rule| rule.value.trim().to_string())
                .filter(|rule| !rule.is_empty()),
        ),
    );
    out.insert(
        "exdates".into(),
        Value::List(reader.set_dates(component, "EXDATE", zone_name.as_deref())),
    );
    out.insert(
        "rdates".into(),
        Value::List(reader.set_dates(component, "RDATE", zone_name.as_deref())),
    );
    let recurrence_id = match reader.when(component.first("RECURRENCE-ID")) {
        Some(When::At(at, _)) => Some(at.as_millisecond()),
        Some(When::Day(day)) => {
            let zone = zone_name
                .as_deref()
                .map(|name| reader.zone(Some(name)).1)
                .unwrap_or_else(|| reader.default.1.clone());
            reader.day_start(day, &zone)
        }
        None => None,
    };
    out.insert("recurrenceId".into(), recurrence_id.map_or(Value::Null, Value::I64));
    out.insert(
        "organizer".into(),
        component
            .first("ORGANIZER")
            .map_or(Value::Null, |organizer| Value::Map(person(organizer))),
    );
    let attendees = component
        .all("ATTENDEE")
        .map(|attendee| {
            let mut map = person(attendee);
            map.insert(
                "partstat".into(),
                Value::String(
                    attendee
                        .param("PARTSTAT")
                        .map_or("NEEDS-ACTION".to_string(), |value| value.trim().to_ascii_uppercase()),
                ),
            );
            map.insert(
                "role".into(),
                Value::String(
                    attendee
                        .param("ROLE")
                        .map_or("REQ-PARTICIPANT".to_string(), |value| value.trim().to_ascii_uppercase()),
                ),
            );
            map.insert(
                "rsvp".into(),
                Value::Bool(
                    attendee
                        .param("RSVP")
                        .is_some_and(|value| value.trim().eq_ignore_ascii_case("TRUE")),
                ),
            );
            if let Some(kind) = attendee.param("CUTYPE") {
                map.insert("cutype".into(), Value::String(kind.trim().to_ascii_uppercase()));
            }
            Value::Map(map)
        })
        .collect();
    out.insert("attendees".into(), Value::List(attendees));
    let alarms = component
        .children
        .iter()
        .filter(|child| child.name == "VALARM")
        .filter_map(|alarm| {
            let trigger = alarm.first("TRIGGER")?;
            let related_end = trigger
                .param("RELATED")
                .is_some_and(|related| related.eq_ignore_ascii_case("END"));
            let offset_minutes = if trigger
                .param("VALUE")
                .is_some_and(|kind| kind.eq_ignore_ascii_case("DATE-TIME"))
            {
                let Some(When::At(at, _)) = reader.when_value(trigger, trigger.value.trim()) else {
                    return None;
                };
                (start_ms? - at.as_millisecond()) / 60_000
            } else {
                let seconds = parse_duration(&trigger.value)?;
                let length = if related_end {
                    start_ms.zip(end_ms).map_or(0, |(start, end)| (end - start) / 1000)
                } else {
                    0
                };
                -(seconds + length) / 60
            };
            let mut map = IndexMap::new();
            map.insert("offsetMinutes".to_string(), Value::I64(offset_minutes));
            let action = alarm
                .text("ACTION")
                .map_or("DISPLAY".to_string(), |action| action.trim().to_ascii_uppercase());
            map.insert("action".to_string(), Value::String(action));
            Some(Value::Map(map))
        })
        .collect();
    out.insert("alarms".into(), Value::List(alarms));
    let last_modified = match reader.when(component.first("LAST-MODIFIED")) {
        Some(When::At(at, _)) => Some(at.as_millisecond()),
        _ => None,
    };
    out.insert("lastModified".into(), last_modified.map_or(Value::Null, Value::I64));
    out
}

/// The to-do map of a VTODO: `uid`, `sequence`, `summary`, `description`, `status` (NEEDS-ACTION, IN-PROCESS,
/// COMPLETED, CANCELLED), `priority` (1 high to 9 low, 0 none), `percentComplete`, `start` and `due` (epoch millis;
/// `dueDate` yyyy-MM-dd and `allDay` for a due day), `completed`, `timeZone` and `lastModified`.
fn todo(reader: &Reader<'_>, component: &Component) -> IndexMap<String, Value> {
    let mut out: IndexMap<String, Value> = IndexMap::new();
    out.insert(
        "uid".into(),
        text_value(component.text("UID").map(|uid| uid.trim().to_string())),
    );
    let number = |name: &str| component.text(name).and_then(|text| text.trim().parse::<i64>().ok());
    out.insert("sequence".into(), Value::I64(number("SEQUENCE").unwrap_or(0)));
    out.insert("summary".into(), text_value(component.text("SUMMARY")));
    out.insert("description".into(), text_value(component.text("DESCRIPTION")));
    out.insert(
        "status".into(),
        text_value(
            component
                .text("STATUS")
                .map(|status| status.trim().to_ascii_uppercase()),
        ),
    );
    out.insert("priority".into(), number("PRIORITY").map_or(Value::Null, Value::I64));
    out.insert(
        "percentComplete".into(),
        number("PERCENT-COMPLETE").map_or(Value::Null, Value::I64),
    );
    let mut zone_name: Option<String> = None;
    let mut instant = |when: Option<When>| match when {
        Some(When::At(at, zone)) => {
            zone_name.get_or_insert(zone);
            Some(at.as_millisecond())
        }
        _ => None,
    };
    let start = reader.when(component.first("DTSTART"));
    let start_ms = instant(start.clone());
    out.insert("start".into(), start_ms.map_or(Value::Null, Value::I64));
    let due = reader.when(component.first("DUE"));
    let due_day = match &due {
        Some(When::Day(day)) => Some(*day),
        _ => None,
    };
    let due_ms = instant(due).or_else(|| {
        let seconds = component.text("DURATION").and_then(|text| parse_duration(&text))?;
        start_ms.map(|start| start + seconds * 1000)
    });
    out.insert("due".into(), due_ms.map_or(Value::Null, Value::I64));
    out.insert(
        "dueDate".into(),
        due_day.map_or(Value::Null, |day| Value::String(day_text(day))),
    );
    out.insert("allDay".into(), Value::Bool(due_day.is_some()));
    let completed = match reader.when(component.first("COMPLETED")) {
        Some(When::At(at, _)) => Some(at.as_millisecond()),
        _ => None,
    };
    out.insert("completed".into(), completed.map_or(Value::Null, Value::I64));
    out.insert("timeZone".into(), zone_name.map_or(Value::Null, Value::String));
    let last_modified = match reader.when(component.first("LAST-MODIFIED")) {
        Some(When::At(at, _)) => Some(at.as_millisecond()),
        _ => None,
    };
    out.insert("lastModified".into(), last_modified.map_or(Value::Null, Value::I64));
    out
}

/// A calendar file read: `{events, todos, method, calendarName, timeZone}`, the events and to-dos of every VCALENDAR in
/// it (a file of bare VEVENTs or VTODOs too). `year` is the year a VTIMEZONE is matched to an IANA zone on.
pub(crate) fn parse(text: &str, year: i16) -> IndexMap<String, Value> {
    let top = components(text);
    let calendars: Vec<&Component> = top.iter().filter(|component| component.name == "VCALENDAR").collect();
    let loose = Component {
        name: "VCALENDAR".into(),
        children: Vec::new(),
        properties: Vec::new(),
    };
    let mut events = Vec::new();
    let mut todos = Vec::new();
    let mut method = None;
    let mut calendar_name = None;
    let mut time_zone = None;
    let bare: Vec<&Component> = top
        .iter()
        .filter(|component| component.name == "VEVENT" || component.name == "VTODO")
        .collect();
    let mut lists: Vec<(&Component, Vec<&Component>)> = calendars
        .iter()
        .map(|calendar| (*calendar, calendar.children.iter().collect::<Vec<_>>()))
        .collect();
    if !bare.is_empty() {
        lists.push((&loose, bare));
    }
    for (calendar, children) in lists {
        let mut zones = Zones::new(year);
        for vtimezone in children.iter().filter(|child| child.name == "VTIMEZONE") {
            let Some(tzid) = vtimezone.text("TZID") else { continue };
            let observances: Vec<Observance> = vtimezone
                .children
                .iter()
                .filter_map(|observance| match observance.name.as_str() {
                    "STANDARD" => Observance::from_properties(false, &observance.properties),
                    "DAYLIGHT" => Observance::from_properties(true, &observance.properties),
                    _ => None,
                })
                .collect();
            zones.definitions.insert(tzid.trim().to_string(), observances);
        }
        if method.is_none() {
            method = calendar.text("METHOD").map(|text| text.trim().to_ascii_uppercase());
        }
        if calendar_name.is_none() {
            calendar_name = calendar.text("X-WR-CALNAME").map(|text| text.trim().to_string());
        }
        let default = calendar
            .text("X-WR-TIMEZONE")
            .and_then(|name| zones.resolve(&name))
            .unwrap_or_else(|| ("UTC".to_string(), TimeZone::UTC));
        if time_zone.is_none() && default.0 != "UTC" {
            time_zone = Some(default.0.clone());
        }
        let reader = Reader { zones: &zones, default };
        for child in &children {
            match child.name.as_str() {
                "VEVENT" => events.push(Value::Map(event(&reader, child))),
                "VTODO" => todos.push(Value::Map(todo(&reader, child))),
                _ => {}
            }
        }
    }
    let mut out = IndexMap::new();
    out.insert("events".to_string(), Value::List(events));
    out.insert("todos".to_string(), Value::List(todos));
    out.insert("method".to_string(), text_value(method));
    out.insert("calendarName".to_string(), text_value(calendar_name));
    out.insert("timeZone".to_string(), text_value(time_zone));
    out
}

// ===== writing

fn get<'a>(map: &'a IndexMap<String, Value>, key: &str) -> Option<&'a Value> {
    map.get(key).filter(|value| !matches!(value, Value::Null))
}

fn text_of(map: &IndexMap<String, Value>, key: &str) -> Option<String> {
    get(map, key)
        .map(to_plain_string)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// Epoch millis from a number, a time stamp, digits or ISO text (UTC unless it has an offset).
pub(crate) fn millis_of(value: &Value) -> Option<i64> {
    match value {
        Value::I32(number) => Some(i64::from(*number)),
        Value::I64(number) => Some(*number),
        Value::F64(number) if number.is_finite() => Some(number.round() as i64),
        Value::Decimal(number) => number.round().to_string().parse().ok(),
        Value::Timestamp(at) => Some(at.as_millisecond()),
        Value::Date(date) => date
            .to_zoned(TimeZone::UTC)
            .ok()
            .map(|zoned| zoned.timestamp().as_millisecond()),
        Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                return None;
            }
            if let Ok(number) = text.parse::<i64>() {
                return Some(number);
            }
            if let Ok(at) = text.parse::<Timestamp>() {
                return Some(at.as_millisecond());
            }
            let text = text.replace(' ', "T");
            text.parse::<DateTime>()
                .ok()
                .and_then(|local| local.to_zoned(TimeZone::UTC).ok())
                .map(|zoned| zoned.timestamp().as_millisecond())
        }
        _ => None,
    }
}

fn flag(map: &IndexMap<String, Value>, key: &str) -> bool {
    match get(map, key) {
        Some(Value::Bool(flag)) => *flag,
        Some(value) => matches!(
            to_plain_string(value).trim().to_ascii_lowercase().as_str(),
            "true" | "y" | "yes" | "1"
        ),
        None => false,
    }
}

fn utc_text(at: Timestamp) -> String {
    at.to_zoned(TimeZone::UTC).strftime("%Y%m%dT%H%M%SZ").to_string()
}

fn date_value(text: &str) -> Option<Date> {
    let digits: String = text.chars().filter(char::is_ascii_digit).take(8).collect();
    if digits.len() != 8 {
        return None;
    }
    Date::new(
        digits[0..4].parse().ok()?,
        digits[4..6].parse().ok()?,
        digits[6..8].parse().ok()?,
    )
    .ok()
}

/// The zone an event is written in: a known zone other than UTC (TZID and a VTIMEZONE), else None (UTC times).
struct Zone {
    name: String,
    zone: TimeZone,
}

/// One time zone the written file uses, with the years it is used in.
struct Used {
    zone: TimeZone,
    first: i16,
    last: i16,
}

/// `name;TZID=...:value`, `name;VALUE=DATE:value` or `name:...Z` for an instant or a day.
fn date_property(
    name: &str,
    at: Option<Timestamp>,
    day: Option<Date>,
    zone: Option<&Zone>,
    used: &mut BTreeMap<String, Used>,
) -> Option<String> {
    if let Some(day) = day {
        return Some(format!("{name};VALUE=DATE:{}", day.strftime("%Y%m%d")));
    }
    let at = at?;
    match zone {
        Some(zone) => {
            let local = at.to_zoned(zone.zone.clone());
            let year = local.year();
            used.entry(zone.name.clone())
                .and_modify(|entry| {
                    entry.first = entry.first.min(year);
                    entry.last = entry.last.max(year);
                })
                .or_insert(Used {
                    zone: zone.zone.clone(),
                    first: year,
                    last: year,
                });
            Some(format!(
                "{name};TZID={}:{}",
                line::param_value(&zone.name),
                local.strftime("%Y%m%dT%H%M%S")
            ))
        }
        None => Some(format!("{name}:{}", utc_text(at))),
    }
}

/// EXDATE or RDATE properties for value text as `parse` gives it (and as Mantle stores it), grouped by kind.
fn set_date_properties(name: &str, values: &[String], zone: Option<&Zone>, all_day: bool, lines: &mut Vec<String>) {
    let mut days = Vec::new();
    let mut utc = Vec::new();
    let mut local = Vec::new();
    let mut zoned: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for text in values
        .iter()
        .flat_map(|text| text.split(','))
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        if text.len() > 5 && text[..5].eq_ignore_ascii_case("TZID=") {
            if let Some((tzid, value)) = text[5..].rsplit_once(':') {
                zoned.entry(tzid.to_string()).or_default().push(value.to_string());
                continue;
            }
        }
        if text.len() > 9 && text.bytes().all(|byte| byte.is_ascii_digit()) {
            if let Some(at) = text
                .parse::<i64>()
                .ok()
                .and_then(|millis| Timestamp::from_millisecond(millis).ok())
            {
                if all_day {
                    days.push(
                        at.to_zoned(zone.map_or(TimeZone::UTC, |zone| zone.zone.clone()))
                            .strftime("%Y%m%d")
                            .to_string(),
                    );
                } else {
                    utc.push(utc_text(at));
                }
                continue;
            }
        }
        match rrule::parse_date_value(text) {
            Some(Until::Date(date)) => days.push(date.strftime("%Y%m%d").to_string()),
            Some(Until::Utc(at)) => utc.push(utc_text(at)),
            Some(Until::Local(at)) => local.push(at.strftime("%Y%m%dT%H%M%S").to_string()),
            None => tracing::warn!("iCalendar write: {name} {text} is not a date and is left out"),
        }
    }
    if !days.is_empty() {
        lines.push(format!("{name};VALUE=DATE:{}", days.join(",")));
    }
    if !local.is_empty() {
        match zone {
            Some(zone) => lines.push(format!(
                "{name};TZID={}:{}",
                line::param_value(&zone.name),
                local.join(",")
            )),
            None => lines.push(format!("{name}:{}", local.join(","))),
        }
    }
    for (tzid, values) in zoned {
        lines.push(format!("{name};TZID={}:{}", line::param_value(&tzid), values.join(",")));
    }
    if !utc.is_empty() {
        lines.push(format!("{name}:{}", utc.join(",")));
    }
}

fn mailto(person: &IndexMap<String, Value>) -> Option<(String, Option<String>)> {
    let email = text_of(person, "email").or_else(|| text_of(person, "emailAddress"))?;
    let email = if email.len() >= 7 && email[..7].eq_ignore_ascii_case("mailto:") {
        email[7..].to_string()
    } else {
        email
    };
    Some((format!("mailto:{email}"), text_of(person, "name")))
}

fn list<'a>(map: &'a IndexMap<String, Value>, key: &str) -> Vec<&'a IndexMap<String, Value>> {
    match get(map, key) {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::Map(map) => Some(map),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn texts(map: &IndexMap<String, Value>, key: &str) -> Vec<String> {
    match get(map, key) {
        Some(Value::List(items)) => items
            .iter()
            .map(to_plain_string)
            .filter(|text| !text.trim().is_empty())
            .collect(),
        Some(value) => to_plain_string(value)
            .split(',')
            .map(str::to_string)
            .filter(|text| !text.trim().is_empty())
            .collect(),
        None => Vec::new(),
    }
}

/// The ICS text of `{method, calendarName, timeZone, events, todos}`. `now` is the DTSTAMP. With `calendarObject` it
/// is a CalDAV calendar object resource (RFC 4791 §4.1): no METHOD, and the reminders as a published calendar has them.
pub(crate) fn write(calendar: &IndexMap<String, Value>, now: Timestamp) -> Result<String, String> {
    let calendar_object = flag(calendar, "calendarObject");
    let method = text_of(calendar, "method")
        .filter(|_| !calendar_object)
        .unwrap_or_else(|| "PUBLISH".to_string())
        .to_ascii_uppercase();
    if !method.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-') {
        return Err(format!("{method} is not an iTIP method"));
    }
    let calendar_zone = text_of(calendar, "timeZone").and_then(|name| zones::zone_by_name(&name));
    let reply_from = text_of(calendar, "attendee")
        .filter(|_| method == "REPLY" || method == "COUNTER")
        .map(|email| email.trim_start_matches("mailto:").to_string());
    let zone_of = |name: Option<String>| -> Option<Zone> {
        let (name, zone) = name
            .and_then(|name| zones::zone_by_name(&name))
            .or_else(|| calendar_zone.clone())?;
        (name != "UTC" && zone.iana_name().is_some_and(|iana| iana != "UTC" && iana != "Etc/UTC"))
            .then_some(Zone { name, zone })
    };
    let escape = line::escape;
    let mut used: BTreeMap<String, Used> = BTreeMap::new();
    let mut events: Vec<String> = Vec::new();
    let events_in = match get(calendar, "events") {
        Some(Value::List(items)) => items.as_slice(),
        Some(_) => return Err("events is not a list".to_string()),
        None => &[],
    };
    for (index, value) in events_in.iter().enumerate() {
        let Value::Map(event) = value else {
            tracing::warn!("iCalendar write: an event that is not a map is left out");
            continue;
        };
        let mut lines = vec!["BEGIN:VEVENT".to_string()];
        let uid = text_of(event, "uid").unwrap_or_else(|| format!("moqui-{}-{index}", now.as_millisecond()));
        lines.push(format!("UID:{}", escape(&uid)));
        lines.push(format!("DTSTAMP:{}", utc_text(now)));
        let all_day = flag(event, "allDay");
        let zone = zone_of(text_of(event, "timeZone"));
        let start = get(event, "start")
            .and_then(millis_of)
            .and_then(|millis| Timestamp::from_millisecond(millis).ok());
        let end = get(event, "end")
            .and_then(millis_of)
            .and_then(|millis| Timestamp::from_millisecond(millis).ok());
        if all_day {
            let day_of = |at: Timestamp| {
                at.to_zoned(zone.as_ref().map_or(TimeZone::UTC, |zone| zone.zone.clone()))
                    .date()
            };
            let start_day = text_of(event, "startDate")
                .and_then(|text| date_value(&text))
                .or_else(|| start.map(day_of));
            let end_day = text_of(event, "endDate")
                .and_then(|text| date_value(&text))
                .or_else(|| end.map(day_of))
                .or_else(|| start_day.and_then(|day| day.checked_add(1.day()).ok()));
            if let Some(line) = date_property("DTSTART", None, start_day, None, &mut used) {
                lines.push(line);
            }
            if let Some(line) = date_property(
                "DTEND",
                None,
                end_day.filter(|end| start_day.is_none_or(|start| *end > start)),
                None,
                &mut used,
            ) {
                lines.push(line);
            }
        } else {
            if let Some(line) = date_property("DTSTART", start, None, zone.as_ref(), &mut used) {
                lines.push(line);
            }
            if let Some(line) = date_property(
                "DTEND",
                end.filter(|end| start.is_none_or(|start| *end >= start)),
                None,
                zone.as_ref(),
                &mut used,
            ) {
                lines.push(line);
            }
        }
        if let Some(rule) = text_of(event, "rrule") {
            let rule = if rule.len() >= 6 && rule[..6].eq_ignore_ascii_case("RRULE:") {
                rule[6..].to_string()
            } else {
                rule
            };
            lines.push(format!("RRULE:{rule}"));
        }
        set_date_properties("EXDATE", &texts(event, "exdates"), zone.as_ref(), all_day, &mut lines);
        set_date_properties("RDATE", &texts(event, "rdates"), zone.as_ref(), all_day, &mut lines);
        if let Some(at) = get(event, "recurrenceId")
            .and_then(millis_of)
            .and_then(|millis| Timestamp::from_millisecond(millis).ok())
        {
            let day = all_day.then(|| {
                at.to_zoned(zone.as_ref().map_or(TimeZone::UTC, |zone| zone.zone.clone()))
                    .date()
            });
            if let Some(line) = date_property("RECURRENCE-ID", Some(at), day, zone.as_ref(), &mut used) {
                lines.push(line);
            }
        }
        let sequence = get(event, "sequence").and_then(millis_of).unwrap_or(0);
        lines.push(format!("SEQUENCE:{sequence}"));
        for (key, name) in [
            ("summary", "SUMMARY"),
            ("description", "DESCRIPTION"),
            ("location", "LOCATION"),
        ] {
            if let Some(text) = text_of(event, key) {
                lines.push(format!("{name}:{}", escape(&text)));
            }
        }
        if let Some(url) = text_of(event, "url") {
            lines.push(format!("URL:{url}"));
        }
        let status = if method == "CANCEL" {
            Some("CANCELLED".to_string())
        } else {
            text_of(event, "status").map(|status| status.to_ascii_uppercase())
        };
        if let Some(status) = status.filter(|status| ["TENTATIVE", "CONFIRMED", "CANCELLED"].contains(&status.as_str()))
        {
            lines.push(format!("STATUS:{status}"));
        }
        if let Some(Value::Map(organizer)) = get(event, "organizer") {
            if let Some((address, name)) = mailto(organizer) {
                let cn = name
                    .map(|name| format!(";CN={}", line::param_value(&name)))
                    .unwrap_or_default();
                lines.push(format!("ORGANIZER{cn}:{address}"));
            }
        }
        for attendee in list(event, "attendees") {
            let Some((address, name)) = mailto(attendee) else {
                continue;
            };
            // a REPLY carries the replying attendee only (RFC 5546 §3.2.3), when the calendar names one
            if let Some(replier) = &reply_from {
                if !address[7..].eq_ignore_ascii_case(replier) {
                    continue;
                }
            }
            let mut params = String::new();
            if let Some(name) = name {
                params.push_str(&format!(";CN={}", line::param_value(&name)));
            }
            if let Some(kind) = text_of(attendee, "cutype") {
                params.push_str(&format!(";CUTYPE={}", kind.to_ascii_uppercase()));
            }
            params.push_str(&format!(
                ";ROLE={}",
                text_of(attendee, "role")
                    .unwrap_or_else(|| "REQ-PARTICIPANT".into())
                    .to_ascii_uppercase()
            ));
            params.push_str(&format!(
                ";PARTSTAT={}",
                text_of(attendee, "partstat")
                    .unwrap_or_else(|| "NEEDS-ACTION".into())
                    .to_ascii_uppercase()
            ));
            if flag(attendee, "rsvp") {
                params.push_str(";RSVP=TRUE");
            }
            lines.push(format!("ATTENDEE{params}:{address}"));
        }
        if let Some(at) = get(event, "lastModified")
            .and_then(millis_of)
            .and_then(|millis| Timestamp::from_millisecond(millis).ok())
        {
            lines.push(format!("LAST-MODIFIED:{}", utc_text(at)));
        }
        // a reminder is the reader's own: only a published calendar, or the user's own copy over CalDAV, carries them
        if method == "PUBLISH" {
            for alarm in list(event, "alarms") {
                let minutes = get(alarm, "offsetMinutes").and_then(millis_of).unwrap_or(0);
                let action = text_of(alarm, "action")
                    .unwrap_or_else(|| "DISPLAY".into())
                    .to_ascii_uppercase();
                let trigger = match minutes {
                    0 => "PT0S".to_string(),
                    minutes if minutes > 0 => format!("-PT{minutes}M"),
                    minutes => format!("PT{}M", -minutes),
                };
                let summary = escape(&text_of(event, "summary").unwrap_or_else(|| "Reminder".into()));
                lines.push("BEGIN:VALARM".to_string());
                lines.push(format!(
                    "ACTION:{}",
                    if action == "EMAIL" { "EMAIL" } else { "DISPLAY" }
                ));
                lines.push(format!("TRIGGER:{trigger}"));
                lines.push(format!("DESCRIPTION:{summary}"));
                if action == "EMAIL" {
                    lines.push(format!("SUMMARY:{summary}"));
                    if let Some((address, _)) = get(event, "organizer").and_then(|organizer| match organizer {
                        Value::Map(organizer) => mailto(organizer),
                        _ => None,
                    }) {
                        lines.push(format!("ATTENDEE:{address}"));
                    }
                }
                lines.push("END:VALARM".to_string());
            }
        }
        lines.push("END:VEVENT".to_string());
        events.extend(lines);
    }
    let todos_in = match get(calendar, "todos") {
        Some(Value::List(items)) => items.as_slice(),
        Some(_) => return Err("todos is not a list".to_string()),
        None => &[],
    };
    for (index, value) in todos_in.iter().enumerate() {
        let Value::Map(todo) = value else {
            tracing::warn!("iCalendar write: a to-do that is not a map is left out");
            continue;
        };
        let mut lines = vec!["BEGIN:VTODO".to_string()];
        let uid = text_of(todo, "uid").unwrap_or_else(|| format!("moqui-todo-{}-{index}", now.as_millisecond()));
        lines.push(format!("UID:{}", escape(&uid)));
        lines.push(format!("DTSTAMP:{}", utc_text(now)));
        let zone = zone_of(text_of(todo, "timeZone"));
        let at = |key: &str| {
            get(todo, key)
                .and_then(millis_of)
                .and_then(|millis| Timestamp::from_millisecond(millis).ok())
        };
        if let Some(line) = date_property("DTSTART", at("start"), None, zone.as_ref(), &mut used) {
            lines.push(line);
        }
        let due_day = text_of(todo, "dueDate").and_then(|text| date_value(&text));
        let due = if due_day.is_some() { None } else { at("due") };
        if let Some(line) = date_property("DUE", due, due_day, zone.as_ref(), &mut used) {
            lines.push(line);
        }
        if let Some(completed) = at("completed") {
            lines.push(format!("COMPLETED:{}", utc_text(completed)));
        }
        let sequence = get(todo, "sequence").and_then(millis_of).unwrap_or(0);
        lines.push(format!("SEQUENCE:{sequence}"));
        for (key, name) in [("summary", "SUMMARY"), ("description", "DESCRIPTION")] {
            if let Some(text) = text_of(todo, key) {
                lines.push(format!("{name}:{}", escape(&text)));
            }
        }
        if let Some(status) = text_of(todo, "status")
            .map(|status| status.to_ascii_uppercase())
            .filter(|status| ["NEEDS-ACTION", "IN-PROCESS", "COMPLETED", "CANCELLED"].contains(&status.as_str()))
        {
            lines.push(format!("STATUS:{status}"));
        }
        if let Some(priority) = get(todo, "priority")
            .and_then(millis_of)
            .filter(|priority| (0..=9).contains(priority))
        {
            lines.push(format!("PRIORITY:{priority}"));
        }
        if let Some(percent) = get(todo, "percentComplete")
            .and_then(millis_of)
            .filter(|percent| (0..=100).contains(percent))
        {
            lines.push(format!("PERCENT-COMPLETE:{percent}"));
        }
        if let Some(modified) = at("lastModified") {
            lines.push(format!("LAST-MODIFIED:{}", utc_text(modified)));
        }
        lines.push("END:VTODO".to_string());
        events.extend(lines);
    }
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".to_string(),
        "PRODID:-//Moqui Framework//mr-moqui//EN".to_string(),
        "CALSCALE:GREGORIAN".to_string(),
    ];
    if !calendar_object {
        lines.push(format!("METHOD:{method}"));
    }
    if let Some(name) = text_of(calendar, "calendarName") {
        lines.push(format!("X-WR-CALNAME:{}", escape(&name)));
    }
    if let Some((name, _)) = &calendar_zone {
        lines.push(format!("X-WR-TIMEZONE:{name}"));
    }
    for (name, entry) in &used {
        lines.extend(zones::vtimezone(
            name,
            &entry.zone,
            entry.first.saturating_sub(1),
            entry.last.saturating_add(1),
        ));
    }
    lines.extend(events);
    lines.push("END:VCALENDAR".to_string());
    Ok(lines
        .iter()
        .map(|text| line::fold(text))
        .collect::<Vec<_>>()
        .join("\r\n")
        + "\r\n")
}
