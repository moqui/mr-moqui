use super::*;

#[test]
fn an_ingest_key_writes_no_business_event_and_no_release_of_this_servers_own_service() {
    assert!(ingest_refusal(BUSINESS, None, "moqui").is_some());
    assert!(ingest_refusal("release", Some("moqui"), "moqui").is_some());
    assert!(ingest_refusal("release", Some(" moqui "), "moqui").is_some());
    // a sender's own service's release, and the local service's deploy marker, are taken
    assert!(ingest_refusal("release", Some("billing"), "moqui").is_none());
    assert!(ingest_refusal("release", None, "moqui").is_none());
    assert!(ingest_refusal("deploy", Some("moqui"), "moqui").is_none());
}
