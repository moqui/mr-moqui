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

//! The change set card from a patch (test/vectors/change-card-vectors.json): what [`change_card`] makes of a root's
//! files and the changes proposed for them, which test/client/pages/DevAideCard.page.js renders in the page.

use std::path::PathBuf;

use serde_json::Value as Json;

use super::{change_card, FileChange, Prepared};
use crate::agent::NamedFile;
use crate::roots::{Root, RootComponent, RootKind};

fn root(components: &Json) -> Root {
    Root {
        id: "work".into(),
        label: "work".into(),
        path: PathBuf::from("/r/work"),
        kind: RootKind::Git,
        components: serde_json::from_value::<Vec<Json>>(components.clone())
            .unwrap()
            .iter()
            .map(|component| RootComponent {
                name: component["name"].as_str().unwrap().to_string(),
                path: component["path"].as_str().unwrap().to_string(),
                site: component["site"].as_bool().unwrap(),
            })
            .collect(),
        upstream: false,
        read_only: false,
        publish_branch: None,
        publish_branch_required: false,
        hosted: None,
        worktree_of: None,
        conversation_id: None,
        worktree_user_id: None,
    }
}

fn kind(name: &str) -> moqui_devkit::ArtifactKind {
    use moqui_devkit::ArtifactKind as Kind;
    match name {
        "entity" => Kind::Entity,
        "service" => Kind::Service,
        "screen" => Kind::Screen,
        "screen-extend" => Kind::ScreenExtend,
        other => panic!("no kind {other} in the vectors"),
    }
}

/// Each vector's card is what the server makes of its files: the change as artifacts (an entity's new field, a form's
/// new field by the screen it extends, a new service, a line count where no artifact changed), the files with their
/// counts and side, the new titles a business user may word, and the screens to preview. With
/// MOQUI_REGENERATE_VECTORS set the cards are written instead, for review.
#[test]
fn the_change_card_vectors_hold() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test/vectors/change-card-vectors.json");
    let mut vectors: Json = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let regenerate = std::env::var_os("MOQUI_REGENERATE_VECTORS").is_some();
    for vector in vectors["vectors"].as_array_mut().unwrap() {
        let name = vector["name"].as_str().unwrap_or_default().to_string();
        let root = root(&vector["components"]);
        let files = vector["files"].as_array().unwrap();
        let prepared: Vec<Prepared<'_>> = files
            .iter()
            .map(|file| {
                let change: FileChange = serde_json::from_value(file.clone()).unwrap();
                let current = file["current"].as_str().map(str::to_string);
                let new_text = change
                    .apply(current.as_deref())
                    .unwrap_or_else(|why| panic!("{name}: {}: {why}", change.file));
                let relative = file["relative"].as_str().unwrap().to_string();
                Prepared {
                    named: NamedFile {
                        root: &root,
                        path: root.path.join(&relative),
                        relative,
                        shown: change.file.clone(),
                    },
                    current,
                    stale: None,
                    new_text,
                    kind: kind(file["kind"].as_str().unwrap()),
                    location: change.file.clone(),
                }
            })
            .collect();
        let card = change_card(&prepared);
        if regenerate {
            vector["card"] = card;
        } else {
            assert_eq!(vector["card"], card, "{name}: the card");
        }
    }
    if regenerate {
        std::fs::write(&path, format!("{}\n", serde_json::to_string_pretty(&vectors).unwrap())).unwrap();
    }
}
