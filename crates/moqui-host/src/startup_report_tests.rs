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
        since: jiff::Timestamp::UNIX_EPOCH,
        cause: None,
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
            State::off("sip-server@port and sip-server@tls-port are not set"),
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
    SIP     off: sip-server@port and sip-server@tls-port are not set
  Integrations
    Search  on: Postgres
    SSO     on
  Tools: VCard
  Not in this build: IPP printing
  Routes: /health, /metrics"
    );
}

/// The log's report shows the state alone: a row the listener registry recorded later, with its time and cause, reads
/// as one recorded at startup.
#[test]
fn the_report_leaves_out_the_time_and_cause_of_a_row() {
    let mut caused = entry(Section::Listener, "SSH", State::On("tcp 127.0.0.1:2222".into()));
    caused.since = jiff::Timestamp::now();
    caused.cause = Some("set ssh-server@port".to_string());
    let plain = entry(Section::Listener, "SSH", State::On("tcp 127.0.0.1:2222".into()));
    assert_eq!(
        render(&header(), &[caused], &[], ""),
        render(&header(), &[plain], &[], "")
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

/// The LLM line names the model Assist is bound to, by its id and name, the conf's properties expanded; with the default
/// model bound it is the default model's line.
#[test]
fn the_llm_line_names_the_model_assist_is_bound_to() {
    let conf = r#"<moqui-conf><llm-facade>
        <model id="default" endpoint="default" name=""/>
        <model id="claude-haiku" endpoint="anthropic" name="claude-haiku-5-5"/>
        <bind feature="title" model="default"/>
        <bind feature="assist" model="claude-haiku" reasoning="low"/>
    </llm-facade></moqui-conf>"#;
    assert_eq!(
        assist_model(conf),
        Some(("claude-haiku".to_string(), "claude-haiku-5-5".to_string()))
    );
    let bound_default = conf.replace(r#"model="claude-haiku" reasoning"#, r#"model="default" reasoning"#);
    assert_eq!(assist_model(&bound_default).map(|(id, _)| id), Some("default".to_string()));
    assert_eq!(assist_model("<moqui-conf/>"), None);
}
