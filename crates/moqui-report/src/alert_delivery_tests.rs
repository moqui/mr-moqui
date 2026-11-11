use super::*;

#[test]
fn a_notification_links_only_to_a_plain_path_on_this_server() {
    assert!(server_path("/apps/boss/Insights/Alerts/AlertDetail"));
    assert!(!server_path("//evil.example/Alerts"));
    assert!(!server_path("https://evil.example/Alerts"));
    assert!(!server_path("/apps/${evil}"));
    assert!(!server_path("/apps/Alerts?next=x"));
    assert!(!server_path("/apps/../Alerts"));
    assert!(!server_path("apps/Alerts"));
}

#[test]
fn an_audience_names_users_and_groups_and_nobody_named_is_the_owner_alone() {
    let body = json!({"audience": {"recipientUserIds": ["B", " A ", "B", ""], "recipientGroupIds": ["TEAM"]}});
    assert_eq!(
        named(&body),
        (vec!["A".to_owned(), "B".to_owned()], vec!["TEAM".to_owned()])
    );
    assert_eq!(named(&json!({})), (Vec::new(), Vec::new()));
}

#[test]
fn a_business_rule_is_delivered_by_its_audience_channels_and_never_by_a_stored_contact() {
    // a row written past save#Definition: a webhook, a shared contact reference and an unknown channel
    let body = json!({
        "contactPoints": [{"type": "webhook", "url": "https://example.test/hook"}],
        "contactPointRefs": ["ops"], "silenceRefs": ["window"],
        "audience": {"channels": ["email", "webhook", "notification"], "repeatSeconds": 600, "groupBy": ["queue"]},
    });
    let effective = contact_body(&body);
    assert!(effective.get("contactPointRefs").is_none());
    assert_eq!(effective["silenceRefs"], json!(["window"]));
    assert_eq!(
        effective["contactPoints"],
        json!([
            {"id": "audience:notification", "type": "notification", "groupBy": ["queue"], "repeatSeconds": 600},
            {"id": "audience:email", "type": "email", "groupBy": ["queue"], "repeatSeconds": 600},
        ])
    );
    // no audience at all: the owner, in the app
    assert_eq!(
        contact_body(&json!({}))["contactPoints"],
        json!([{"id": "audience:notification", "type": "notification"}])
    );
}
