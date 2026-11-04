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

use std::path::PathBuf;

use crate::store::{ChatEvent, EventPage};

use super::Stores;

fn yes() -> bool {
    true
}

fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-desktop-stores-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn forget_closes_and_deletes_an_accounts_store_and_its_old_cache() {
    let dir = dir("forget");
    std::fs::create_dir_all(dir.join("cache/a")).unwrap();
    std::fs::write(dir.join("cache/a/shellData.json"), b"{\"apps\":[1]}").unwrap();
    let stores = Stores::new(&dir);
    // the MD3 file moved in when the store opened
    let body = stores.with("a", 1 << 20, yes, |store| {
        store.answer("/shellData", "").map(|answer| answer.body)
    });
    assert_eq!(body.flatten().as_deref(), Some(&b"{\"apps\":[1]}"[..]));
    assert!(!dir.join("cache/a").exists());
    stores
        .with("a", 1 << 20, yes, |store| {
            store.put_answer("/x", "", "application/json", b"{}")
        })
        .unwrap()
        .unwrap();
    stores.with("b", 1 << 20, yes, |_| ()).unwrap();
    stores.forget("a");
    assert!(!dir.join("data/a.db").exists());
    assert!(dir.join("data/b.db").exists());
    // opened again: empty
    assert_eq!(
        stores.with("a", 1 << 20, yes, |store| store.answer("/x", "").is_none()),
        Some(true)
    );
    // forgotten, and no longer kept: a late write opens nothing
    stores.forget("a");
    assert_eq!(stores.with("a", 1 << 20, || false, |_| ()), None);
    assert!(!dir.join("data/a.db").exists());
    // an account never opened here: its files go all the same
    stores.forget("b");
    let other = Stores::new(&dir);
    other.forget("a");
    assert!(!dir.join("data/a.db").exists() && !dir.join("data/b.db").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_store_opened_again_has_its_heads_stale() {
    let dir = dir("stale");
    let page = EventPage {
        limit: 10,
        ..EventPage::default()
    };
    let event = ChatEvent {
        chat_room_id: "R1".to_string(),
        event_seq: 1,
        event: "{\"eventSeq\":1}".to_string(),
        thread_seq: None,
    };
    {
        let stores = Stores::new(&dir);
        stores
            .with("a", 1 << 20, yes, |store| {
                store.put_chat_page("R1", &page, std::slice::from_ref(&event), false, false)
            })
            .unwrap()
            .unwrap();
        assert_eq!(
            stores.with("a", 1 << 20, yes, |store| store.chat_events("R1", &page).gap_after),
            Some(false)
        );
    }
    let stores = Stores::new(&dir);
    let slice = stores
        .with("a", 1 << 20, yes, |store| store.chat_events("R1", &page))
        .unwrap();
    assert_eq!(slice.events.len(), 1);
    assert!(slice.gap_after);
    let _ = std::fs::remove_dir_all(&dir);
}
