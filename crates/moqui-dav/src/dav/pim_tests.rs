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

use super::{asked_hrefs, ical_utc_millis, multiget_matches, text_of, xml};

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

fn object(name: &str) -> IndexMap<String, Value> {
    let mut row = IndexMap::new();
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
    let asked = asked_hrefs(&body);
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
