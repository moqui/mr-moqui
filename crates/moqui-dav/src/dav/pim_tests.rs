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

use indexmap::IndexMap;
use moqui_lib::object::Value;

use super::{
    asked_hrefs, ical_utc_millis, multiget_matches, object_text, text_of, xml, MAX_MULTIGET_HREFS, MAX_OBJECT,
};

/// SEC-FIL-67: a calendar or contact object past its own limit is refused before the adapter parses it.
#[test]
fn an_object_past_its_limit_is_refused_before_it_is_parsed() {
    let put = |body: Vec<u8>| crate::dav::DavRequest {
        method: "PUT".into(),
        body,
        ..crate::dav::DavRequest::default()
    };
    let card = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:A\r\nEND:VCARD\r\n";
    assert_eq!(object_text(&put(card.as_bytes().to_vec())).unwrap(), card);
    assert_eq!(object_text(&put(vec![b'x'; MAX_OBJECT + 1])).unwrap_err().status, 413);
    assert_eq!(object_text(&put(vec![0xff, 0xfe])).unwrap_err().status, 415);
}

#[test]
fn a_time_range_is_read_in_utc() {
    assert_eq!(ical_utc_millis("19700101T000001Z"), Some(1000));
    assert_eq!(ical_utc_millis("20240101T000000Z"), Some(1_704_067_200_000));
    assert_eq!(ical_utc_millis("2024"), None);
    assert_eq!(ical_utc_millis(" 19700102 "), Some(86_400_000));
    assert_eq!(ical_utc_millis("19700101T00000"), None);
    // eight bytes but not eight ASCII digits: None, where an index into the text panicked
    assert_eq!(ical_utc_millis("202\u{e9}1310T000000Z"), None);
    assert_eq!(ical_utc_millis("20240101T0\u{e9}0000Z"), None);
}

fn object(name: &str) -> moqui_lib::object::ValueMap {
    let mut row = IndexMap::default();
    row.insert("name".to_string(), Value::string(name));
    row.insert("etag".to_string(), Value::string(format!("e-{name}")));
    row
}

#[test]
fn a_multiget_over_many_objects_answers_exactly_the_hrefs_asked() {
    let base = "/dav/calendars/john/work%20cal/";
    let objects: Vec<_> = (0..5000).map(|i| object(&format!("ev{i}.ics"))).collect();
    let body = format!(
        r#"<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
             <D:prop><D:getetag/></D:prop>
             <D:href>{base}ev4999.ics</D:href>
             <D:href>https://host.example{base}ev7.ics</D:href>
             <D:href>{base}missing.ics</D:href>
             <D:href>/dav/calendars/john/other/ev1.ics</D:href>
             <D:href>{base}ev%31%32.ics</D:href>
           </C:calendar-multiget>"#
    );
    let body = xml::parse(body.as_bytes()).unwrap().unwrap();
    let asked = asked_hrefs(&body).expect("under the bound");
    assert_eq!(asked.len(), 5);
    assert_eq!(asked[1].0, format!("{base}ev7.ics"), "the scheme and host are dropped");
    assert_eq!(asked[4].1, "ev12.ics", "the name is decoded");

    let answered: Vec<(&str, Option<String>)> = multiget_matches(base, &asked, &objects)
        .into_iter()
        .map(|(path, object)| (path, object.map(|row| text_of(row, "name"))))
        .collect();
    assert_eq!(
        answered,
        vec![
            (asked[0].0.as_str(), Some("ev4999.ics".to_string())),
            (asked[1].0.as_str(), Some("ev7.ics".to_string())),
            (asked[2].0.as_str(), None),
            // an href outside the collection is a 404 even when the name matches an object here
            (asked[3].0.as_str(), None),
            (asked[4].0.as_str(), Some("ev12.ics".to_string())),
        ]
    );
}

/// SEC-FIL-86: a multiget that names one object 40,000 times asks for it once, so its data is answered once; one that
/// names more than `MAX_MULTIGET_HREFS` objects is refused before any is read.
#[test]
fn a_multiget_names_each_object_once_and_a_bounded_number_of_them() {
    let base = "/dav/calendars/john/work/";
    let multiget = |hrefs: &str| {
        let body = format!(
            r#"<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><D:prop><D:getetag/></D:prop>{hrefs}</C:calendar-multiget>"#
        );
        xml::parse(body.as_bytes()).unwrap().unwrap()
    };
    let repeated = format!("<D:href>{base}big.ics</D:href>").repeat(40_000);
    let asked = asked_hrefs(&multiget(&repeated)).expect("one object");
    assert_eq!(asked, vec![(format!("{base}big.ics"), "big.ics".to_string())]);
    let many: String = (0..=MAX_MULTIGET_HREFS)
        .map(|at| format!("<D:href>{base}ev{at}.ics</D:href>"))
        .collect();
    assert!(asked_hrefs(&multiget(&many)).is_none());
    let most: String = (0..MAX_MULTIGET_HREFS)
        .map(|at| format!("<D:href>{base}ev{at}.ics</D:href>"))
        .collect();
    assert_eq!(
        asked_hrefs(&multiget(&most)).map(|asked| asked.len()),
        Some(MAX_MULTIGET_HREFS)
    );
}
