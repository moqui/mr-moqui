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

use std::sync::Arc;

use moqui_client::account::Account;

use super::*;

fn account(id: &str) -> Account {
    Account {
        id: id.to_string(),
        server: format!("https://{id}.example.com"),
        username: "joe".to_string(),
        user_id: String::new(),
        display: String::new(),
        server_name: String::new(),
        device_mode: None,
    }
}

#[test]
fn one_client_per_process_until_it_stops_and_the_current_account_is_asked_first() {
    let dir = std::env::temp_dir().join(format!("moqui-mobile-native-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let core = core_in(&dir).unwrap();
    assert!(
        Arc::ptr_eq(&core, &core_in(&dir).unwrap()),
        "a push and the app's setup share one client"
    );
    assert!(!has_account(&dir));
    {
        let mut accounts = moqui_lib::sync::lock(core.accounts());
        for id in ["a", "b", "c"] {
            accounts.put(account(id));
        }
        accounts.set_current("b");
    }
    // c has no key: it cannot be asked
    core.keys().set("a", "key-a").unwrap();
    core.keys().set("b", "key-b").unwrap();
    let order: Vec<String> = asked(&core).into_iter().map(|asked| asked.account.id).collect();
    assert_eq!(order, vec!["b".to_string(), "a".to_string()]);
    assert_eq!(asked(&core)[0].comms_root, "/qapps/BasicComms");
    assert!(has_account(&dir));
    // a bad id is never sent
    assert_eq!(decline_call(&dir, "1/../x"), "not a call id");
    // a background send with nothing queued: nothing waits for a time
    assert_eq!(flush_background(&dir, 1000), -1);
    core.shutdown();
    let again = core_in(&dir).unwrap();
    assert!(!Arc::ptr_eq(&core, &again), "a stopped client is made again");
    again.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
