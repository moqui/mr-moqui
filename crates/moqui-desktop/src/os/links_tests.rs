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

use super::{from_args, route, DEFAULT_COMMS_ROOT};
use tauri::Url;

fn node() -> Url {
    Url::parse("http://127.0.0.1:43123/qapps").unwrap()
}

fn routed(link: &str, root: &str) -> Option<(String, Vec<(String, String)>)> {
    let url = route(&Url::parse(link).unwrap(), &node(), root)?;
    assert_eq!(url.origin(), node().origin());
    let pairs = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    Some((url.path().to_string(), pairs))
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

#[test]
fn a_mail_link_opens_compose_with_its_recipients_subject_and_body() {
    let (path, query) = routed(
        "mailto:a@example.com,b@example.com?subject=Hello%20there&body=Line%201%0ALine+2&cc=c@example.com&CC=d@example.com&bcc=e@example.com&attach=x",
        DEFAULT_COMMS_ROOT,
    )
    .unwrap();
    assert_eq!(path, "/qapps/BasicComms/Inbox/Compose");
    assert_eq!(
        query,
        pairs(&[
            ("to", "a@example.com,b@example.com"),
            ("cc", "c@example.com,d@example.com"),
            ("bcc", "e@example.com"),
            ("subject", "Hello there"),
            ("body", "Line 1\nLine+2"),
        ])
    );
}

#[test]
fn a_mail_link_takes_to_headers_and_may_have_no_address() {
    let (_, query) = routed("mailto:?to=a@example.com&to=b%40example.com&subject=Hi", "/qapps/Comms").unwrap();
    assert_eq!(
        query,
        pairs(&[("to", "a@example.com,b@example.com"), ("subject", "Hi")])
    );
    let (path, query) = routed("mailto:", "/qapps/Comms/").unwrap();
    assert_eq!(path, "/qapps/Comms/Inbox/Compose");
    assert!(query.is_empty());
}

#[test]
fn a_named_address_keeps_its_name() {
    let (_, query) = routed("mailto:%22Jo%20Ann%22%20%3Cjo@example.com%3E", DEFAULT_COMMS_ROOT).unwrap();
    assert_eq!(query, pairs(&[("to", "\"Jo Ann\" <jo@example.com>")]));
}

#[test]
fn a_call_link_opens_the_dialer_with_the_number() {
    let (path, query) = routed("tel:+1-555-123-4567;ext=12", "/qapps/Comms").unwrap();
    assert_eq!(path, "/qapps/Comms/Calls/Dialer");
    assert_eq!(query, pairs(&[("number", "+1-555-123-4567")]));
    let (_, query) = routed("sip:+15551234567@sip.example.com;transport=tcp", DEFAULT_COMMS_ROOT).unwrap();
    assert_eq!(query, pairs(&[("number", "+15551234567")]));
    let (_, query) = routed("sip:alice@example.com", DEFAULT_COMMS_ROOT).unwrap();
    assert_eq!(query, pairs(&[("number", "alice@example.com")]));
    assert_eq!(routed("tel:", DEFAULT_COMMS_ROOT), None);
}

#[test]
fn an_empty_comms_root_is_the_default_and_other_schemes_are_not_routed() {
    let (path, _) = routed("tel:123", " ").unwrap();
    assert_eq!(path, "/qapps/BasicComms/Calls/Dialer");
    assert_eq!(routed("https://example.com/", DEFAULT_COMMS_ROOT), None);
}

#[test]
fn links_among_the_arguments_are_those_of_the_schemes() {
    let args = [
        "--no-sandbox",
        "mailto:a@example.com",
        "/tmp/file",
        "https://example.com",
        "tel:123",
    ];
    let links: Vec<String> = from_args(args.map(String::from))
        .iter()
        .map(|url| url.to_string())
        .collect();
    assert_eq!(links, ["mailto:a@example.com", "tel:123"]);
}

#[test]
fn a_launch_keeps_the_apps_own_sso_link() {
    let args = [
        "MR-Moqui",
        "--no-sandbox",
        "org.moqui.desktop:/sso?code=c1&state=s1",
        "https://example.com/",
        "mailto:a@example.com",
    ];
    let links: Vec<String> = from_args(args.map(String::from))
        .into_iter()
        .map(|url| url.to_string())
        .collect();
    assert_eq!(
        links,
        ["org.moqui.desktop:/sso?code=c1&state=s1", "mailto:a@example.com"]
    );
}
