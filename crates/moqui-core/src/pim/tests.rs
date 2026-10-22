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

//! The tools over exports of Google, Apple and Outlook (testdata/).

use indexmap::IndexMap;
use jiff::tz::TimeZone;
use jiff::Timestamp;
use moqui_lib::object::{to_plain_string, Value};

use super::{ics, vcard};

fn sample(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/pim/testdata")
            .join(name),
    )
    .unwrap()
}

fn text(map: &IndexMap<String, Value>, key: &str) -> String {
    map.get(key).map(to_plain_string).unwrap_or_default()
}

fn local(millis: &Value, zone: &str) -> String {
    let Value::I64(millis) = millis else {
        panic!("{millis:?}")
    };
    Timestamp::from_millisecond(*millis)
        .unwrap()
        .to_zoned(TimeZone::get(zone).unwrap())
        .strftime("%Y-%m-%d %H:%M %:z")
        .to_string()
}

#[test]
fn a_google_vcard_reads_into_the_import_fields() {
    let cards = vcard::parse(&sample("google.vcf"));
    assert_eq!(cards.len(), 2);
    let gina = &cards[0];
    assert_eq!(text(gina, "fullName"), "Gina Googler");
    assert_eq!(text(gina, "firstName"), "Gina");
    assert_eq!(text(gina, "middleName"), "Marie");
    assert_eq!(text(gina, "lastName"), "Googler");
    assert_eq!(text(gina, "prefix"), "Dr.");
    assert_eq!(text(gina, "suffix"), "PhD");
    assert_eq!(text(gina, "nickname"), "Gigi");
    assert_eq!(text(gina, "organizationName"), "Googler Widgets");
    assert_eq!(text(gina, "department"), "Research");
    assert_eq!(text(gina, "jobTitle"), "Chief Widget Officer");
    assert_eq!(text(gina, "birthday"), "1985-04-12");
    assert_eq!(text(gina, "email1"), "gina.home@example.com");
    assert_eq!(text(gina, "email1.label"), "Home");
    assert_eq!(text(gina, "email2"), "gina.googler@example.com");
    assert_eq!(text(gina, "email2.label"), "* Work");
    assert_eq!(text(gina, "phone1.label"), "Mobile");
    assert_eq!(text(gina, "phone2"), "(801) 555-8123");
    assert_eq!(text(gina, "phone2.label"), "Work");
    assert_eq!(text(gina, "phone3.label"), "Home Fax");
    assert_eq!(text(gina, "address1.street"), "123 Main St");
    assert_eq!(text(gina, "address1.street2"), "Building B, Apt 4");
    assert_eq!(text(gina, "address1.city"), "Salt Lake City");
    assert_eq!(text(gina, "address1.region"), "UT");
    assert_eq!(text(gina, "address1.postalCode"), "84101");
    assert_eq!(text(gina, "address1.country"), "United States");
    assert_eq!(text(gina, "address1.label"), "Home");
    assert_eq!(text(gina, "website1"), "https://gina.example.com");
    assert_eq!(text(gina, "notes"), "Met at the fair, 2024\nAsk about widgets");
    assert_eq!(text(gina, "groups"), "* myContacts, Friends, Work");
    assert_eq!(text(&cards[1], "birthday"), "--07-04");
}

#[test]
fn apple_labels_companies_and_groups() {
    let cards = vcard::parse(&sample("apple.vcf"));
    assert_eq!(cards.len(), 2, "the group card is not a contact");
    let johnny = &cards[0];
    assert_eq!(text(johnny, "email1.label"), "* Other");
    assert_eq!(text(johnny, "phone1"), "+1 (408) 555-0199");
    assert_eq!(text(johnny, "phone1.label"), "* iPhone");
    assert_eq!(text(johnny, "phone2.label"), "Work");
    assert_eq!(text(johnny, "address1.city"), "Cupertino");
    assert_eq!(text(johnny, "birthday"), "--06-15");
    assert_eq!(text(johnny, "organizationName"), "Apple Inc.");
    assert_eq!(text(johnny, "groups"), "Hiking Club");
    let acme = &cards[1];
    assert!(
        !acme.contains_key("fullName") && !acme.contains_key("firstName"),
        "{acme:?}"
    );
    assert_eq!(text(acme, "organizationName"), "Acme Rockets");
    assert_eq!(text(acme, "phone1.label"), "Main");
}

#[test]
fn outlook_21_quoted_printable_and_charsets() {
    let cards = vcard::parse(&sample("outlook21.vcf"));
    assert_eq!(cards.len(), 1);
    let card = &cards[0];
    assert_eq!(text(card, "lastName"), "Müller");
    assert_eq!(text(card, "firstName"), "Jürgen");
    assert_eq!(text(card, "fullName"), "Jürgen Müller");
    assert_eq!(text(card, "phone1.label"), "Work");
    assert_eq!(text(card, "phone2.label"), "Mobile");
    assert_eq!(text(card, "address1.street"), "Hauptstraße 1");
    assert_eq!(text(card, "address1.street2"), "Hinterhaus");
    assert_eq!(text(card, "address1.country"), "Deutschland");
    assert_eq!(text(card, "address1.formatted"), "Hauptstraße 1\r\n10115 Berlin");
    assert_eq!(text(card, "email1.label"), "*");
    assert_eq!(text(card, "notes"), "Zeile eins\r\nZeile zwei");
}

#[test]
fn rfc_6350_cards_with_uris_lists_and_organizations() {
    let cards = vcard::parse(&sample("rfc6350.vcf"));
    let simon = &cards[0];
    assert_eq!(text(simon, "suffix"), "ing. jr M.Sc.");
    assert_eq!(text(simon, "birthday"), "--02-03");
    assert_eq!(text(simon, "phone1"), "+1 418 656 9254 ext. 102");
    assert_eq!(text(simon, "phone1.label"), "* Work");
    assert_eq!(text(simon, "phone2.label"), "Mobile");
    assert_eq!(text(simon, "address1.street2"), "Suite D2-630");
    let abc = &cards[1];
    assert_eq!(text(abc, "organizationName"), "ABC, Inc.");
    assert_eq!(text(abc, "department"), "North American Division, Marketing");
}

#[test]
fn vcards_write_as_the_mantle_export_does_and_read_back() {
    let export: Value = serde_json::from_str::<serde_json::Value>(
        r#"{"partyId": "100050", "isOrganization": false, "firstName": "Gina", "middleName": null, "lastName": "Googler",
        "personalTitle": null, "suffix": null, "nickname": null, "organizationName": "Googler Widgets", "jobTitle": "CWO",
        "birthDate": "1985-04-12", "comments": "Met at the fair, 2024", "name": "Gina Googler",
        "emails": [{"emailAddress": "gina.googler@example.com", "contactMechPurposeId": "EmailPrimary"}],
        "phones": [{"countryCode": "1", "areaCode": "801", "contactNumber": "555-8142", "contactMechPurposeId": "PhoneMobile"},
                   {"areaCode": "801", "contactNumber": "555-8123", "contactMechPurposeId": "PhoneWork"}],
        "addresses": [{"address1": "123 Main St", "address2": "Apt 4", "city": "Salt Lake City", "stateGeoCodeAlpha2": "UT",
                       "postalCode": "84101", "countryName": "United States", "contactMechPurposeId": "PostalHome"}],
        "websites": [{"webAddress": "https://gina.example.com"}], "groupNames": ["Friends", "Work, Play"], "isFavorite": true}"#,
    )
    .map(|json| moqui_lib::json::from_json(&json))
    .unwrap();
    let text4 = vcard::write(std::slice::from_ref(&export), "4.0").unwrap();
    // the lines Mantle's ContactBooks spec reads
    assert!(text4.starts_with("BEGIN:VCARD\r\nVERSION:4.0\r\n"), "{text4}");
    for line in [
        "UID:urn:moqui:mantle.party.Party:100050",
        "FN:Gina Googler",
        "N:Googler;Gina;;;",
        "EMAIL;TYPE=home;PREF=1:gina.googler@example.com",
        "TEL;TYPE=cell;PREF=1:+1 801 555-8142",
        "TEL;TYPE=work:801 555-8123",
        "ADR;TYPE=home:;Apt 4;123 Main St;Salt Lake City;UT;84101;United States",
        "BDAY:19850412",
        "NOTE:Met at the fair\\, 2024",
        "ORG:Googler Widgets",
        "CATEGORIES:starred,Friends,Work\\, Play",
    ] {
        assert!(text4.contains(&format!("\r\n{line}\r\n")), "{line} in\n{text4}");
    }
    assert!(text4.ends_with("END:VCARD\r\n"));
    let text3 = vcard::write(std::slice::from_ref(&export), "3.0").unwrap();
    assert!(
        text3.contains("\r\nEMAIL;TYPE=INTERNET,HOME,PREF:gina.googler@example.com\r\n"),
        "{text3}"
    );
    assert!(text3.contains("\r\nBDAY:1985-04-12\r\n"), "{text3}");
    // what was written reads back
    let read = vcard::parse(&text4);
    assert_eq!(text(&read[0], "firstName"), "Gina");
    assert_eq!(text(&read[0], "email1.label"), "* Home");
    assert_eq!(text(&read[0], "phone1.label"), "* Mobile");
    assert_eq!(text(&read[0], "groups"), "* starred, Friends, Work, Play");
    // and a read card writes again
    let again = vcard::write(&[Value::Map(vcard::parse(&sample("google.vcf")).remove(0))], "4.0").unwrap();
    assert!(
        again.contains("\r\nEMAIL;TYPE=work;PREF=1:gina.googler@example.com\r\n"),
        "{again}"
    );
    assert!(again.contains("\r\nTEL;TYPE=fax,home:801 555 8199\r\n"), "{again}");
    assert!(vcard::write(&[], "2.1").is_err());
    assert_eq!(vcard::write(&[], "").unwrap(), "");
}

#[test]
fn a_google_calendar_reads_with_its_series_override_and_all_day_event() {
    let parsed = ics::parse(&sample("google.ics"), 2026);
    assert_eq!(text(&parsed, "calendarName"), "Team Calendar");
    assert_eq!(text(&parsed, "timeZone"), "America/Chicago");
    assert_eq!(text(&parsed, "method"), "PUBLISH");
    let Value::List(events) = &parsed["events"] else {
        panic!()
    };
    assert_eq!(events.len(), 3);
    let Value::Map(series) = &events[0] else { panic!() };
    assert_eq!(text(series, "uid"), "7kukuqrfedlm2f9t6sbe0a3eh8@google.com");
    assert_eq!(series["sequence"], Value::I64(2));
    assert_eq!(text(series, "summary"), "Team sync");
    assert_eq!(text(series, "description"), "Weekly sync, agenda:\n1. Status\n2. Risks");
    assert_eq!(text(series, "location"), "Room 4; Building B");
    assert_eq!(text(series, "status"), "CONFIRMED");
    assert_eq!(local(&series["start"], "America/Chicago"), "2026-10-19 10:00 -05:00");
    assert_eq!(local(&series["end"], "America/Chicago"), "2026-10-19 10:30 -05:00");
    assert_eq!(series["allDay"], Value::Bool(false));
    assert_eq!(text(series, "timeZone"), "America/Chicago");
    assert_eq!(text(series, "rrule"), "FREQ=WEEKLY;BYDAY=MO;UNTIL=20261207T155959Z");
    assert_eq!(series["exdates"], Value::List(vec![Value::string("20261102T100000")]));
    let Value::Map(organizer) = &series["organizer"] else {
        panic!()
    };
    assert_eq!(text(organizer, "name"), "Ada Lovelace");
    assert_eq!(text(organizer, "email"), "ada@example.com");
    let Value::List(attendees) = &series["attendees"] else {
        panic!()
    };
    let Value::Map(bob) = &attendees[1] else { panic!() };
    assert_eq!(text(bob, "partstat"), "NEEDS-ACTION");
    assert_eq!(text(bob, "role"), "OPT-PARTICIPANT");
    assert_eq!(bob["rsvp"], Value::Bool(true));
    let Value::List(alarms) = &series["alarms"] else {
        panic!()
    };
    let Value::Map(alarm) = &alarms[0] else { panic!() };
    assert_eq!(alarm["offsetMinutes"], Value::I64(10));
    assert_eq!(text(alarm, "action"), "DISPLAY");
    assert_eq!(local(&series["lastModified"], "UTC"), "2026-10-15 08:00 +00:00");

    let Value::Map(moved) = &events[1] else { panic!() };
    assert_eq!(
        local(&moved["recurrenceId"], "America/Chicago"),
        "2026-10-26 10:00 -05:00"
    );
    assert_eq!(local(&moved["start"], "America/Chicago"), "2026-10-26 14:00 -05:00");

    let Value::Map(holiday) = &events[2] else { panic!() };
    assert_eq!(holiday["allDay"], Value::Bool(true));
    assert_eq!(holiday["start"], Value::Null);
    assert_eq!(text(holiday, "startDate"), "2026-11-26");
    assert_eq!(text(holiday, "endDate"), "2026-11-28");

    // the series expands with its EXDATE across the change to standard time, to its UNTIL
    let zone = TimeZone::get("America/Chicago").unwrap();
    let Value::I64(start) = series["start"] else { panic!() };
    let starts = super::rrule::expand_set(
        Some(&text(series, "rrule")),
        Timestamp::from_millisecond(start).unwrap(),
        &zone,
        &["20261102T100000".to_string()],
        &[],
        None,
        None,
        &|name| TimeZone::get(name).ok(),
    )
    .unwrap();
    let starts: Vec<String> = starts
        .iter()
        .map(|at| local(&Value::I64(at.as_millisecond()), "America/Chicago"))
        .collect();
    assert_eq!(
        starts,
        [
            "2026-10-19 10:00 -05:00",
            "2026-10-26 10:00 -05:00",
            "2026-11-09 10:00 -06:00",
            "2026-11-16 10:00 -06:00",
            "2026-11-23 10:00 -06:00",
            // UNTIL is 09:59:59 that Monday, before its instance
            "2026-11-30 10:00 -06:00"
        ]
    );
}

#[test]
fn an_outlook_request_with_a_windows_zone() {
    let parsed = ics::parse(&sample("outlook.ics"), 2026);
    assert_eq!(text(&parsed, "method"), "REQUEST");
    let Value::List(events) = &parsed["events"] else {
        panic!()
    };
    let Value::Map(event) = &events[0] else { panic!() };
    assert_eq!(text(event, "timeZone"), "America/Los_Angeles");
    assert_eq!(local(&event["start"], "America/Los_Angeles"), "2026-10-08 09:00 -07:00");
    assert_eq!(text(event, "summary"), "Planning");
    let Value::List(attendees) = &event["attendees"] else {
        panic!()
    };
    let Value::Map(room) = &attendees[1] else { panic!() };
    assert_eq!(text(room, "cutype"), "RESOURCE");
    let Value::List(alarms) = &event["alarms"] else {
        panic!()
    };
    let Value::Map(alarm) = &alarms[0] else { panic!() };
    assert_eq!(alarm["offsetMinutes"], Value::I64(15));
    // the second Thursday of six months, across the change to standard time
    let zone = TimeZone::get("America/Los_Angeles").unwrap();
    let Value::I64(start) = event["start"] else { panic!() };
    let rule = super::rrule::Rule::parse(&text(event, "rrule")).unwrap();
    let starts: Vec<String> = super::rrule::expand(
        &rule,
        Timestamp::from_millisecond(start).unwrap(),
        &zone,
        None,
        None,
        100,
    )
    .iter()
    .map(|at| local(&Value::I64(at.as_millisecond()), "America/Los_Angeles"))
    .collect();
    assert_eq!(
        starts,
        [
            "2026-10-08 09:00 -07:00",
            "2026-11-12 09:00 -08:00",
            "2026-12-10 09:00 -08:00",
            "2027-01-14 09:00 -08:00",
            "2027-02-11 09:00 -08:00",
            "2027-03-11 09:00 -08:00"
        ]
    );
}

#[test]
fn an_apple_calendar_with_a_vtimezone_of_its_own_floating_times_and_alarms() {
    let parsed = ics::parse(&sample("apple.ics"), 2026);
    assert_eq!(text(&parsed, "calendarName"), "Privat");
    let Value::List(events) = &parsed["events"] else {
        panic!()
    };
    let Value::Map(concert) = &events[0] else { panic!() };
    let zone = text(concert, "timeZone");
    assert!(zone.starts_with("Europe/"), "{zone}");
    assert_eq!(local(&concert["start"], "Europe/Berlin"), "2026-10-24 18:30 +02:00");
    assert_eq!(local(&concert["end"], "Europe/Berlin"), "2026-10-24 20:30 +02:00");
    let Value::List(alarms) = &concert["alarms"] else {
        panic!()
    };
    let Value::Map(first) = &alarms[0] else { panic!() };
    assert_eq!(first["offsetMinutes"], Value::I64(30));
    let Value::Map(second) = &alarms[1] else { panic!() };
    assert_eq!(second["offsetMinutes"], Value::I64(-120));
    assert_eq!(text(second, "action"), "EMAIL");
    let Value::Map(birthday) = &events[1] else { panic!() };
    assert_eq!(text(birthday, "startDate"), "2026-06-15");
    assert_eq!(text(birthday, "endDate"), "2026-06-16");
    assert_eq!(text(birthday, "rrule"), "FREQ=YEARLY");
    // a floating time without X-WR-TIMEZONE is UTC
    let Value::Map(floating) = &events[2] else { panic!() };
    assert_eq!(local(&floating["start"], "UTC"), "2026-11-01 09:00 +00:00");
}

#[test]
fn calendars_write_with_vtimezones_and_read_back() {
    let parsed = ics::parse(&sample("google.ics"), 2026);
    let mut calendar = IndexMap::new();
    calendar.insert("method".to_string(), Value::string("PUBLISH"));
    calendar.insert("calendarName".to_string(), Value::string("Team, Calendar"));
    calendar.insert("timeZone".to_string(), Value::string("America/Chicago"));
    calendar.insert("events".to_string(), parsed["events"].clone());
    let now = Timestamp::from_millisecond(1_792_400_000_000).unwrap();
    let written = ics::write(&calendar, now).unwrap();
    assert!(
        written.starts_with("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Moqui Framework//mr-moqui//EN\r\n"),
        "{written}"
    );
    assert!(written.split("\r\n").all(|line| line.len() <= 75), "{written}");
    let unfolded = super::line::unfold(&written);
    for line in [
        "METHOD:PUBLISH",
        "X-WR-CALNAME:Team\\, Calendar",
        "BEGIN:VTIMEZONE",
        "TZID:America/Chicago",
        "RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=2SU",
        "DTSTART;TZID=America/Chicago:20261019T100000",
        "RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261207T155959Z",
        "EXDATE;TZID=America/Chicago:20261102T100000",
        "RECURRENCE-ID;TZID=America/Chicago:20261026T100000",
        "DTSTART;VALUE=DATE:20261126",
        "DTEND;VALUE=DATE:20261128",
        "ORGANIZER;CN=Ada Lovelace:mailto:ada@example.com",
        "ATTENDEE;CN=bob@example.com;CUTYPE=INDIVIDUAL;ROLE=OPT-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:bob@example.com",
        "TRIGGER:-PT10M",
        "LOCATION:Room 4\\; Building B",
        "SEQUENCE:2",
    ] {
        assert!(unfolded.iter().any(|have| have == line), "{line} in\n{written}");
    }
    let again = ics::parse(&written, 2026);
    let (Value::List(before), Value::List(after)) = (&parsed["events"], &again["events"]) else {
        panic!()
    };
    for (one, two) in before.iter().zip(after) {
        let (Value::Map(one), Value::Map(two)) = (one, two) else {
            panic!()
        };
        for key in [
            "uid",
            "start",
            "end",
            "startDate",
            "endDate",
            "rrule",
            "exdates",
            "recurrenceId",
            "summary",
            "timeZone",
        ] {
            assert_eq!(one[key], two[key], "{key}");
        }
    }
    // iMIP: a CANCEL is cancelled and carries no alarms; a UTC event has no VTIMEZONE
    let mut cancel = calendar.clone();
    cancel.insert("method".to_string(), Value::string("CANCEL"));
    cancel.shift_remove("timeZone");
    let Value::List(events) = &calendar["events"] else {
        panic!()
    };
    let Value::Map(mut first) = events[0].clone() else {
        panic!()
    };
    first.insert("timeZone".to_string(), Value::Null);
    cancel.insert("events".to_string(), Value::List(vec![Value::Map(first)]));
    let cancelled = ics::write(&cancel, now).unwrap();
    assert!(
        cancelled.contains("\r\nSTATUS:CANCELLED\r\n")
            && !cancelled.contains("VALARM")
            && !cancelled.contains("VTIMEZONE"),
        "{cancelled}"
    );
    assert!(cancelled.contains("\r\nDTSTART:20261019T150000Z\r\n"), "{cancelled}");
    // a REPLY for one attendee carries that attendee only
    let mut reply = cancel.clone();
    reply.insert("method".to_string(), Value::string("REPLY"));
    reply.insert("attendee".to_string(), Value::string("BOB@example.com"));
    let replied = super::line::unfold(&ics::write(&reply, now).unwrap()).join("\n");
    assert!(
        replied.contains("METHOD:REPLY")
            && replied.contains("mailto:bob@example.com")
            && !replied.contains("ATTENDEE;CN=Ada"),
        "{replied}"
    );
}

#[test]
fn durations_read() {
    assert_eq!(ics::parse_duration("-PT15M"), Some(-900));
    assert_eq!(ics::parse_duration("P1DT2H"), Some(93_600));
    assert_eq!(ics::parse_duration("P2W"), Some(1_209_600));
    assert_eq!(ics::parse_duration("-P0DT0H10M0S"), Some(-600));
    assert_eq!(ics::parse_duration("PT"), None);
    assert_eq!(ics::parse_duration("15M"), None);
}

#[test]
fn to_dos_read_and_write_and_a_calendar_object_has_no_method() {
    let source = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:todo-1\r\nSUMMARY:File the report\r\n\
        DUE;VALUE=DATE:20261030\r\nSTATUS:IN-PROCESS\r\nPERCENT-COMPLETE:40\r\nPRIORITY:1\r\nEND:VTODO\r\n\
        BEGIN:VTODO\r\nUID:todo-2\r\nSUMMARY:Call back\r\nDUE:20261021T150000Z\r\nSTATUS:COMPLETED\r\n\
        COMPLETED:20261021T140000Z\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
    let parsed = ics::parse(source, 2026);
    let Some(Value::List(todos)) = parsed.get("todos") else {
        panic!("{parsed:?}")
    };
    assert_eq!(todos.len(), 2);
    let (Value::Map(first), Value::Map(second)) = (&todos[0], &todos[1]) else {
        panic!()
    };
    assert_eq!(text(first, "dueDate"), "2026-10-30");
    assert_eq!(text(first, "status"), "IN-PROCESS");
    assert_eq!(text(first, "percentComplete"), "40");
    assert_eq!(text(second, "due"), "1792594800000");
    assert_eq!(text(second, "completed"), "1792591200000");
    let mut calendar = IndexMap::new();
    calendar.insert("calendarObject".to_string(), Value::Bool(true));
    calendar.insert("todos".to_string(), Value::List(todos.clone()));
    let written = ics::write(&calendar, Timestamp::from_millisecond(1_792_000_000_000).unwrap()).unwrap();
    assert!(!written.contains("METHOD:"), "{written}");
    assert!(
        written.contains("DUE;VALUE=DATE:20261030") && written.contains("PERCENT-COMPLETE:40"),
        "{written}"
    );
    assert!(
        written.contains("COMPLETED:20261021T140000Z") && written.contains("STATUS:COMPLETED"),
        "{written}"
    );
    let again = ics::parse(&written, 2026);
    let Some(Value::List(back)) = again.get("todos") else {
        panic!()
    };
    assert_eq!(back, todos, "a to-do reads back as it was written");
}

#[test]
fn a_vcard_keeps_its_uid() {
    let cards =
        vcard::parse("BEGIN:VCARD\r\nVERSION:3.0\r\nUID:card-77\r\nFN:Una Uid\r\nN:Uid;Una;;;\r\nEND:VCARD\r\n");
    assert_eq!(text(&cards[0], "uid"), "card-77");
}
