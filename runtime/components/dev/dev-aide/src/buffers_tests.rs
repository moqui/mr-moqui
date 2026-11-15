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

use std::time::{Duration, Instant};

use super::*;

fn file(path: &str, text: &str, base: &str) -> BufferFile {
    BufferFile {
        root_id: "site".into(),
        path: path.into(),
        location: format!("component://site/{path}"),
        text: text.into(),
        base_hash: base.into(),
        load: false,
    }
}

fn key(id: &str) -> (String, String) {
    ("john".into(), id.into())
}

#[test]
fn a_put_file_is_got_replaced_and_cleared() {
    let mut store = Store::default();
    let now = Instant::now();
    store
        .put(key("a"), file("entity/A.xml", "one", "h1"), true, now)
        .unwrap();
    store
        .put(key("a"), file("data/Seed.xml", "rows", ""), true, now)
        .unwrap();
    // the same root and path replaces the text and keeps the base hash given
    store
        .put(key("a"), file("entity/A.xml", "two", "h1"), true, now)
        .unwrap();
    let files = store.files(&key("a"), now).unwrap().to_vec();
    assert_eq!(files.len(), 2);
    assert_eq!((files[0].text.as_str(), files[0].base_hash.as_str()), ("two", "h1"));
    assert_eq!(files[1].base_hash, "", "a file made on its first save has no base");
    // another user's buffer of the same id is not this one
    assert!(store.files(&("jane".into(), "a".into()), now).is_none());
    // a text back to the disk's takes the file out; the last file out drops the buffer
    store
        .put(key("a"), file("entity/A.xml", "one", "h1"), false, now)
        .unwrap();
    assert_eq!(store.files(&key("a"), now).unwrap().len(), 1);
    store.remove(&key("a"), Some(("site", "data/Seed.xml")));
    assert!(store.files(&key("a"), now).is_none());
    store.put(key("b"), file("entity/B.xml", "x", "h"), true, now).unwrap();
    store.remove(&key("b"), None);
    assert!(store.files(&key("b"), now).is_none());
    assert!(!store.was_expired(&key("b"), now), "a discard is not an expiry");
}

#[test]
fn a_seed_file_marked_to_load_stays_marked() {
    let mut store = Store::default();
    let now = Instant::now();
    let mut seed = file("data/Seed.xml", "rows", "");
    seed.load = true;
    store.put(key("a"), seed, true, now).unwrap();
    store
        .put(key("a"), file("data/Seed.xml", "more rows", ""), true, now)
        .unwrap();
    assert!(store.files(&key("a"), now).unwrap()[0].load);
}

#[test]
fn a_buffer_expires_two_hours_after_its_last_put_and_says_so() {
    let mut store = Store::default();
    let start = Instant::now();
    store
        .put(key("a"), file("entity/A.xml", "one", "h"), true, start)
        .unwrap();
    let later = start + Duration::from_secs(90 * 60);
    // a put keeps it another two hours
    store
        .put(key("a"), file("entity/A.xml", "two", "h"), true, later)
        .unwrap();
    assert_eq!(store.expires(&key("a")), Some(later + BUFFER_LIFE));
    let almost = later + BUFFER_LIFE - Duration::from_secs(1);
    assert!(store.files(&key("a"), almost).is_some());
    assert!(!store.was_expired(&key("a"), almost));
    let past = later + BUFFER_LIFE;
    assert!(store.files(&key("a"), past).is_none());
    assert!(store.was_expired(&key("a"), past));
    // a new put starts it again, not expired
    store
        .put(key("a"), file("entity/A.xml", "three", "h"), true, past)
        .unwrap();
    assert!(!store.was_expired(&key("a"), past));
}

#[test]
fn a_buffer_holds_at_most_twenty_files() {
    let mut store = Store::default();
    let now = Instant::now();
    for at in 0..MAX_FILES {
        store
            .put(key("a"), file(&format!("data/F{at}.xml"), "x", ""), true, now)
            .unwrap();
    }
    let refused = store.put(key("a"), file("data/More.xml", "x", ""), true, now);
    assert!(refused.unwrap_err().to_string().contains("at most"));
}

#[test]
fn the_kind_order_is_entities_services_rules_screens_then_data() {
    assert_eq!(kind_rank("my-site/entity/MySiteEntities.xml"), 0);
    assert_eq!(kind_rank("entity/Order.eecas.xml"), 2);
    assert_eq!(kind_rank("my-site/service/MySite.secas.xml"), 2);
    assert_eq!(kind_rank("service/my/Services.xml"), 1);
    assert_eq!(kind_rank("my-site/screen-extend/example/screen/X.xml"), 3);
    assert_eq!(kind_rank("screen/App/Home.xml"), 3);
    assert_eq!(kind_rank("my-site/data/MySiteSeedData.xml"), 4);
    let files = vec![
        file("data/Seed.xml", "", ""),
        file("screen-extend/X.xml", "", ""),
        file("service/S.secas.xml", "", ""),
        file("service/S.xml", "", ""),
        file("entity/E.xml", "", ""),
        file("data/Other.xml", "", ""),
    ];
    let order: Vec<&str> = save_order(&files).iter().map(|file| file.path.as_str()).collect();
    assert_eq!(
        order,
        [
            "entity/E.xml",
            "service/S.xml",
            "service/S.secas.xml",
            "screen-extend/X.xml",
            "data/Seed.xml",
            "data/Other.xml"
        ]
    );
}

/// A save as the pipeline does it on scratch files: refused with the disk's text when the disk is no longer at the
/// base hash, else written.
fn write_at(dir: &std::path::Path) -> impl FnMut(&BufferFile) -> Result<Written, LibError> + '_ {
    move |file| {
        let path = dir.join(&file.path);
        let disk = std::fs::read(&path).ok();
        let disk_hash = disk.as_deref().map(crate::workspace::sha256).unwrap_or_default();
        if disk.is_some() && disk_hash != file.base_hash {
            return Ok(Written::Conflict {
                disk_hash,
                disk_text: String::from_utf8_lossy(disk.as_deref().unwrap_or_default()).into_owned(),
            });
        }
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &file.text).unwrap();
        Ok(Written::Saved(Parameters::new()))
    }
}

#[test]
fn a_save_writes_in_kind_order_and_stops_at_a_conflict() {
    let dir = std::env::temp_dir().join(format!("dev-aide-buffers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("service")).unwrap();
    std::fs::write(dir.join("service/S.xml"), "<services/>").unwrap();
    let base = crate::workspace::sha256(b"<services/>");
    let files = vec![
        file("screen-extend/X.xml", "<screen-extend/>", ""),
        file("service/S.xml", "<services><service/></services>", &base),
        file("entity/E.xml", "<entities/>", ""),
    ];
    let mut order = Vec::new();
    let mut write = write_at(&dir);
    let saving = save_in_order(&files, |file| {
        order.push(file.path.clone());
        write(file)
    });
    assert_eq!(order, ["entity/E.xml", "service/S.xml", "screen-extend/X.xml"]);
    assert_eq!(saving.saved.len(), 3);
    assert!(saving.conflict.is_none() && saving.failed.is_none());
    assert_eq!(
        std::fs::read_to_string(dir.join("service/S.xml")).unwrap(),
        "<services><service/></services>"
    );

    // the service file changed on disk since: the entity file is written, the save stops at the service
    std::fs::write(dir.join("service/S.xml"), "<services><other/></services>").unwrap();
    let files = vec![
        file(
            "screen-extend/X.xml",
            "<screen-extend><a/></screen-extend>",
            &crate::workspace::sha256(b"<screen-extend/>"),
        ),
        file("service/S.xml", "<services><mine/></services>", &base),
        file(
            "entity/E.xml",
            "<entities><e/></entities>",
            &crate::workspace::sha256(b"<entities/>"),
        ),
    ];
    let saving = save_in_order(&files, write_at(&dir));
    let saved: Vec<&str> = saving.saved.iter().map(|(file, _)| file.path.as_str()).collect();
    assert_eq!(saved, ["entity/E.xml"]);
    let (conflicted, disk_hash, disk_text) = saving.conflict.unwrap();
    assert_eq!(conflicted.path, "service/S.xml");
    assert_eq!(disk_text, "<services><other/></services>");
    assert_eq!(disk_hash, crate::workspace::sha256(b"<services><other/></services>"));
    assert_eq!(
        std::fs::read_to_string(dir.join("screen-extend/X.xml")).unwrap(),
        "<screen-extend/>",
        "nothing after it"
    );

    // "Save mine over it": the base is the disk's hash now, and the rest follows
    let mut over = files[1].clone();
    over.base_hash = disk_hash;
    let files = vec![files[0].clone(), over];
    let saving = save_in_order(&files, write_at(&dir));
    assert_eq!(saving.saved.len(), 2);
    assert_eq!(
        std::fs::read_to_string(dir.join("service/S.xml")).unwrap(),
        "<services><mine/></services>"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_guided_buffer_id_is_one_per_target() {
    let id = guided_id("service:example.ExampleServices.greet#WithTool");
    assert!(id.starts_with("guided-") && id.len() == 27);
    assert_eq!(id, guided_id("service:example.ExampleServices.greet#WithTool"));
    assert_ne!(id, guided_id("entity:moqui.example.Example"));
    let mut parameters = Parameters::new();
    parameters.insert("target".into(), Value::string("entity:moqui.example.Example"));
    assert_eq!(
        buffer_id(&parameters).unwrap(),
        guided_id("entity:moqui.example.Example")
    );
    parameters.insert("bufferId".into(), Value::string("bad id!"));
    assert!(buffer_id(&parameters).is_err());
}
