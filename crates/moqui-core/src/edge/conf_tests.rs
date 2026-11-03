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

fn conf(text: &str) -> MNode {
    MNode::parse_text("moqui-conf", text).unwrap()
}

#[test]
fn the_edge_conf_reads_its_lists_and_limits() {
    let settings = EdgeSettings::parse(
        &conf(
            r#"<moqui-conf><edge queue-retention-days="3" queue-max-megabytes="2" queue-max-attempts="4"
                    queue-retry-seconds="0.25">
                <delegate service="a.B.call#One"/>
                <delegate service="create-moqui.test.Thing" allow-anonymous="true"/>
                <delegate service=" "/>
                <queue-kind name="echo" service="a.B.apply#Echo"/>
                <queue-kind name="nothing"/>
                <replicate set="groups"><entity name="moqui.security.UserGroup" fields="description, groupTypeEnumId"/></replicate>
            </edge></moqui-conf>"#,
        ),
        NodeRole::Edge,
    );
    assert!(settings.edge_node());
    assert!(settings.takes_links());
    assert!(!settings.all_in_one());
    assert!(settings.delegates("a.B.call#One"));
    assert_eq!(
        settings.delegate("a.B.call#One").map(|entry| entry.allow_anonymous),
        Some(false)
    );
    assert_eq!(
        settings
            .delegate("create#moqui.test.Thing")
            .map(|entry| entry.allow_anonymous),
        Some(true),
        "named as the service facade names it"
    );
    assert_eq!(settings.delegates.len(), 2);
    assert_eq!(settings.kinds.get("echo").map(String::as_str), Some("a.B.apply#Echo"));
    assert_eq!(settings.kinds.len(), 1, "a kind with no service is left out");
    assert_eq!(settings.retention_days, 3);
    assert_eq!(settings.max_bytes, 2 * 1024 * 1024);
    assert_eq!(settings.max_attempts, 4);
    assert_eq!(settings.retry_first, Duration::from_millis(250));
    assert_eq!(
        settings.sets,
        vec![ReplicaSet {
            name: "groups".to_string(),
            entities: vec![ReplicaEntity {
                name: "moqui.security.UserGroup".to_string(),
                fields: vec!["description".to_string(), "groupTypeEnumId".to_string()],
            }],
        }]
    );
}

#[test]
fn no_edge_conf_is_the_defaults_and_a_bad_number_keeps_its_default() {
    let settings = EdgeSettings::parse(&conf("<moqui-conf/>"), NodeRole::Normal);
    assert_eq!(settings, EdgeSettings::default());
    let settings = EdgeSettings::parse(
        &conf(r#"<moqui-conf><edge queue-retention-days="many" queue-retry-seconds="-1"/></moqui-conf>"#),
        NodeRole::Normal,
    );
    assert_eq!(settings.retention_days, 14);
    assert_eq!(settings.retry_first, Duration::from_secs(5));
}

#[test]
fn the_retry_wait_doubles_up_to_ten_minutes() {
    let settings = EdgeSettings::default();
    assert_eq!(settings.retry_after(1), Duration::from_secs(5));
    assert_eq!(settings.retry_after(2), Duration::from_secs(10));
    assert_eq!(settings.retry_after(4), Duration::from_secs(40));
    assert_eq!(settings.retry_after(30), MAX_RETRY);
}
