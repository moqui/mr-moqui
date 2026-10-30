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

//! The framework's `VCard` and `ICalendar` tools: contacts and calendars as files, with no crate beyond jiff (time
//! zones) and encoding_rs (2.1 character sets). `MoquiDefaultConf.xml` selects both (`<tool-factory
//! class="moqui_core::pim::VCardToolFactory"/>`, `...::ICalendarToolFactory`), so `ec.factory.getToolFactory('VCard')`
//! is not null and `ec.factory.getTool('VCard')` gives the tool; a runtime conf can turn one off with
//! `disabled="true"`. Both are stateless.
//!
//! **VCard**:
//! - `parse(text)`: a list of maps, one per contact card of a vCard 2.1, 3.0 or 4.0 file, keyed by the target fields
//!   of Mantle's contact import (the `vcard` module lists them). Folded lines, escapes, TYPE and PREF parameters,
//!   Apple item labels and group cards, quoted-printable values in their CHARSET.
//! - `write(contacts, version)`: vCard text, `4.0` (the default) or `3.0`, from the maps of
//!   `mantle.party.ContactImportServices.export#Contacts` (emails, phones, addresses and websites as lists with a
//!   contactMechPurposeId) or the flat maps `parse` gives.
//!
//! **ICalendar**:
//! - `parse(icsText)`: `{events: [event], method, calendarName, timeZone}`.
//! - `write({method, calendarName, timeZone, attendee, events})`: ICS text; method PUBLISH (the default), REQUEST,
//!   REPLY or CANCEL. Times are written in the event's zone (TZID and a VTIMEZONE) or in UTC; alarms only for PUBLISH;
//!   a CANCEL is STATUS:CANCELLED; for a REPLY, `attendee` (an email) keeps only that attendee, as RFC 5546 asks.
//! - `expand(rrule, dtstart, timeZone, exdates, rdates, rangeStart, rangeEnd)`: the instance starts of a recurrence,
//!   epoch millis ascending, from rangeStart to rangeEnd (both included; either may be null), DTSTART among them as
//!   RFC 5545 counts it (the `rrule` module).
//!
//! The event map: `{uid, sequence, summary, description, location, url, status (CONFIRMED, TENTATIVE or CANCELLED),
//! start and end (epoch millis; end may be null; both null for an all-day event), allDay, startDate and endDate
//! (yyyy-MM-dd, endDate the day after the last day, for an all-day event), timeZone (IANA, else UTC, or a POSIX rule
//! for a VTIMEZONE no IANA zone matches), rrule (the RRULE value), exdates and rdates (lists of RFC 5545 DATE or
//! DATE-TIME text: a day, a time in timeZone, or UTC with Z), recurrenceId (epoch millis: the start of the instance an
//! override replaces), recurrenceRange (`THISANDFUTURE` when the override replaces that instance and every later one,
//! RECURRENCE-ID's RANGE; else null), organizer {name, email}, attendees [{name, email, partstat, role, rsvp, cutype}], alarms
//! [{offsetMinutes (before the start), action}], lastModified}`.

use std::sync::Arc;

use indexmap::IndexMap;
use jiff::tz::TimeZone;
use jiff::Timestamp;
use moqui_lib::object::{to_plain_string, Value};

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Tool, ToolFactory};

pub(crate) mod ics;
pub(crate) mod line;
pub(crate) mod rrule;
pub(crate) mod vcard;
pub(crate) mod zones;

#[cfg(test)]
mod tests;

/// The class `<tool-factory>` names for the VCard tool.
pub const VCARD_CLASS: &str = "moqui_core::pim::VCardToolFactory";
/// The class `<tool-factory>` names for the ICalendar tool.
pub const ICALENDAR_CLASS: &str = "moqui_core::pim::ICalendarToolFactory";

/// The framework's own tool factories, before any component's.
pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.tool(VCARD_CLASS, || Arc::new(VCardToolFactory))?;
    registry.tool(ICALENDAR_CLASS, || Arc::new(ICalendarToolFactory))
}

/// `ec.factory.getTool('VCard')`.
pub struct VCardToolFactory;

impl ToolFactory for VCardToolFactory {
    fn name(&self) -> &str {
        "VCard"
    }

    fn get_instance(&self, _parameters: &[Value]) -> Result<Arc<dyn Tool>, Error> {
        Ok(Arc::new(VCardTool))
    }

    fn type_declaration(&self) -> Option<&'static str> {
        Some(
            "{ parse(text: string): Record<string, string>[]; \
             write(contacts: Record<string, unknown>[], version?: \"4.0\" | \"3.0\"): string }",
        )
    }
}

/// `ec.factory.getTool('ICalendar')`.
pub struct ICalendarToolFactory;

impl ToolFactory for ICalendarToolFactory {
    fn name(&self) -> &str {
        "ICalendar"
    }

    fn get_instance(&self, _parameters: &[Value]) -> Result<Arc<dyn Tool>, Error> {
        Ok(Arc::new(ICalendarTool))
    }

    fn type_declaration(&self) -> Option<&'static str> {
        Some(
            "{ parse(icsText: string): { events: Record<string, unknown>[]; method: string | null; \
             calendarName: string | null; timeZone: string | null }; \
             write(calendar: { method?: string; calendarName?: string; timeZone?: string; attendee?: string; \
             events: Record<string, unknown>[] }): string; \
             expand(rrule: string | null, dtstart: number, timeZone: string | null, exdates?: string[] | null, \
             rdates?: string[] | null, rangeStart?: number | null, rangeEnd?: number | null): number[] }",
        )
    }
}

pub struct VCardTool;

impl Tool for VCardTool {
    fn methods(&self) -> &'static [&'static str] {
        &["parse", "write"]
    }

    fn call(&self, _ec: &ExecutionContext, method: &str, args: &[Value]) -> Result<Value, Error> {
        match method {
            "parse" => {
                let text = args.first().map(to_plain_string).unwrap_or_default();
                Ok(Value::List(vcard::parse(&text).into_iter().map(Value::Map).collect()))
            }
            "write" => {
                let contacts = match args.first() {
                    Some(Value::List(items)) => items.clone(),
                    Some(Value::Map(map)) => vec![Value::Map(map.clone())],
                    Some(Value::Null) | None => Vec::new(),
                    Some(_) => return Err(Error::new("VCard.write takes a list of contact maps")),
                };
                let version = args.get(1).map(to_plain_string).unwrap_or_default();
                vcard::write(&contacts, &version).map(Value::String).map_err(Error::new)
            }
            _ => Err(Error::new(format!("VCard has no method {method}"))),
        }
    }
}

pub struct ICalendarTool;

/// A list of text values: a list, or one comma separated text.
fn text_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::List(items)) => items
            .iter()
            .map(to_plain_string)
            .filter(|text| !text.trim().is_empty())
            .collect(),
        Some(Value::Null) | None => Vec::new(),
        Some(value) => to_plain_string(value)
            .split(',')
            .map(str::to_string)
            .filter(|text| !text.trim().is_empty())
            .collect(),
    }
}

fn timestamp_arg(value: Option<&Value>, name: &str) -> Result<Option<Timestamp>, Error> {
    match value {
        Some(Value::Null) | None => Ok(None),
        Some(value) => {
            let millis = ics::millis_of(value)
                .ok_or_else(|| Error::new(format!("ICalendar.expand: {name} is not epoch millis")))?;
            Timestamp::from_millisecond(millis)
                .map(Some)
                .map_err(|err| Error::new(format!("ICalendar.expand: {name}: {err}")))
        }
    }
}

impl Tool for ICalendarTool {
    fn methods(&self) -> &'static [&'static str] {
        &["parse", "write", "expand"]
    }

    fn call(&self, _ec: &ExecutionContext, method: &str, args: &[Value]) -> Result<Value, Error> {
        match method {
            "parse" => {
                let text = args.first().map(to_plain_string).unwrap_or_default();
                let year = Timestamp::from_millisecond(moqui_core::user::now_effective_millis())
                    .map(|now| now.to_zoned(TimeZone::UTC).year())
                    .unwrap_or(2026);
                Ok(Value::Map(ics::parse(&text, year)))
            }
            "write" => {
                let calendar = match args.first() {
                    Some(Value::Map(map)) => map.clone(),
                    Some(Value::List(events)) => IndexMap::from([("events".to_string(), Value::List(events.clone()))]),
                    _ => {
                        return Err(Error::new(
                            "ICalendar.write takes {method, calendarName, timeZone, events}",
                        ))
                    }
                };
                let now = Timestamp::from_millisecond(moqui_core::user::now_effective_millis())
                    .unwrap_or_else(|_| Timestamp::now());
                ics::write(&calendar, now).map(Value::String).map_err(Error::new)
            }
            "expand" => {
                let rrule = args
                    .first()
                    .map(to_plain_string)
                    .filter(|text| !text.trim().is_empty() && text != "null");
                let dtstart = timestamp_arg(args.get(1), "dtstart")?
                    .ok_or_else(|| Error::new("ICalendar.expand needs dtstart"))?;
                let zone = match args
                    .get(2)
                    .map(to_plain_string)
                    .filter(|name| !name.trim().is_empty() && name != "null")
                {
                    Some(name) => zones::zone_by_name(&name)
                        .map(|(_, zone)| zone)
                        .ok_or_else(|| Error::new(format!("ICalendar.expand: {name} is not a time zone")))?,
                    None => moqui_core::l10n::time_zone(),
                };
                let exdates = text_list(args.get(3));
                let rdates = text_list(args.get(4));
                let range_start = timestamp_arg(args.get(5), "rangeStart")?;
                let range_end = timestamp_arg(args.get(6), "rangeEnd")?;
                let resolve = |name: &str| zones::zone_by_name(name).map(|(_, zone)| zone);
                let starts = rrule::expand_set(
                    rrule.as_deref(),
                    dtstart,
                    &zone,
                    &exdates,
                    &rdates,
                    range_start,
                    range_end,
                    &resolve,
                )
                .map_err(|err| Error::new(format!("ICalendar.expand: {err}")))?;
                Ok(Value::List(
                    starts.into_iter().map(|at| Value::I64(at.as_millisecond())).collect(),
                ))
            }
            _ => Err(Error::new(format!("ICalendar has no method {method}"))),
        }
    }
}
