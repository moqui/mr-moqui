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

#[test]
fn a_dialed_number_keeps_its_digits_and_refuses_anything_else() {
    assert_eq!(dial_number(" +1 (801) 555-0100 ").as_deref(), Some("+18015550100"));
    assert_eq!(dial_number("801.555.0100").as_deref(), Some("8015550100"));
    assert_eq!(dial_number("*67#123").as_deref(), Some("*67#123"));
    // a pause, an extension, letters, a + inside, too short or too long: not called
    for text in [
        "8015550100,123",
        "8015550100;ext=12",
        "1-800-FLOWERS",
        "80+15550100",
        "12",
        "",
        &"9".repeat(33),
    ] {
        assert_eq!(dial_number(text), None, "{text}");
    }
}

#[test]
fn a_number_handed_over_is_taken_once_and_not_after_its_time() {
    let mut links = DialLinks::default();
    let nonce = links.make("+1 801 555 0100", 1_000).expect("a number");
    assert_eq!(links.take("not-made", 1_100), None, "a nonce not made");
    assert_eq!(links.take(&nonce, 1_100).as_deref(), Some("+18015550100"));
    assert_eq!(links.take(&nonce, 1_200), None, "taken once");
    let late = links.make("8015550100", 2_000).expect("a number");
    assert_eq!(links.take(&late, 2_000 + LINK_MS), None, "too old");
    assert!(
        links.make("call me", 3_000).is_err(),
        "no nonce for what is not a number"
    );
}

#[test]
fn a_dial_link_is_read_only_with_a_nonce_and_carries_no_number() {
    let url = tauri::Url::parse("org.moqui.mobile:/dial?nonce=abc123").expect("a link");
    assert!(is_dial_link(&url));
    assert_eq!(nonce_of(&url).as_deref(), Some("abc123"));
    let bare = tauri::Url::parse("org.moqui.mobile:/dial?number=%2B18015550100").expect("a link");
    assert_eq!(nonce_of(&bare), None, "a number alone opens nothing");
    let other = tauri::Url::parse("org.moqui.mobile:/call?nonce=abc123").expect("a link");
    assert!(!is_dial_link(&other));
    assert_eq!(nonce_of(&other), None);
}

#[test]
fn the_dialer_path_carries_the_number_and_a_new_handing() {
    assert_eq!(
        dialer_path("/qapps/BasicComms/", "+18015550100", 3),
        "/qapps/BasicComms/Calls/Dialer?number=%2B18015550100&dial=3"
    );
    assert_eq!(
        dialer_path("/qapps/Comms", "*67#1", 4),
        "/qapps/Comms/Calls/Dialer?number=%2A67%231&dial=4"
    );
}

#[test]
fn the_number_is_called_from_the_current_account_or_another_that_can_call() {
    let dir = std::env::temp_dir().join(format!("moqui-mobile-dial-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut accounts = DialAccounts::load(&dir);
    assert!(
        !accounts.any(),
        "none can call at first: the calling account is not registered"
    );
    assert!(accounts.set("a.example.com-1", true));
    assert!(!accounts.set("a.example.com-1", true), "no change");
    accounts.save(&dir).expect("kept");
    let accounts = DialAccounts::load(&dir);
    assert!(accounts.any());
    let signed_in = vec!["b.example.com-2".to_string(), "a.example.com-1".to_string()];
    assert_eq!(
        accounts.choose(Some("a.example.com-1"), &signed_in).as_deref(),
        Some("a.example.com-1")
    );
    assert_eq!(
        accounts.choose(Some("b.example.com-2"), &signed_in).as_deref(),
        Some("a.example.com-1"),
        "the current one has no number: the one that has"
    );
    assert_eq!(
        accounts.choose(None, &[]),
        None,
        "the account that could is not signed in"
    );
    let mut accounts = accounts;
    assert!(accounts.set("a.example.com-1", false), "signed out, or its number gone");
    assert!(!accounts.any(), "the calling account is taken away");
    let _ = std::fs::remove_dir_all(&dir);
}
