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

use tauri::http::{HeaderMap, HeaderValue};

use super::{is_vault_screen, note_request};
use crate::tests::{fixture, Fixture};

fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.insert(*name, HeaderValue::from_static(value));
    }
    map
}

#[test]
fn only_the_vault_screens_are_vault_screens() {
    for path in [
        "/qapps/boss/Commons/Vault",
        "/qapps/boss/Commons/Vault/Item",
        "/qapps/boss/Commons/Settings/VaultKey",
        "/qapps/boss/Admin/Security/RecoveryKeys",
        "/qapps/system/Vaults/Lease",
        "/apps/system/Vaults",
    ] {
        assert!(is_vault_screen(path), "{path}");
    }
    for path in [
        "/qapps/boss/Commons/Calendar",
        "/qapps/boss/Commons/VaultNotes",
        "/Vault",
        "/rest/s1/vault/vaults",
        "/qapps/boss/Commons/Calendar/../Vault",
        "/qapps/boss/Commons/%56ault",
    ] {
        assert!(!is_vault_screen(path), "{path}");
    }
}

/// V5 review: a page may call the vault only while its document is a vault screen's and it has loaded no other screen
/// or frame since; a new document, or leaving the vault screens, locks its session.
#[test]
fn a_page_may_call_the_vault_only_from_a_vault_screens_document() {
    let Fixture { state, dir, .. } = fixture("vault-pages", false);
    let allowed = |label: &str| moqui_lib::sync::lock(&state.vault_pages).get(label).copied() == Some(true);
    let document = headers(&[("sec-fetch-dest", "document")]);
    let screen = headers(&[
        ("sec-fetch-dest", "empty"),
        ("accept", "text/html,application/javascript,*/*"),
    ]);
    let frame = headers(&[("sec-fetch-dest", "iframe")]);
    let data = headers(&[("sec-fetch-dest", "empty"), ("accept", "application/json")]);

    note_request(&state, "remote-1", "/qapps/boss/Commons/Calendar", &document);
    assert!(!allowed("remote-1"));
    note_request(&state, "remote-1", "/qapps/boss/Commons/Vault", &document);
    assert!(allowed("remote-1"));
    // its own data and its own sub-screens leave it allowed
    note_request(&state, "remote-1", "/rest/s1/vault/vaults", &data);
    note_request(&state, "remote-1", "/qapps/boss/Commons/Vault/Item", &screen);
    assert!(allowed("remote-1"));
    // another screen loaded into the page, or a frame of one, ends it until a new vault document
    note_request(&state, "remote-1", "/qapps/boss/Commons/Calendar", &screen);
    assert!(!allowed("remote-1"));
    note_request(&state, "remote-1", "/qapps/boss/Commons/Vault/Item", &screen);
    assert!(!allowed("remote-1"), "coming back by a screen load does not restore it");
    note_request(&state, "remote-1", "/qapps/boss/Commons/Vault", &document);
    assert!(allowed("remote-1"));
    note_request(&state, "remote-1", "/qapps/boss/Commons/Wiki", &frame);
    assert!(!allowed("remote-1"));
    // each page its own
    note_request(&state, "remote-2", "/qapps/system/Vaults/Lease", &document);
    assert!(allowed("remote-2"));
    let _ = std::fs::remove_dir_all(&dir);
}
