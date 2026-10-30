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
