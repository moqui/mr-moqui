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

use super::*;

fn entry(section: Section, name: &str, state: State) -> Entry {
    Entry {
        section,
        name: name.to_string(),
        state,
    }
}

fn header() -> Header {
    Header {
        version: "0.1.0",
        package: "maximal",
        seconds: 1.25,
        purpose: "dev".to_string(),
        role: String::new(),
    }
}

#[test]
fn the_report_orders_listeners_and_lists_what_is_not_built() {
    let entries = [
        entry(Section::Listener, "SSH", State::On("tcp 127.0.0.1:2222".into())),
        entry(Section::Integration, "Search", State::on("Postgres")),
        entry(Section::Listener, "HTTP", State::On("tcp 0.0.0.0:8080".into())),
        entry(
            Section::Listener,
            "SIP",
            State::off("sip_port and sip_tls_port are not set"),
        ),
        entry(Section::Integration, "IPP printing", State::NotBuilt),
        entry(Section::Integration, "SSO", State::on("")),
    ];
    let (text, warn) = render(&header(), &entries, &["VCard".to_string()], "/health, /metrics");
    assert!(!warn);
    assert_eq!(
        text,
        "Moqui 0.1.0 (maximal) ready in 1.250 seconds, instance purpose dev, node role all
  Listeners
    HTTP    tcp 0.0.0.0:8080
    SSH     tcp 127.0.0.1:2222
    SIP     off: sip_port and sip_tls_port are not set
  Integrations
    Search  on: Postgres
    SSO     on
  Tools: VCard
  Not in this build: IPP printing
  Routes: /health, /metrics"
    );
}

#[test]
fn a_skipped_or_failed_entry_makes_the_report_a_warning() {
    let skipped = [entry(
        Section::Listener,
        "SIP",
        State::Skipped("port 5060 was busy at the start check".into()),
    )];
    let (text, warn) = render(&header(), &skipped, &[], "");
    assert!(warn);
    assert!(
        text.contains("SIP  skipped: port 5060 was busy at the start check"),
        "{text}"
    );
    let failed = [entry(Section::Integration, "Search", State::failed("no store"))];
    assert!(render(&header(), &failed, &[], "").1);
}
