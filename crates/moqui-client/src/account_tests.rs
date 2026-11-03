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

fn account(id: &str) -> Account {
    Account {
        id: id.to_string(),
        server: "https://erp.example.com".to_string(),
        username: "john.doe".to_string(),
        user_id: "EX_JOHN_DOE".to_string(),
        display: "John Doe".to_string(),
        server_name: "Example".to_string(),
        device_mode: None,
    }
}

#[test]
fn plain_http_is_for_this_machine_only() {
    assert_eq!(base_url("http://localhost:8080/").unwrap(), "http://localhost:8080");
    assert_eq!(base_url("erp.example.com").unwrap(), "https://erp.example.com");
    assert_eq!(
        base_url(" https://erp.example.com/moqui/ ").unwrap(),
        "https://erp.example.com/moqui"
    );
    assert!(base_url("http://127.0.0.1:8080").is_ok());
    assert!(base_url("http://erp.example.com").is_err());
    assert!(base_url("ftp://localhost").is_err());
    assert!(base_url("https://").is_err());
}

#[test]
fn an_account_id_is_stable_and_apart_per_user_and_server() {
    let one = account_id("https://erp.example.com", "john.doe");
    assert_eq!(one, account_id("https://erp.example.com", "john.doe"));
    assert!(one.starts_with("erp.example.com-"), "{one}");
    assert_ne!(one, account_id("https://erp.example.com", "jane.doe"));
    assert_ne!(one, account_id("https://erp.example.com:8443", "john.doe"));
    assert!(
        one.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'),
        "{one}"
    );
}

#[test]
fn accounts_survive_a_restart_and_removing_the_current_one_clears_it() {
    let dir = std::env::temp_dir().join(format!("moqui-client-accounts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut accounts = Accounts::load(&dir);
    assert!(accounts.list().is_empty() && accounts.current().is_none());
    accounts.put(account("a"));
    accounts.put(account("b"));
    assert!(!accounts.set_current("missing"));
    assert!(accounts.set_current("b"));
    let mut changed = account("a");
    changed.display = "Johnny".to_string();
    accounts.put(changed);
    accounts.save().unwrap();

    let mut again = Accounts::load(&dir);
    assert_eq!(again.list().len(), 2);
    assert_eq!(again.get("a").map(|kept| kept.display.as_str()), Some("Johnny"));
    assert_eq!(again.current().map(|kept| kept.id.as_str()), Some("b"));
    again.remove("b");
    assert!(again.current().is_none());
    assert_eq!(again.list().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_that_does_not_parse_starts_empty() {
    let dir = std::env::temp_dir().join(format!("moqui-client-accounts-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("accounts.json"), "{not json").unwrap();
    assert!(Accounts::load(&dir).list().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_device_mode_is_kept_and_never_lowered() {
    let dir = std::env::temp_dir().join(format!("moqui-client-accounts-mode-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut accounts = Accounts::load(&dir);
    let mut shared = account("a");
    shared.device_mode = Some(DeviceMode::Shared);
    accounts.put(shared);
    accounts.put(account("b"));
    assert_eq!(accounts.get("a").unwrap().device_mode, Some(DeviceMode::Shared));
    assert_eq!(accounts.get("b").unwrap().device_mode, None);

    // a sign-in again with no mode keeps the one chosen; shared may become personal, never back
    accounts.put(account("a"));
    assert_eq!(accounts.get("a").unwrap().device_mode, Some(DeviceMode::Shared));
    assert!(accounts.set_device_mode("a", DeviceMode::Personal));
    assert!(!accounts.set_device_mode("a", DeviceMode::Shared));
    let mut lowered = account("a");
    lowered.device_mode = Some(DeviceMode::Shared);
    accounts.put(lowered);
    assert_eq!(accounts.get("a").unwrap().device_mode, Some(DeviceMode::Personal));
    assert!(!accounts.set_device_mode("b", DeviceMode::Choose));
    assert!(accounts.set_device_mode("b", DeviceMode::Shared));
    assert!(!accounts.set_device_mode("missing", DeviceMode::Personal));
    accounts.save().unwrap();

    let text = std::fs::read_to_string(dir.join("accounts.json")).unwrap();
    assert!(text.contains("\"deviceMode\": \"personal\""), "{text}");
    let again = Accounts::load(&dir);
    assert_eq!(again.get("a").unwrap().device_mode, Some(DeviceMode::Personal));
    assert_eq!(again.get("b").unwrap().device_mode, Some(DeviceMode::Shared));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_accounts_file_of_before_md4_or_of_a_newer_client_still_loads() {
    let dir = std::env::temp_dir().join(format!("moqui-client-accounts-old-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("accounts.json"),
        r#"{"accounts": [
            {"id": "old", "server": "https://erp.example.com", "username": "john.doe"},
            {"id": "new", "server": "https://erp.example.com", "username": "jane.doe", "deviceMode": "kiosk"},
            {"id": "choose", "server": "https://erp.example.com", "username": "joe", "deviceMode": "choose"},
            {"id": "number", "server": "https://erp.example.com", "username": "jim", "deviceMode": 3},
            {"id": "object", "server": "https://erp.example.com", "username": "jo", "deviceMode": {"a": 1}},
            {"id": "null", "server": "https://erp.example.com", "username": "al", "deviceMode": null}
        ], "current": "old"}"#,
    )
    .unwrap();
    let accounts = Accounts::load(&dir);
    assert_eq!(accounts.list().len(), 6);
    assert!(accounts.list().iter().all(|kept| kept.device_mode.is_none()));
    assert_eq!(accounts.current().map(|kept| kept.id.as_str()), Some("old"));
    let _ = std::fs::remove_dir_all(&dir);
}
