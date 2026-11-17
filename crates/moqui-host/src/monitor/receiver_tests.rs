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

use super::note_refusal;

/// SEC-TLS-17: a refused ingest key counts as a failed sign-in of its client, so a flood of made-up keys bans it as
/// wrong passwords do; no key, a busy receiver and a rate limit do not count.
#[test]
fn a_refused_ingest_key_is_a_failed_sign_in_of_its_client() {
    let guard = crate::guard::tests::guard(
        r#"<web-protection enabled="true">
                <ban name="auth-failure" events="auth-failure" threshold="3" seconds="600" duration="900"/>
            </web-protection>"#,
        "",
    );
    let client: std::net::IpAddr = "203.0.113.5".parse().unwrap();
    let invalid = (401, "ingest key is invalid".to_string());
    for refusal in [
        (401, "an ingest key is required".to_string()),
        (503, "telemetry receiver is busy".to_string()),
        (429, "ingest key rate limit exceeded".to_string()),
    ] {
        let given = refusal.1 != "an ingest key is required";
        for _ in 0..5 {
            note_refusal(&guard, client, given, &refusal);
        }
    }
    assert!(
        guard.admit(client).is_some(),
        "no key, a busy receiver and a rate limit are not failed sign-ins"
    );
    note_refusal(&guard, client, true, &invalid);
    note_refusal(&guard, client, true, &invalid);
    assert!(guard.admit(client).is_some());
    note_refusal(&guard, client, true, &invalid);
    assert!(guard.admit(client).is_none(), "three refused keys ban the client");
}
