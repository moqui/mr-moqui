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
        // the index reads a directory's files by their canonical paths: macOS's temp dir is under a symlink
        // (/var to /private/var), so the paths the tests compare are the canonical ones too
        let dir = std::fs::canonicalize(&dir).unwrap();
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

/// The index's fault hook (plans/archive/design-dev-aide.md 19): an expansion of a file not read now whose write fails is
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
    lock(&FAIL_EXPANSION_WRITE).push((factory_key(bed.factory()), service.to_string()));
    std::fs::write(
        &entities,
        ROWS_ENTITIES.replace(
            r#"<field name="name""#,
            r#"<field name="color" type="id"/><field name="name""#,
        ),
    )
    .unwrap();
    update_files(bed.factory(), std::slice::from_ref(&entities)).unwrap();
    let key = (factory_key(bed.factory()), service.to_string());
    assert!(
        !lock(&FAIL_EXPANSION_WRITE).contains(&key),
        "the failure point was reached"
    );
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
            // matched whole as Java matches it: no group of its own to close, so it does not compile and matches none
            ("x)|(.*", "AT_SERVICE", "Y"),
        ] {
            let mut member = facade.make_value(rows::ARTIFACT_GROUP_MEMBER).unwrap();
            member.set("artifactGroupId", Value::string("THINGS"));
            member.set("artifactName", Value::string(name));
            member.set("artifactTypeEnumId", Value::string(kind));
            member.set("nameIsPattern", Value::string(pattern));
            member.set(
                "inheritAuthz",
                Value::string(if name == "rows.Thing" { "Y" } else { "N" }),
            );
            member.create(&facade).unwrap();
        }
    }
    build_files(bed.factory(), bed.files(), true).unwrap();
    let group = "artifact-group:THINGS";
    assert!(bed.has_artifact(group));
    let members = || bed.ids(REF, "fromArtifactId", group, "toArtifactId");
    // the pattern's service, the entity by its name; the missing entity and the bad pattern unresolved
    assert_eq!(
        members(),
        ["", "", "entity:rows.Thing", "service:rows.ThingServices.save#Thing"]
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
            "",
            "entity:rows.Thing",
            "service:rows.ThingServices.save#Thing",
            "service:rows.ThingServices.ship#Thing"
        ]
    );

    // and gone again: no member left naming it
    bed.write("service/rows/ThingServices.xml", ROWS_SERVICES);
    update_files(bed.factory(), std::slice::from_ref(&added)).unwrap();
    assert_eq!(
        members(),
        ["", "", "entity:rows.Thing", "service:rows.ThingServices.save#Thing"]
    );
    // a member that inherits its authorization says so
    let inherited = bed.ids(REF, "operation", "authz-inherit", "toArtifactId");
    assert_eq!(inherited, ["entity:rows.Thing"]);
}

/// An entity two files define, written in one batch: one row, the later file's, as when each file is written on its
/// own; an update of the earlier file then takes it.
#[test]
fn an_id_two_files_of_a_batch_define_is_the_later_files() {
    let bed = RowsBed::new("shared");
    let first = bed.write("entity/AEntities.xml", ROWS_ENTITIES);
    let second = bed.write("entity/BEntities.xml", ROWS_ENTITIES);
    let _enter = bed.ctx.enter();
    let _authz = bed.ctx.disable_authz();
    let files = bed.files();
    let later = files
        .iter()
        .rposition(|file| file.path == first || file.path == second)
        .map(|at| files[at].path.display().to_string())
        .unwrap();
    let earlier = if later == first.display().to_string() {
        &second
    } else {
        &first
    };
    build_files(bed.factory(), files, false).unwrap();
    let entity = "entity:rows.Thing";
    let paths = || bed.ids(ARTIFACT, "artifactId", entity, "filePath");
    assert_eq!(paths(), [later]);
    assert_eq!(
        bed.ids(ARTIFACT, "artifactId", "field:rows.Thing.name", "filePath")
            .len(),
        1
    );
    update_files(bed.factory(), std::slice::from_ref(earlier)).unwrap();
    assert_eq!(paths(), [earlier.display().to_string()]);
}

/// A build that finds the rows as the last one read them reads no ids again; one after a write, or a file row another
/// server changed, does.
#[test]
fn a_build_reads_the_ids_again_only_after_the_rows_changed() {
    let bed = RowsBed::new("loads");
    let entities = bed.write("entity/ThingEntities.xml", ROWS_ENTITIES);
    let _enter = bed.ctx.enter();
    let _authz = bed.ctx.disable_authz();
    let loads = || state(bed.factory()).loads.load(Ordering::SeqCst);
    build_files(bed.factory(), bed.files(), false).unwrap();
    // the first build wrote rows: the next reads the ids, and the one after it not
    build_files(bed.factory(), bed.files(), false).unwrap();
    let after = loads();
    build_files(bed.factory(), bed.files(), false).unwrap();
    assert_eq!(loads(), after, "nothing changed");
    assert!(bed.has_artifact("entity:rows.Thing"));

    // a file row changed outside this build: read again
    {
        let facade = bed.factory().entity().unwrap();
        let _lifted = fence::lift();
        let mut row = facade
            .find(FILE)
            .unwrap()
            .disable_authz()
            .eq("filePath", Value::string(entities.display().to_string()))
            .one()
            .unwrap()
            .unwrap();
        row.set("fileHash", Value::string("another server's"));
        row.update(&facade).unwrap();
    }
    let report = build_files(bed.factory(), bed.files(), false).unwrap();
    assert_eq!(loads(), after + 1);
    assert_eq!(report.read, 1, "the file read again");
    build_files(bed.factory(), bed.files(), false).unwrap();
    assert_eq!(loads(), after + 2, "the build before wrote its rows");
}

/// A batch with a file whose rows fail (a duplicate key) is written again file by file: the other files' rows are
/// written, the failing file is logged alone and has no rows, and the next build reads it again.
#[test]
fn a_batch_that_fails_is_written_again_file_by_file() {
    let bed = RowsBed::new("retry-files");
    let entity = |name: &str| {
        format!(
            r#"<entities><entity entity-name="{name}" package="rows"><field name="id" type="id" is-pk="true"/></entity></entities>"#
        )
    };
    bed.write("entity/AEntities.xml", &entity("Apple"));
    let bad = bed.write("entity/BEntities.xml", &entity("Banana"));
    bed.write("entity/CEntities.xml", &entity("Cherry"));
    let bad_path = bad.display().to_string();
    let _enter = bed.ctx.enter();
    let _authz = bed.ctx.disable_authz();
    lock(&FAIL_FILE_WRITE).push(bad_path.clone());
    let report = build_files(bed.factory(), bed.files(), false).unwrap();
    lock(&FAIL_FILE_WRITE).retain(|path| *path != bad_path);
    let logged: Vec<String> = lock(&UNWRITTEN)
        .iter()
        .filter(|path| path.starts_with(&bed.dir.display().to_string()))
        .cloned()
        .collect();
    assert_eq!(
        logged,
        std::slice::from_ref(&bad_path),
        "the failing file is logged alone"
    );
    assert_eq!(report.read, 2, "{report:?}");
    assert!(bed.has_artifact("entity:rows.Apple"));
    assert!(bed.has_artifact("entity:rows.Cherry"));
    assert!(
        !bed.has_artifact("entity:rows.Banana"),
        "its batch's rows were rolled back"
    );
    assert!(bed.ids(FILE, "filePath", &bad_path, "filePath").is_empty());

    let report = build_files(bed.factory(), bed.files(), false).unwrap();
    assert_eq!((report.read, report.unchanged), (1, 2), "{report:?}");
    assert!(bed.has_artifact("entity:rows.Banana"));
}

/// Batches of `size` files written on three connections at once (the test pool has no spare ones), for a test of
/// batches written at once; the size and writers as they were when dropped.
struct SmallBatches(usize, Option<usize>);

impl SmallBatches {
    fn of(size: usize) -> Self {
        Self(
            TEST_BATCH_FILES.with(|batch| batch.replace(size)),
            TEST_WRITERS.with(|writers| writers.replace(Some(3))),
        )
    }
}

impl Drop for SmallBatches {
    fn drop(&mut self) {
        TEST_BATCH_FILES.with(|batch| batch.set(self.0));
        TEST_WRITERS.with(|writers| writers.set(self.1));
    }
}

/// The paths logged as not written under `bed`'s directory.
fn unwritten(bed: &RowsBed) -> Vec<String> {
    let dir = bed.dir.display().to_string();
    lock(&UNWRITTEN)
        .iter()
        .filter(|path| path.starts_with(&dir))
        .cloned()
        .collect()
}

/// Entity files, `count` of them, each with an entity of its own.
fn entity_files(bed: &RowsBed, count: usize) {
    for at in 0..count {
        bed.write(
            &format!("entity/F{at:02}Entities.xml"),
            &format!(
                r#"<entities><entity entity-name="F{at:02}" package="rows"><field name="id" type="id" is-pk="true"/></entity></entities>"#
            ),
        );
    }
}

/// An entity two files define, in batches written at once on several connections: one row, the later file's, whichever
/// batch is written first; an update of the earlier file then takes it.
#[test]
fn an_id_two_files_of_batches_written_at_once_define_is_the_later_files() {
    let bed = RowsBed::new("shared-parallel");
    entity_files(&bed, 12);
    let first = bed.write("entity/AEntities.xml", ROWS_ENTITIES);
    let second = bed.write("entity/ZEntities.xml", ROWS_ENTITIES);
    let _enter = bed.ctx.enter();
    let _authz = bed.ctx.disable_authz();
    let _small = SmallBatches::of(1);
    let files = bed.files();
    let later = files
        .iter()
        .rposition(|file| file.path == first || file.path == second)
        .map(|at| files[at].path.display().to_string())
        .unwrap();
    let earlier = if later == first.display().to_string() {
        &second
    } else {
        &first
    };
    let report = build_files(bed.factory(), files, false).unwrap();
    assert_eq!(report.read, 14, "{report:?}");
    for at in 0..12 {
        assert!(bed.has_artifact(&format!("entity:rows.F{at:02}")), "F{at:02}");
    }
    let paths = || bed.ids(ARTIFACT, "artifactId", "entity:rows.Thing", "filePath");
    assert_eq!(paths(), [later]);
    assert_eq!(
        bed.ids(ARTIFACT, "artifactId", "field:rows.Thing.name", "filePath")
            .len(),
        1
    );
    update_files(bed.factory(), std::slice::from_ref(earlier)).unwrap();
    assert_eq!(paths(), [earlier.display().to_string()]);
}

/// A failing file among batches written at once: its batch is written again file by file, the file is logged alone and
/// has no rows, and every other batch's files are written.
#[test]
fn a_file_that_fails_among_batches_written_at_once_fails_alone() {
    let bed = RowsBed::new("retry-parallel");
    entity_files(&bed, 9);
    let bad_path = bed.dir.join("rows/entity/F04Entities.xml").display().to_string();
    let _enter = bed.ctx.enter();
    let _authz = bed.ctx.disable_authz();
    let _small = SmallBatches::of(2);
    lock(&FAIL_FILE_WRITE).push(bad_path.clone());
    let report = build_files(bed.factory(), bed.files(), false).unwrap();
    lock(&FAIL_FILE_WRITE).retain(|path| *path != bad_path);
    let logged: Vec<String> = lock(&UNWRITTEN)
        .iter()
        .filter(|path| path.starts_with(&bed.dir.display().to_string()))
        .cloned()
        .collect();
    assert_eq!(
        logged,
        std::slice::from_ref(&bad_path),
        "the failing file is logged alone"
    );
    assert_eq!(report.read, 8, "{report:?}");
    for at in (0..9).filter(|at| *at != 4) {
        assert!(bed.has_artifact(&format!("entity:rows.F{at:02}")), "F{at:02}");
    }
    assert!(!bed.has_artifact("entity:rows.F04"));
    let report = build_files(bed.factory(), bed.files(), false).unwrap();
    assert_eq!((report.read, report.unchanged), (1, 8), "{report:?}");
    assert!(bed.has_artifact("entity:rows.F04"));
}

/// An entity two files define whose later file (its row's owner) fails: the earlier file, which passed the id over,
/// writes its row once the batches are in, so the table has the definition the resolver knows.
#[test]
fn an_id_whose_owner_fails_is_written_from_the_other_file() {
    let bed = RowsBed::new("shared-owner-fails");
    entity_files(&bed, 6);
    let first = bed.write("entity/AEntities.xml", ROWS_ENTITIES);
    let second = bed.write("entity/ZEntities.xml", ROWS_ENTITIES);
    let _enter = bed.ctx.enter();
    let _authz = bed.ctx.disable_authz();
    let _small = SmallBatches::of(1);
    let files = bed.files();
    let later = files
        .iter()
        .rposition(|file| file.path == first || file.path == second)
        .map(|at| files[at].path.display().to_string())
        .unwrap();
    let earlier = [first, second]
        .iter()
        .map(|path| path.display().to_string())
        .find(|path| *path != later)
        .unwrap();
    lock(&FAIL_FILE_WRITE).push(later.clone());
    let report = build_files(bed.factory(), files, false).unwrap();
    lock(&FAIL_FILE_WRITE).retain(|path| *path != later);
    assert_eq!(
        unwritten(&bed),
        std::slice::from_ref(&later),
        "the owner is logged alone"
    );
    assert_eq!(report.read, 7, "{report:?}");
    for id in ["entity:rows.Thing", "field:rows.Thing.thingId", "field:rows.Thing.name"] {
        assert_eq!(
            bed.ids(ARTIFACT, "artifactId", id, "filePath"),
            std::slice::from_ref(&earlier),
            "{id}"
        );
    }
}

/// An entity moved from a file to another, both updated in batches written at once, the new file's batch first: its
/// row is the new file's, and no file fails on the old row.
#[test]
fn an_id_moved_to_another_file_is_written_after_its_old_file() {
    let bed = RowsBed::new("moved-parallel");
    entity_files(&bed, 6);
    let entity = |name: &str| {
        format!(
            r#"<entities><entity entity-name="{name}" package="rows"><field name="id" type="id" is-pk="true"/></entity></entities>"#
        )
    };
    let a = bed.write("entity/AEntities.xml", &entity("Apple"));
    let z = bed.write("entity/ZEntities.xml", ROWS_ENTITIES);
    let _enter = bed.ctx.enter();
    let _authz = bed.ctx.disable_authz();
    let _small = SmallBatches::of(1);
    build_files(bed.factory(), bed.files(), false).unwrap();
    let paths = || bed.ids(ARTIFACT, "artifactId", "entity:rows.Thing", "filePath");
    assert_eq!(paths(), [z.display().to_string()]);

    bed.write("entity/AEntities.xml", ROWS_ENTITIES);
    bed.write("entity/ZEntities.xml", &entity("Zebra"));
    let report = update_files(bed.factory(), &[a.clone(), z]).unwrap();
    assert_eq!(report.read, 2, "{report:?}");
    assert!(unwritten(&bed).is_empty(), "{:?}", unwritten(&bed));
    assert_eq!(paths(), [a.display().to_string()]);
    assert!(bed.has_artifact("entity:rows.Zebra"));
    assert!(!bed.has_artifact("entity:rows.Apple"));
}
