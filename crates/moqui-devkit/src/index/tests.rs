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

fn artifact(id: &str, kind: &str, name: &str) -> ArtifactRow {
    ArtifactRow {
        id: id.into(),
        kind: kind.into(),
        name: name.into(),
        short_name: extract::short_of(kind, name),
        component: "c".into(),
        path: String::new(),
        line: 1,
        column: 1,
        summary: String::new(),
        parent: None,
        origin: String::new(),
        marks: String::new(),
    }
}

#[test]
fn ranking_puts_exact_and_short_matches_first() {
    let candidates = vec![
        artifact("entity:mantle.order.OrderItem", "entity", "mantle.order.OrderItem"),
        artifact("entity:mantle.order.OrderHeader", "entity", "mantle.order.OrderHeader"),
        artifact(
            "field:mantle.order.OrderHeader.orderId",
            "field",
            "mantle.order.OrderHeader.orderId",
        ),
        artifact(
            "service:mantle.order.OrderServices.place#Order",
            "service",
            "mantle.order.OrderServices.place#Order",
        ),
    ];
    let found = rank(candidates.clone(), "OrdHdr", 10);
    assert_eq!(found[0].id, "entity:mantle.order.OrderHeader");
    assert!(found
        .iter()
        .all(|artifact| artifact.id != "entity:mantle.order.OrderItem"));
    let found = rank(candidates.clone(), "place#Order", 10);
    assert_eq!(found[0].id, "service:mantle.order.OrderServices.place#Order");
    let found = rank(candidates, "entity:mantle.order.OrderItem", 1);
    assert_eq!(found[0].id, "entity:mantle.order.OrderItem");
}

/// The stamp is of the files' locations and hashes, in any order: the same files give the same hash, and a changed,
/// added or removed file another.
#[test]
fn the_stamp_is_the_files_and_their_hashes_in_any_order() {
    let files = |list: &[(&str, &str)]| {
        list.iter()
            .map(|(location, hash)| ((*location).to_string(), (*hash).to_string()))
            .collect::<Vec<_>>()
    };
    let one = IndexStamp::of(
        files(&[("component://a/x.xml", "h1"), ("component://b/y.xml", "h2")]),
        5,
    );
    let reordered = IndexStamp::of(
        files(&[("component://b/y.xml", "h2"), ("component://a/x.xml", "h1")]),
        5,
    );
    assert_eq!(one, reordered);
    assert_eq!((one.files, one.built_at, one.hash.len()), (2, 5, 24));
    let changed = IndexStamp::of(
        files(&[("component://a/x.xml", "h3"), ("component://b/y.xml", "h2")]),
        5,
    );
    let fewer = IndexStamp::of(files(&[("component://a/x.xml", "h1")]), 5);
    assert_ne!(one.hash, changed.hash);
    assert_ne!(one.hash, fewer.hash);
    assert_ne!(IndexStamp::of(Vec::new(), 0).hash, one.hash);
}

/// A file that does not parse is in a memory index's stamp, as a server's rows keep it (stale): the same files give the
/// same stamp on both, and fixing the file changes it.
#[test]
fn a_file_that_does_not_parse_is_in_the_stamp() {
    let dir = std::env::temp_dir().join(format!("moqui-stamp-problem-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let located = |name: &str, text: &str| {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        Located {
            path,
            component: "c".to_string(),
            location: format!("component://c/entity/{name}"),
            kind: ArtifactKind::Entity,
        }
    };
    let good = located(
        "Good.xml",
        r#"<entities><entity entity-name="Good" package="p"><field name="goodId" type="id" is-pk="true"/></entity></entities>"#,
    );
    let broken = located("Broken.xml", "<entities><entity");
    let ctx = moqui_core::context::ExecutionContextFactory::test_context();
    let mut index = MemoryIndex::of_files(ctx.factory(), &[good.clone(), broken.clone()]);
    assert_eq!(index.problems.len(), 1, "{:?}", index.problems);
    let stamp = index.stamp();
    let expected = IndexStamp::of(
        vec![
            (
                good.location.clone(),
                hash_of(&std::fs::read_to_string(&good.path).unwrap()),
            ),
            (broken.location.clone(), hash_of("<entities><entity")),
        ],
        stamp.built_at,
    );
    assert_eq!(stamp, expected);

    let fixed = located("Broken.xml", "<entities/>");
    index.replace_files(ctx.factory(), &[fixed.path.display().to_string()], &[fixed]);
    assert!(index.problems.is_empty(), "{:?}", index.problems);
    assert_eq!(index.stamp().files, 2);
    assert_ne!(index.stamp().hash, stamp.hash);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The reference table answers every artifact's lists as the memory index does, in the same order: over the example
/// component's files, both ways.
#[test]
fn the_ref_table_answers_as_the_memory_index() {
    let dir = std::fs::canonicalize(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/components/dev/example"
    ))
    .unwrap();
    let ctx = moqui_core::context::ExecutionContextFactory::test_context();
    let index = MemoryIndex::of_files(ctx.factory(), &directory_files("example", &dir));
    assert!(index.artifacts.len() > 100, "{}", index.artifacts.len());
    let table = index.ref_table();
    let key = |entries: Vec<RefEntry>| {
        entries
            .into_iter()
            .map(|entry| (entry.path, entry.line, entry.from, entry.to_id, entry.other.map(|other| other.id)))
            .collect::<Vec<_>>()
    };
    let mut compared = 0;
    for artifact in &index.artifacts {
        for direction in [Direction::In, Direction::Out] {
            let expected = key(index.refs(&artifact.id, direction));
            compared += expected.len();
            assert_eq!(key(table.refs(&artifact.id, direction)), expected, "{} {direction:?}", artifact.id);
        }
        assert_eq!(
            table.children(&artifact.id).iter().map(|row| &row.id).collect::<Vec<_>>(),
            index.children(&artifact.id).iter().map(|row| &row.id).collect::<Vec<_>>()
        );
    }
    assert!(compared > 100, "{compared}");
}

/// A project's index (a directory read on its own) walks its components' conf mounts: `/apps/Shop/Detail` goes through
/// the mount of `Shop` under `apps` in the component's `MoquiConf.xml`, as a server's index walks the merged conf's.
#[test]
fn a_project_index_walks_its_components_conf_mounts() {
    let dir = std::env::temp_dir().join(format!("moqui-project-mounts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let write = |path: &str, text: &str| {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write("webroot/screen/webroot.xml", "<screen/>");
    write("webroot/screen/webroot/apps.xml", "<screen/>");
    write(
        "shop/MoquiConf.xml",
        r#"<moqui-conf><screen-facade><screen location="component://webroot/screen/webroot/apps.xml">
            <subscreens-item name="Shop" location="component://shop/screen/Shop.xml"/>
        </screen></screen-facade></moqui-conf>"#,
    );
    write("shop/screen/Shop.xml", "<screen/>");
    write("shop/screen/Shop/Detail.xml", "<screen/>");
    write(
        "shop/screen/Links.xml",
        r#"<screen><widgets><link url="/apps/Shop/Detail" text="Detail"/></widgets></screen>"#,
    );
    let mut files = directory_files("webroot", &dir.join("webroot"));
    files.extend(directory_files("shop", &dir.join("shop")));
    let ctx = moqui_core::context::ExecutionContextFactory::test_context();
    let index = MemoryIndex::of_files(ctx.factory(), &files);
    assert!(
        index
            .describe("subscreen:component://webroot/screen/webroot/apps.xml#Shop@conf")
            .is_some(),
        "{:?}",
        index.artifacts.iter().map(|artifact| &artifact.id).collect::<Vec<_>>()
    );
    let link = index
        .refs
        .iter()
        .map(|(_, row)| row)
        .find(|row| row.to_name == "/apps/Shop/Detail")
        .expect("the link");
    assert_eq!(
        link.to_id.as_deref(),
        Some("screen:component://shop/screen/Shop/Detail.xml")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
