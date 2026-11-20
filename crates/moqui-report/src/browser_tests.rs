use super::*;

#[test]
fn a_replay_is_held_to_its_owner_time_and_bytes() {
    let budget = ReplayBudget::default();
    budget.charge("replay-aaaaaaaaaaaa", "u/v", 600, 30, 1000).unwrap();
    assert!(
        budget.charge("replay-aaaaaaaaaaaa", "other/v", 1, 30, 1000).is_err(),
        "another session"
    );
    assert!(
        budget.charge("replay-aaaaaaaaaaaa", "u/v", 401, 30, 1000).is_err(),
        "over the bytes"
    );
    budget.charge("replay-aaaaaaaaaaaa", "u/v", 400, 30, 1000).unwrap();
    assert!(
        budget.charge("replay-bbbbbbbbbbbb", "u/v", 1, -1, 1000).is_err(),
        "past the minutes"
    );
}

#[test]
fn replay_ids_and_page_addresses_are_held_to_their_shape() {
    assert!(valid_replay_id("0f3c2a9e-77aa-4d1b-9c2e"));
    assert!(!valid_replay_id("short"));
    assert!(!valid_replay_id("has spaces in it here"));
    assert!(!valid_replay_id("../../../../etc/passwd!"));
    assert_eq!(
        page_path("https://x.test/apps/a?token=secret#frag"),
        "https://x.test/apps/a"
    );
}

#[test]
fn replay_request_caps_default_separately_and_preserve_explicit_legacy_limits() {
    assert_eq!(replay_body_limit("", ""), 1_048_576);
    assert_eq!(
        replay_body_limit("262144", ""),
        262_144,
        "an explicit legacy cap remains binding"
    );
    assert_eq!(
        replay_body_limit("262144", "524288"),
        524_288,
        "only an explicit replay cap overrides it"
    );
    assert_eq!(
        replay_body_limit("65536", "32768"),
        32_768,
        "a smaller replay cap is honored"
    );
    assert_eq!(
        replay_body_limit("0", ""),
        0,
        "legacy zero is not replaced by a default"
    );
    assert_eq!(replay_body_limit("262144", "0"), 0);
}

/// SEC-TLS-110: one user's replays are held to their bytes an hour whatever replay ids the page picks, a chunk of
/// another session never shares the owner's document id, and only the first chunk's session's chunks are read back.
#[test]
fn a_users_replays_are_held_per_hour_and_another_sessions_chunks_are_left_out() {
    let bytes = UserReplayBytes::default();
    bytes.charge("u", 600, 1000).unwrap();
    bytes.charge("other", 600, 1000).unwrap();
    assert!(
        bytes.charge("u", 401, 1000).is_err(),
        "a fresh replay id is the same user's"
    );
    bytes.charge("u", 400, 1000).unwrap();
    assert_ne!(owner_tag("u/v"), owner_tag("other/v"));
    let hit = |seq: i64, received: i64, user: &str| moqui_core::search::SearchHit {
        id: format!("{seq}-{user}"),
        body: [
            ("seq".to_string(), Value::I64(seq)),
            ("received".to_string(), Value::I64(received)),
            ("user.id".to_string(), Value::string(user)),
            ("moqui.visit.id".to_string(), Value::string("v")),
        ]
        .into_iter()
        .collect(),
        document_id: String::new(),
        index: String::new(),
        score: 0.0,
        highlights: indexmap::IndexMap::default(),
    };
    let hits = [
        hit(0, 10, "u"),
        hit(0, 20, "other"),
        hit(1, 30, "u"),
        hit(1, 15, "other"),
    ];
    let kept: Vec<String> = owners_chunks(&hits).iter().map(|hit| hit.id.clone()).collect();
    assert_eq!(kept, ["0-u", "1-u"]);
}

/// SEC-TLS-110: past 4,096 users in an hour the one who sent least recently is let go, never a new user refused; and a
/// replay is listed once, as the chunk first received.
#[test]
fn a_new_user_is_not_refused_for_the_others_number_and_a_replay_is_listed_once() {
    let bytes = UserReplayBytes::default();
    for user in 0..4096 {
        bytes.charge(&format!("u{user}"), 1, 1000).unwrap();
    }
    bytes.charge("one-more", 1, 1000).unwrap();
    assert_eq!(moqui_lib::sync::lock(&bytes.0).len(), 4096);
    let hit = |id: &str, received: i64, user: &str| moqui_core::search::SearchHit {
        document_id: String::new(),
        id: format!("{id}-{user}"),
        index: String::new(),
        score: 0.0,
        body: [
            ("replay_id".to_string(), Value::string(id)),
            ("received".to_string(), Value::I64(received)),
        ]
        .into_iter()
        .collect(),
        highlights: indexmap::IndexMap::default(),
    };
    let hits = [hit("r1", 20, "planted"), hit("r1", 10, "owner"), hit("r2", 5, "other")];
    let listed: Vec<String> = first_chunks(&hits).iter().map(|hit| hit.id.clone()).collect();
    assert_eq!(listed, ["r1-owner", "r2-other"]);
}
