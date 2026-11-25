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
            .map(|entry| {
                (
                    entry.path,
                    entry.line,
                    entry.from,
                    entry.to_id,
                    entry.other.map(|other| other.id),
                )
            })
            .collect::<Vec<_>>()
    };
    let mut compared = 0;
    for artifact in &index.artifacts {
        for direction in [Direction::In, Direction::Out] {
            let expected = key(index.refs(&artifact.id, direction));
            compared += expected.len();
            assert_eq!(
                key(table.refs(&artifact.id, direction)),
                expected,
                "{} {direction:?}",
                artifact.id
            );
        }
        assert_eq!(
            table
                .children(&artifact.id)
                .iter()
                .map(|row| &row.id)
                .collect::<Vec<_>>(),
            index
                .children(&artifact.id)
                .iter()
                .map(|row| &row.id)
                .collect::<Vec<_>>()
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

/// A component `rows` in a scratch directory on an in-memory database, for the index's rows: its factory, the
/// component's directory, and a context entered with authorization off.
struct RowsBed {
    ctx: moqui_core::context::ExecutionContext,
    dir: PathBuf,
}

impl Drop for RowsBed {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl RowsBed {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("moqui-index-rows-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("framework")).unwrap();
        std::fs::create_dir_all(dir.join("rows")).unwrap();
        let component = moqui_core::context::ComponentInfo {
            name: "rows".to_string(),
            version: String::new(),
            location: dir.join("rows"),
            depends_on: Vec::new(),
            purposes: Vec::new(),
        };
        let ctx = ExecutionContextFactory::test_context_with_components(
            "",
            moqui_core::service::ServicePools::idle(),
            dir.join("framework"),
            vec![component],
        );
        ctx.factory().set_entity(moqui_core::testing::open());
        Self { ctx, dir }
    }

    fn factory(&self) -> &Arc<ExecutionContextFactory> {
        self.ctx.factory()
    }

    fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.dir.join("rows").join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    fn files(&self) -> Vec<Located> {
        directory_files("rows", &self.dir.join("rows"))
    }

    /// The ids of the rows of `entity` whose `field` is `value`, the ids from `id_field`, sorted.
    fn ids(&self, entity: &str, field: &str, value: &str, id_field: &str) -> Vec<String> {
        let facade = self.factory().entity().unwrap();
        let _lifted = fence::lift();
        let mut ids: Vec<String> = facade
            .find(entity)
            .unwrap()
            .disable_authz()
            .eq(field, Value::string(value))
            .list()
            .unwrap()
            .values()
            .iter()
            .map(|row| text(row, id_field))
            .collect();
        ids.sort();
        ids
    }

    fn has_artifact(&self, id: &str) -> bool {
        !self.ids(ARTIFACT, "artifactId", id, "artifactId").is_empty()
    }
}

const ROWS_ENTITIES: &str = r#"<entities><entity entity-name="Thing" package="rows">
    <field name="thingId" type="id" is-pk="true"/><field name="name" type="text-medium"/>
</entity></entities>"#;

const ROWS_SERVICES: &str = r#"<services><service verb="save" noun="Thing">
    <in-parameters><auto-parameters entity-name="rows.Thing"/></in-parameters>
</service></services>"#;

/// The index's fault hook (plans/design-dev-aide.md 19): an expansion of a file not read now whose write fails is
/// written again at the next update, though nothing it reads changed again.
#[test]
fn an_expansions_write_that_fails_is_written_again_at_the_next_update() {
    let bed = RowsBed::new("retry");
    let entities = bed.write("entity/ThingEntities.xml", ROWS_ENTITIES);
    bed.write("service/rows/ThingServices.xml", ROWS_SERVICES);
    let other = bed.write("service/rows/OtherServices.xml", "<services/>");
    let _enter = bed.ctx.enter();
    let _authz = bed.ctx.disable_authz();
    build_files(bed.factory(), bed.files(), false).unwrap();
    let service = "service:rows.ThingServices.save#Thing";
    let color = "parameter:service:rows.ThingServices.save#Thing@in.color";
    assert!(bed.has_artifact("parameter:service:rows.ThingServices.save#Thing@in.name"));

    // a field added to the entity: the service in its own file is expanded again, and its write fails
    lock(&FAIL_EXPANSION_WRITE).push(service.to_string());
    std::fs::write(
        &entities,
        ROWS_ENTITIES.replace(
            r#"<field name="name""#,
            r#"<field name="color" type="id"/><field name="name""#,
        ),
    )
    .unwrap();
    update_files(bed.factory(), std::slice::from_ref(&entities)).unwrap();
    assert!(lock(&FAIL_EXPANSION_WRITE).is_empty(), "the failure point was reached");
    assert!(!bed.has_artifact(color), "the failed write left no row");

    // an update of another file: the expansion that failed is written then
    std::fs::write(&other, r#"<services><service verb="run" noun="Other"/></services>"#).unwrap();
    update_files(bed.factory(), std::slice::from_ref(&other)).unwrap();
    assert!(bed.has_artifact(color), "the expansion was written again");
    assert!(bed.has_artifact("service:rows.OtherServices.run#Other"));
}

/// An artifact group's members are references from the group to what each names, one whose name is a pattern to each
/// artifact it matches whole; and an artifact an update adds that a pattern matches is a member at once.
#[test]
fn an_artifact_groups_patterns_resolve_to_the_artifacts_they_match() {
    let bed = RowsBed::new("groups");
    bed.write("entity/ThingEntities.xml", ROWS_ENTITIES);
    bed.write("service/rows/ThingServices.xml", ROWS_SERVICES);
    let _enter = bed.ctx.enter();
    let _authz = bed.ctx.disable_authz();
    let facade = bed.factory().entity().unwrap();
    {
        let _lifted = fence::lift();
        for (name, kind, pattern) in [
            ("rows\\.ThingServices\\..*", "AT_SERVICE", "Y"),
            ("rows.Thing", "AT_ENTITY", "N"),
            ("rows.Missing", "AT_ENTITY", "N"),
        ] {
            let mut member = facade.make_value(rows::ARTIFACT_GROUP_MEMBER).unwrap();
            member.set("artifactGroupId", Value::string("THINGS"));
            member.set("artifactName", Value::string(name));
            member.set("artifactTypeEnumId", Value::string(kind));
            member.set("nameIsPattern", Value::string(pattern));
            member.create(&facade).unwrap();
        }
    }
    build_files(bed.factory(), bed.files(), true).unwrap();
    let group = "artifact-group:THINGS";
    assert!(bed.has_artifact(group));
    let members = || bed.ids(REF, "fromArtifactId", group, "toArtifactId");
    // the pattern's service, the entity by its name; the missing entity unresolved
    assert_eq!(
        members(),
        ["", "entity:rows.Thing", "service:rows.ThingServices.save#Thing"]
    );

    // a service the pattern matches, added by an update: a member of the group at once
    let added = bed.write(
        "service/rows/ThingServices.xml",
        &ROWS_SERVICES.replace("</services>", r#"<service verb="ship" noun="Thing"/></services>"#),
    );
    update_files(bed.factory(), std::slice::from_ref(&added)).unwrap();
    assert_eq!(
        members(),
        [
            "",
            "entity:rows.Thing",
            "service:rows.ThingServices.save#Thing",
            "service:rows.ThingServices.ship#Thing"
        ]
    );
}
