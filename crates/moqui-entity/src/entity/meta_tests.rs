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

use std::sync::{Arc, Mutex};

use crate::testing::{self, LogSubscriber};

fn warnings(facade: &crate::entity::EntityFacade) -> Vec<String> {
    let lines = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::with_default(LogSubscriber(Arc::clone(&lines)), || super::warn_cross_database(facade));
    let lines = lines.lock().expect("lines").clone();
    lines
}

/// The content group on a database of its own: none of the framework's foreign keys or view-entities crosses to it.
#[test]
fn nothing_in_the_framework_model_crosses_to_the_content_database() {
    let facade = testing::open_groups("content");
    assert!(!facade.same_database(Some("content"), None));
    assert!(!facade.same_database(Some("content"), Some("transactional")));
    let crossing = warnings(&facade);
    assert!(crossing.is_empty(), "{crossing:#?}");
}

/// A group with no datasource of its own is on the default group's database.
#[test]
fn a_group_without_a_datasource_is_on_the_default_database() {
    let facade = testing::open();
    assert!(facade.same_database(Some("content"), None));
    assert!(facade.same_database(Some("content"), Some("transactional")));
    assert!(warnings(&facade).is_empty());
}

struct DirGuard(std::path::PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A foreign key relationship to an entity on another database, and a view-entity joining across, are warned of.
#[test]
fn a_foreign_key_or_a_view_to_another_database_is_warned_of() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("moqui-meta-{}-{nanos}", std::process::id()));
    let _guard = DirGuard(dir.clone());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("CrossEntities.xml"),
        r#"<entities>
            <entity entity-name="CrossNote" package="moqui.test" group="content">
                <field name="noteId" type="id" is-pk="true"/>
                <field name="enumId" type="id"/>
                <relationship type="one" related="moqui.basic.Enumeration"/>
            </entity>
            <view-entity entity-name="CrossNoteAndEnum" package="moqui.test">
                <member-entity entity-alias="NTE" entity-name="moqui.test.CrossNote"/>
                <member-entity entity-alias="ENM" entity-name="moqui.basic.Enumeration" join-from-alias="NTE">
                    <key-map field-name="enumId"/></member-entity>
                <alias-all entity-alias="NTE"/>
            </view-entity>
        </entities>"#,
    )
    .unwrap();
    let facade = testing::open_groups_with_components("content", std::slice::from_ref(&dir));
    let crossing = warnings(&facade);
    assert_eq!(crossing.len(), 2, "{crossing:#?}");
    assert!(crossing
        .iter()
        .any(|line| line.contains("moqui.test.CrossNote") && line.contains("one-nofk")));
    assert!(crossing.iter().any(|line| line.contains("moqui.test.CrossNoteAndEnum")));
}
