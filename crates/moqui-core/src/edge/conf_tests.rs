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
        NodeRole::FrontEdge,
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

/// Balancing's defaults (plans/Edge-Node2.md §4.4): the session cookie, three failures in a row and 30 seconds; a conf
/// sets each, and a bad cookie name or count keeps the default (at least one failure).
#[test]
fn the_balancing_defaults_and_their_conf() {
    let defaults = EdgeSettings::default();
    assert_eq!(defaults.sticky_cookie, "moqui.session");
    assert_eq!(defaults.sticky_cookie, crate::user::SESSION_COOKIE_NAME);
    assert_eq!(
        (defaults.set_aside_failures, defaults.set_aside_for),
        (3, Duration::from_secs(30))
    );
    let settings = EdgeSettings::parse(
        &conf(
            r#"<moqui-conf><edge sticky-cookie="JSESSIONID" set-aside-failures="5" set-aside-seconds="10"/></moqui-conf>"#,
        ),
        NodeRole::FrontEdge,
    );
    assert_eq!(settings.sticky_cookie, "JSESSIONID");
    assert_eq!(
        (settings.set_aside_failures, settings.set_aside_for),
        (5, Duration::from_secs(10))
    );
    let settings = EdgeSettings::parse(
        &conf(r#"<moqui-conf><edge sticky-cookie="a b" set-aside-failures="0" set-aside-seconds="x"/></moqui-conf>"#),
        NodeRole::FrontEdge,
    );
    assert_eq!(settings.sticky_cookie, "moqui.session");
    assert_eq!(
        (settings.set_aside_failures, settings.set_aside_for),
        (1, Duration::from_secs(30))
    );
}

/// A spec step run as an edge node (`withProperty("node_role", ...)`) on a node that started as a normal node read the
/// edge settings first, and every spec after it in that process delegated its calls to a link that was not there. Boot
/// now holds the node to the role it booted with, whatever is read later for another role.
#[test]
fn settings_read_for_another_role_are_not_kept_for_a_normal_node() {
    let ctx = crate::context::ExecutionContextFactory::test_context_with_conf(
        r#"<moqui-conf><edge><delegate service="a.B.call#Remote"/></edge></moqui-conf>"#,
    );
    let factory = ctx.factory();
    let by_role = factory.extension(|_| ByRole::default());
    // a step as an edge node: its role's settings, read and kept for that role
    assert!(read(factory, &by_role, NodeRole::FrontEdge).edge_node());
    // the role it booted with (no node_role in a unit test): a normal node's settings, delegating nothing
    let own = settings(factory);
    assert!(!own.takes_links(), "the step's role was kept: {:?}", own.role);
    assert!(super::super::delegate::listed("a.B.call#Remote").is_none());
    // held to another role, as a test does: that role's from then on
    assert!(settings_as(factory, NodeRole::FrontEdge).edge_node());
    assert!(settings(factory).edge_node());
}
