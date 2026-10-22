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

//! `org.moqui.impl.WikiServices` for knowledge and notes (IronStone M9): version notes on both storages with history,
//! compare and restore; the links of a page's text and its backlinks across a rename, an alias and a page taking a
//! path; a move into a git space that replays the history with each version's author and date, and one on the
//! database tree that keeps its history rows; spaces made and cloned on a git repository; a page cloned under a new
//! name with the pages under it, its attachments on the new path and its aliases; a page cloned with its history into
//! a database and a git space, with its published text, and with the pages under it that are only files; the review
//! flow with its notifications and the tray's approve and refuse; search and page lists that leave out what a user may not view; a
//! page written and read at its exact path, never over a page of the same name elsewhere; the writes and reads by the
//! access rule with its admin overrides; and a delete that takes the page's rows.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use moqui_lib::object::{to_plain_string, Value};

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;

const WIKI: &str = "org.moqui.impl.WikiServices";

/// One hour, for version dates set apart.
const HOUR: i64 = 3_600_000;
const BASE_TIME: i64 = 1_760_000_000_000;

fn setup() -> (ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    let facade = crate::testing::open();
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    ctx.factory().resource().install_git().unwrap();
    (ctx, facade)
}

fn install() {
    let framework = crate::context::framework_dir().join("service/org/moqui");
    let paths: Vec<_> = [
        "EntityServices.xml",
        "EmailServices.xml",
        "impl/EmailServices.xml",
        "impl/NotificationServices.xml",
        "impl/WikiServices.xml",
        "impl/GitServices.xml",
    ]
    .iter()
    .map(|file| framework.join(file))
    .collect();
    let failures = crate::service::lint(&paths).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
}

fn text(value: &str) -> Value {
    Value::string(value)
}

fn row(facade: &EntityFacade, entity: &str, fields: &[(&str, Value)]) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut row = facade.make_value(entity).unwrap();
    for (field, value) in fields {
        row.set(*field, value.clone());
    }
    row.create(facade).unwrap();
}

fn users(facade: &EntityFacade) {
    for id in ["alice", "bob", "carol"] {
        row(
            facade,
            "moqui.security.UserAccount",
            &[
                ("userId", text(id)),
                ("username", text(id)),
                ("userFullName", text(&format!("{id} Example"))),
                ("emailAddress", text(&format!("{id}@example.com"))),
            ],
        );
    }
}

/// The seed data of the review entity (its status items and notification topics), read from ResourceEntities.xml.
fn seed_review_data(facade: &EntityFacade) {
    let path = crate::context::framework_dir().join("entity/ResourceEntities.xml");
    let root = moqui_lib::node::MNode::parse_file(&path).unwrap();
    let entity = root
        .children_named("entity")
        .into_iter()
        .find(|node| node.attribute("entity-name").as_deref() == Some("WikiPageReview"))
        .unwrap();
    let seed = entity.first_named("seed-data").unwrap();
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    for node in seed.children() {
        // the transitions name the Default flow, seeded elsewhere
        if node.name() == "moqui.basic.StatusFlowTransition" {
            continue;
        }
        let mut value = facade.make_value(&node.name()).unwrap();
        for (name, attribute) in node.attributes() {
            value.set(&name, Value::string(attribute));
        }
        value.create(facade).unwrap();
    }
}

fn login(user_id: &str) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    crate::user::internal_login_user(user_id).unwrap();
}

fn at(ctx: &ExecutionContext, hours: i64) {
    ctx.set_effective_time(Some(BASE_TIME + hours * HOUR));
}

fn call_named(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    crate::service::sync(name, &parameters).unwrap_or_else(|err| panic!("{name}: {err}"))
}

fn errors() -> String {
    ExecutionContext::with_current(|ec| {
        let text = ec.message().errors().join("\n");
        ec.message().clear_errors();
        text
    })
    .unwrap_or_default()
}

fn ok(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let out = call_named(&format!("{WIKI}.{name}"), parameters);
    let errors = errors();
    assert!(errors.is_empty(), "{name}: {errors}");
    out
}

fn refused(name: &str, parameters: &[(&str, Value)], expected: &str) {
    let _ = call_named(&format!("{WIKI}.{name}"), parameters);
    let errors = errors();
    assert!(
        errors.contains(expected),
        "{name}: expected an error with {expected:?}, got {errors:?}"
    );
}

fn out_text(out: &BTreeMap<String, Value>, key: &str) -> String {
    out.get(key).map(to_plain_string).unwrap_or_default()
}

fn maps(out: &BTreeMap<String, Value>, key: &str) -> Vec<IndexMap<String, Value>> {
    match out.get(key) {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::Map(map) => Some(map.clone()),
                _ => None,
            })
            .collect(),
        other => panic!("{key} is not a list: {other:?}"),
    }
}

fn field(map: &IndexMap<String, Value>, key: &str) -> String {
    map.get(key).map(to_plain_string).unwrap_or_default()
}

fn column(rows: &[IndexMap<String, Value>], key: &str) -> Vec<String> {
    rows.iter().map(|row| field(row, key)).collect()
}

/// A page saved through update#WikiPage; its id.
fn save(space: &str, path: &str, body: &str, note: Option<&str>) -> String {
    let (parent, name) = match path.rfind('/') {
        Some(slash) => (&path[..slash], &path[slash + 1..]),
        None => ("", path),
    };
    let mut parameters = vec![
        ("wikiSpaceId", text(space)),
        ("parentPath", text(parent)),
        ("pageName", text(name)),
        ("pageText", text(body)),
    ];
    if let Some(note) = note {
        parameters.push(("versionNote", text(note)));
    }
    out_text(&ok("update#WikiPage", &parameters), "wikiPageId")
}

fn space(id: &str, extra: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let mut parameters = vec![("wikiSpaceId", text(id)), ("description", text(&format!("{id} space")))];
    parameters.extend(extra.iter().cloned());
    ok("create#WikiSpace", &parameters)
}

fn one(facade: &EntityFacade, entity: &str, pk: &[(&str, &str)]) -> Option<crate::entity::EntityValue> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut find = facade.find(entity).unwrap();
    for (name, value) in pk {
        find = find.eq(name, Value::string(*value));
    }
    find.one().unwrap()
}

fn value_of(row: &Option<crate::entity::EntityValue>, name: &str) -> String {
    row.as_ref()
        .and_then(|row| row.get(name).map(to_plain_string))
        .unwrap_or_default()
}

fn exists(ctx: &ExecutionContext, location: &str) -> bool {
    ctx.factory()
        .resource()
        .get(location)
        .map(|found| found.exists())
        .unwrap_or(false)
}

fn millis(value: &Value) -> i64 {
    crate::context::millis_of(value).unwrap_or(-1)
}

#[test]
fn version_notes_history_compare_and_restore_on_both_storages() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    users(&facade);
    login("alice");
    space("VNT", &[]);
    at(&ctx, 0);
    let page = save("VNT", "Plan", "# Plan\n\nOne line.\n", Some("First draft"));
    at(&ctx, 1);
    save("VNT", "Plan", "# Plan\n\nOne line.\nTwo lines.\n", Some("Second"));
    at(&ctx, 2);
    save("VNT", "Plan", "# Plan\n\nOne line.\nTwo lines.\nThree.\n", None);

    let history = ok("get#WikiPageHistory", &[("wikiPageId", text(&page))]);
    assert_eq!(out_text(&history, "storage"), "db");
    let versions = maps(&history, "versionList");
    assert_eq!(column(&versions, "versionName"), ["03", "02", "01"]);
    assert_eq!(column(&versions, "note"), ["", "Second", "First draft"]);
    assert_eq!(column(&versions, "userName"), ["alice Example"; 3]);
    assert_eq!(field(&versions[0], "isCurrent"), "true");
    let rows = maps(&history, "historyList");
    assert_eq!(column(&rows, "versionNote"), ["", "Second", "First draft"]);

    // the compare of two versions, and the version before the current one by default
    let diff = ok(
        "get#WikiPageVersionDiff",
        &[("wikiPageId", text(&page)), ("fromVersionName", text("01"))],
    );
    let Some(Value::Map(diff_map)) = diff.get("diff") else {
        panic!("no diff: {diff:?}");
    };
    assert!(field(diff_map, "html").contains("moqui-diff"));
    assert_eq!(out_text(&diff, "toVersionName"), "03");
    let previous = ok("get#WikiPageVersionDiff", &[("wikiPageId", text(&page))]);
    assert_eq!(out_text(&previous, "fromVersionName"), "02");

    // a restore is a new version with a note
    let restored = ok(
        "restore#WikiPageVersion",
        &[("wikiPageId", text(&page)), ("versionName", text("01"))],
    );
    assert_eq!(out_text(&restored, "newVersionName"), "04");
    let history = ok("get#WikiPageHistory", &[("wikiPageId", text(&page))]);
    let versions = maps(&history, "versionList");
    assert_eq!(field(&versions[0], "note"), "Restored version 01");
    let info = ok("get#WikiPageInfo", &[("wikiPageId", text(&page))]);
    assert_eq!(out_text(&info, "pageLocation"), "dbresource://WikiSpace/VNT/Plan.md");
    assert_eq!(out_text(&info, "currentVersionName"), "04");

    // on a space in a git repository the note is the commit message
    let made = space("GVN", &[("repositoryKind", text("db"))]);
    assert_eq!(out_text(&made, "repositoryId"), "WIKI_GVN");
    save("GVN", "Plan", "plan\n", Some("Commit message here"));
    let git_history = ctx
        .factory()
        .resource()
        .version_history("git://WIKI_GVN/GVN/Plan.md")
        .unwrap();
    assert_eq!(git_history.len(), 1);
    assert_eq!(git_history[0].note.as_deref(), Some("Commit message here"));
    assert_eq!(git_history[0].user_id.as_deref(), Some("alice"));
}

#[test]
fn links_are_indexed_and_backlinks_follow_a_rename_an_alias_and_a_new_page() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    users(&facade);
    login("alice");
    space("LNK", &[]);
    let alpha = save("LNK", "Alpha", "# Alpha\n", None);
    let beta = save(
        "LNK",
        "Beta",
        "# Beta\n\nSee [to alpha](Alpha) and [the same](wiki:LNK/Alpha.md#top), [[Gamma]], ![a picture](Alpha.png),\n\
         [a site](https://example.com) and [mail](mailto:a@example.com).\n\n```\n[in code](Delta)\n```\n\nAnd `[span](Delta)`.\n",
        None,
    );
    save("LNK", "Docs", "# Docs\n", None);
    let guide = save(
        "LNK",
        "Docs/Guide",
        "Back [up](../Alpha) or [[Docs]] or [ref][r].\n\n[r]: ../Beta\n",
        None,
    );

    let backlinks = ok("get#WikiPageBacklinks", &[("wikiPageId", text(&alpha))]);
    let rows = maps(&backlinks, "backlinkList");
    assert_eq!(column(&rows, "pagePath"), ["Beta", "Docs/Guide"]);
    assert_eq!(field(&rows[0], "linkText"), "to alpha");
    let links = ok("get#WikiPageLinks", &[("wikiPageId", text(&beta))]);
    let rows = maps(&links, "linkList");
    assert_eq!(
        column(&rows, "toPagePath"),
        ["Alpha", "Gamma"],
        "no code, picture, URL or repeat"
    );
    assert_eq!(column(&rows, "found"), ["true", "false"]);
    let guide_links = maps(&ok("get#WikiPageLinks", &[("wikiPageId", text(&guide))]), "linkList");
    assert_eq!(column(&guide_links, "toPagePath"), ["Alpha", "Beta", "Docs"]);

    // a page made at a path takes the links already written to it
    let gamma = save("LNK", "Gamma", "# Gamma\n", None);
    let rows = maps(&ok("get#WikiPageLinks", &[("wikiPageId", text(&beta))]), "linkList");
    assert_eq!(column(&rows, "found"), ["true", "true"]);
    assert_eq!(field(&rows[1], "toWikiPageId"), gamma);

    // a rename keeps the backlinks, leaves an alias at the old path, and the old path reads the page
    let renamed = ok(
        "update#WikiPage",
        &[
            ("wikiSpaceId", text("LNK")),
            ("wikiPageId", text(&alpha)),
            ("pageName", text("First")),
            ("pageText", text("# Alpha\n")),
        ],
    );
    assert_eq!(out_text(&renamed, "pagePath"), "First");
    let rows = maps(
        &ok("get#WikiPageBacklinks", &[("wikiPageId", text(&alpha))]),
        "backlinkList",
    );
    assert_eq!(column(&rows, "pagePath"), ["Beta", "Docs/Guide"]);
    assert_eq!(
        value_of(
            &one(
                &facade,
                "moqui.resource.wiki.WikiPageAlias",
                &[("wikiSpaceId", "LNK"), ("aliasPath", "Alpha")]
            ),
            "wikiPageId"
        ),
        alpha
    );
    let by_alias = ok(
        "get#WikiPageInfo",
        &[("wikiSpaceId", text("LNK")), ("pagePath", text("Alpha"))],
    );
    assert_eq!(out_text(&by_alias, "pagePath"), "First");
    assert_eq!(out_text(&by_alias, "wikiPageId"), alpha);
    let moved = maps(
        &ok("get#WikiPageHistory", &[("wikiPageId", text(&alpha))]),
        "historyList",
    );
    assert_eq!(field(&moved[0], "oldPagePath"), "Alpha");

    // a new page at the old path takes it back: the alias goes, the links are the new page's
    let second = save("LNK", "Alpha", "# A new Alpha\n", None);
    assert_ne!(second, alpha);
    assert!(one(
        &facade,
        "moqui.resource.wiki.WikiPageAlias",
        &[("wikiSpaceId", "LNK"), ("aliasPath", "Alpha")]
    )
    .is_none());
    let rows = maps(
        &ok("get#WikiPageBacklinks", &[("wikiPageId", text(&second))]),
        "backlinkList",
    );
    assert_eq!(column(&rows, "pagePath"), ["Beta", "Docs/Guide"]);
    assert!(maps(
        &ok("get#WikiPageBacklinks", &[("wikiPageId", text(&alpha))]),
        "backlinkList"
    )
    .is_empty());

    // a deleted page leaves links to a page not there
    ok(
        "delete#WikiPage",
        &[("wikiSpaceId", text("LNK")), ("pagePath", text("Alpha"))],
    );
    let rows = maps(&ok("get#WikiPageLinks", &[("wikiPageId", text(&beta))]), "linkList");
    assert_eq!(column(&rows, "found"), ["false", "true"]);

    // a backlink from a restricted space is only for those who may view it
    space("SEC", &[("restrictView", text("Y")), ("restrictUpdate", text("Y"))]);
    save("SEC", "Secret", "Links to [first](wiki:LNK/First).\n", None);
    let rows = maps(
        &ok("get#WikiPageBacklinks", &[("wikiPageId", text(&gamma))]),
        "backlinkList",
    );
    assert_eq!(column(&rows, "pagePath"), ["Beta"]);
    let rows = maps(
        &ok("get#WikiPageBacklinks", &[("wikiPageId", text(&alpha))]),
        "backlinkList",
    );
    assert_eq!(column(&rows, "wikiSpaceId"), ["SEC"]);
    login("bob");
    assert!(maps(
        &ok("get#WikiPageBacklinks", &[("wikiPageId", text(&alpha))]),
        "backlinkList"
    )
    .is_empty());
    refused("get#WikiSpacePages", &[("wikiSpaceId", text("SEC"))], "not found");
}

/// A note (the user's notes space, R22, on database files) moved into a space in a git repository: each version
/// replayed oldest first as a commit by its own author at its own date, with a note naming where it came from; the
/// page keeps its id, its publish and history rows follow the new version names, its child page and attachment come
/// along, the old files go, and the old path still finds it.
#[test]
fn a_note_moves_into_a_git_space_with_its_history_replayed() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    users(&facade);
    login("alice");
    let notes = ok("ensure#UserNotesSpace", &[]);
    let notes_space = out_text(&notes, "wikiSpaceId");
    assert_eq!(notes_space, "NOTES_alice");
    // bob may write in alice's notes, so a version is his
    row(
        &facade,
        "moqui.resource.wiki.WikiSpaceUser",
        &[
            ("wikiSpaceId", text(&notes_space)),
            ("userId", text("bob")),
            ("allowView", text("Y")),
            ("allowUpdate", text("Y")),
        ],
    );
    at(&ctx, 0);
    let page = save(&notes_space, "Groceries", "# Groceries\n\n- milk\n", Some("First list"));
    at(&ctx, 1);
    save(&notes_space, "Groceries", "# Groceries\n\n- milk\n- bread\n", None);
    ok(
        "set#PublishedVersion",
        &[("wikiPageId", text(&page)), ("publishedVersionName", text("02"))],
    );
    login("bob");
    at(&ctx, 2);
    save(
        &notes_space,
        "Groceries",
        "# Groceries\n\n- milk\n- bread\n- eggs\n",
        Some("Added eggs"),
    );
    login("alice");
    at(&ctx, 3);
    let child = save(&notes_space, "Groceries/Weekly", "Every week\n", None);
    facade
        .write_db_resource(
            "dbresource://WikiSpace/NOTES_alice/Groceries/_attachments/list.txt",
            b"attached",
        )
        .unwrap();

    at(&ctx, 5);
    space("KNOW", &[("repositoryKind", text("db"))]);
    at(&ctx, 6);
    let moved = ok(
        "move#WikiPage",
        &[
            ("wikiPageId", text(&page)),
            ("toWikiSpaceId", text("KNOW")),
            ("versionNote", text("Promote the list")),
        ],
    );
    assert_eq!(out_text(&moved, "replayed"), "true");
    assert_eq!(
        out_text(&moved, "versionCount"),
        "5",
        "three versions, the child and the attachment"
    );
    assert_eq!(out_text(&moved, "pagePath"), "Groceries");
    assert_eq!(out_text(&moved, "wikiSpaceId"), "KNOW");

    let location = "git://WIKI_KNOW/KNOW/Groceries.md";
    let history = ctx.factory().resource().version_history(location).unwrap();
    assert_eq!(history.len(), 3);
    let users_of: Vec<_> = history
        .iter()
        .map(|version| version.user_id.clone().unwrap_or_default())
        .collect();
    assert_eq!(users_of, ["bob", "alice", "alice"], "each version by its own author");
    let dates: Vec<_> = history.iter().map(|version| millis(&version.version_date)).collect();
    assert_eq!(
        dates,
        [BASE_TIME + 2 * HOUR, BASE_TIME + HOUR, BASE_TIME],
        "and at its own date"
    );
    let notes_of: Vec<_> = history
        .iter()
        .map(|version| version.note.clone().unwrap_or_default())
        .collect();
    assert!(
        notes_of[0].starts_with("Added eggs\n\nMoved from NOTES_alice/Groceries, version 03"),
        "{notes_of:?}"
    );
    assert!(notes_of[1].starts_with("Version 02\n\nMoved from"), "{notes_of:?}");
    assert!(notes_of[2].starts_with("First list"), "{notes_of:?}");
    assert!(notes_of[0].ends_with("by bob Example"), "{notes_of:?}");
    let resources = ctx.factory().resource();
    assert_eq!(
        resources.get(location).unwrap().read_text().unwrap(),
        "# Groceries\n\n- milk\n- bread\n- eggs\n"
    );
    assert_eq!(
        resources
            .get("git://WIKI_KNOW/KNOW/Groceries/_attachments/list.txt")
            .unwrap()
            .read_text()
            .unwrap(),
        "attached"
    );
    assert!(exists(&ctx, "git://WIKI_KNOW/KNOW/Groceries/Weekly.md"));
    assert!(!exists(&ctx, "dbresource://WikiSpace/NOTES_alice/Groceries.md"));
    assert!(!exists(&ctx, "dbresource://WikiSpace/NOTES_alice/Groceries"));

    // the rows: the same ids, the publish and the history on the new version names, the move recorded, an alias
    let row_of = one(&facade, "moqui.resource.wiki.WikiPage", &[("wikiPageId", &page)]);
    assert_eq!(value_of(&row_of, "wikiSpaceId"), "KNOW");
    assert_eq!(value_of(&row_of, "publishedVersionName"), history[1].version_name);
    let child_row = one(&facade, "moqui.resource.wiki.WikiPage", &[("wikiPageId", &child)]);
    assert_eq!(value_of(&child_row, "pagePath"), "Groceries/Weekly");
    let page_history = ok("get#WikiPageHistory", &[("wikiPageId", text(&page))]);
    assert_eq!(out_text(&page_history, "storage"), "git");
    let versions = maps(&page_history, "versionList");
    assert_eq!(field(&versions[0], "movedFrom"), "NOTES_alice/Groceries");
    assert_eq!(field(&versions[0], "userName"), "bob Example");
    let rows = maps(&page_history, "historyList");
    assert_eq!(field(&rows[0], "oldWikiSpaceId"), "NOTES_alice");
    assert_eq!(field(&rows[0], "versionNote"), "Promote the list");
    let saved: Vec<String> = rows[1..].iter().map(|row| field(row, "versionName")).collect();
    let mut expected: Vec<String> = history.iter().map(|version| version.version_name.clone()).collect();
    expected.reverse();
    let mut saved_sorted = saved.clone();
    saved_sorted.sort();
    expected.sort();
    assert_eq!(saved_sorted, expected, "the save rows name the commits");
    assert_eq!(
        value_of(
            &one(
                &facade,
                "moqui.resource.wiki.WikiPageAlias",
                &[("wikiSpaceId", "NOTES_alice"), ("aliasPath", "Groceries")]
            ),
            "wikiPageId"
        ),
        page
    );
    let by_old_path = ok(
        "get#WikiPageInfo",
        &[("wikiSpaceId", text("NOTES_alice")), ("pagePath", text("Groceries"))],
    );
    assert_eq!(out_text(&by_old_path, "pageLocation"), location);

    // in the same repository a move is one commit, and the history follows it
    ok(
        "move#WikiPage",
        &[("wikiPageId", text(&page)), ("toPageName", text("Shopping"))],
    );
    let renamed = resources.version_history("git://WIKI_KNOW/KNOW/Shopping.md").unwrap();
    assert_eq!(renamed.len(), 4, "the move and the three replayed versions");
    assert!(exists(&ctx, "git://WIKI_KNOW/KNOW/Shopping/Weekly.md"));
    assert_eq!(
        value_of(
            &one(&facade, "moqui.resource.wiki.WikiPage", &[("wikiPageId", &child)]),
            "pagePath"
        ),
        "Shopping/Weekly"
    );

    // a page is not moved under itself, onto another page, or by a user who may not update it
    refused(
        "move#WikiPage",
        &[("wikiPageId", text(&page)), ("toParentPath", text("Shopping"))],
        "under itself",
    );
    save("KNOW", "Taken", "taken\n", None);
    refused(
        "move#WikiPage",
        &[("wikiPageId", text(&page)), ("toPageName", text("Taken"))],
        "already",
    );
    login("carol");
    ok("ensure#UserNotesSpace", &[]);
    let note = save("NOTES_carol", "Mine", "mine\n", None);
    login("bob");
    refused(
        "move#WikiPage",
        &[("wikiPageId", text(&note)), ("toWikiSpaceId", text("KNOW"))],
        "Page not found",
    );
}

#[test]
fn a_page_moves_on_the_database_tree_with_its_history_rows() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    users(&facade);
    login("alice");
    space("ONE", &[]);
    space("TWO", &[]);
    let plan = save("ONE", "Plan", "one\n", Some("v1"));
    save("ONE", "Plan", "two\n", Some("v2"));
    save("ONE", "Plan/Detail", "detail\n", None);
    save("TWO", "Team", "team\n", None);
    let moved = ok(
        "move#WikiPage",
        &[
            ("wikiSpaceId", text("ONE")),
            ("pagePath", text("Plan")),
            ("toWikiSpaceId", text("TWO")),
            ("toParentPath", text("Team")),
        ],
    );
    assert_eq!(out_text(&moved, "replayed"), "false");
    assert_eq!(out_text(&moved, "pagePath"), "Team/Plan");
    let history = ok("get#WikiPageHistory", &[("wikiPageId", text(&plan))]);
    let versions = maps(&history, "versionList");
    assert_eq!(
        column(&versions, "versionName"),
        ["02", "01"],
        "the history rows moved with the file"
    );
    assert_eq!(column(&versions, "note"), ["v2", "v1"]);
    assert!(exists(&ctx, "dbresource://WikiSpace/TWO/Team/Plan/Detail.md"));
    assert!(!exists(&ctx, "dbresource://WikiSpace/ONE/Plan.md"));
    let team = one(
        &facade,
        "moqui.resource.wiki.WikiPage",
        &[("wikiSpaceId", "TWO"), ("pagePath", "Team")],
    );
    assert_eq!(
        value_of(
            &one(&facade, "moqui.resource.wiki.WikiPage", &[("wikiPageId", &plan)]),
            "parentWikiPageId"
        ),
        value_of(&team, "wikiPageId")
    );
    refused(
        "move#WikiPage",
        &[
            ("wikiPageId", text(&plan)),
            ("toWikiSpaceId", text("ONE")),
            ("toParentPath", text("Nowhere")),
        ],
        "Parent page not found",
    );
}

#[test]
fn spaces_are_made_and_cloned_on_a_git_repository_with_or_without_history() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    users(&facade);
    login("alice");
    let made = space("DOCS", &[("repositoryKind", text("db"))]);
    assert_eq!(out_text(&made, "rootPageLocation"), "git://WIKI_DOCS/DOCS.md");
    assert!(exists(&ctx, "git://WIKI_DOCS/DOCS.md"));
    assert_eq!(
        value_of(
            &one(
                &facade,
                "moqui.resource.GitRepository",
                &[("repositoryId", "WIKI_DOCS")]
            ),
            "repositoryKind"
        ),
        "db"
    );
    let member = one(
        &facade,
        "moqui.resource.wiki.WikiSpaceUser",
        &[("wikiSpaceId", "DOCS"), ("userId", "alice")],
    );
    assert_eq!(value_of(&member, "allowAdmin"), "Y");
    let access = ok("get#WikiPageAccess", &[("wikiSpaceId", text("DOCS"))]);
    assert_eq!(out_text(&access, "storage"), "git");
    assert_eq!(out_text(&access, "canUpdate"), "true");
    refused(
        "create#WikiSpace",
        &[
            ("wikiSpaceId", text("MIR")),
            ("repositoryKind", text("remote")),
            ("remoteUrl", text("https://example.com/docs.git")),
        ],
        "Only an administrator",
    );
    refused(
        "create#WikiSpace",
        &[("wikiSpaceId", text("BAD")), ("repositoryKind", text("svn"))],
        "Unknown space storage",
    );

    // a base space on database files with history, cloned into a git repository with it, and without it
    space("SRC", &[]);
    at(&ctx, 0);
    let one_id = save("SRC", "One", "1\n", Some("first"));
    at(&ctx, 1);
    save("SRC", "One", "2\n", Some("second"));
    at(&ctx, 2);
    save("SRC", "One", "3\n", None);
    save("SRC", "One/Two", "two [one](../One)\n", None);
    ok(
        "set#PublishedVersion",
        &[("wikiPageId", text(&one_id)), ("publishedVersionName", text("02"))],
    );
    at(&ctx, 4);
    let cloned = ok(
        "clone#WikiSpace",
        &[
            ("baseWikiSpaceId", text("SRC")),
            ("wikiSpaceId", text("CPY")),
            ("repositoryKind", text("db")),
            ("copyHistory", Value::Bool(true)),
        ],
    );
    assert_eq!(out_text(&cloned, "repositoryId"), "WIKI_CPY");
    let history = ctx
        .factory()
        .resource()
        .version_history("git://WIKI_CPY/CPY/One.md")
        .unwrap();
    assert_eq!(history.len(), 3);
    assert!(history[2]
        .note
        .as_deref()
        .unwrap_or("")
        .starts_with("first\n\nCloned from SRC"));
    assert_eq!(millis(&history[2].version_date), BASE_TIME);
    let copy = one(
        &facade,
        "moqui.resource.wiki.WikiPage",
        &[("wikiSpaceId", "CPY"), ("pagePath", "One")],
    );
    assert_ne!(value_of(&copy, "wikiPageId"), one_id);
    assert_eq!(value_of(&copy, "publishedVersionName"), history[1].version_name);
    let copy_id = value_of(&copy, "wikiPageId");
    let rows = maps(
        &ok("get#WikiPageHistory", &[("wikiPageId", text(&copy_id))]),
        "historyList",
    );
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|row| field(row, "versionName").len() == 40), "{rows:?}");
    let backlinks = maps(
        &ok("get#WikiPageBacklinks", &[("wikiPageId", text(&copy_id))]),
        "backlinkList",
    );
    assert_eq!(
        column(&backlinks, "wikiSpaceId"),
        ["CPY"],
        "the copy's links are its own"
    );

    let plain = ok(
        "clone#WikiSpace",
        &[("baseWikiSpaceId", text("SRC")), ("wikiSpaceId", text("CP2"))],
    );
    assert_eq!(out_text(&plain, "wikiSpaceId"), "CP2");
    let plain_history = ok(
        "get#WikiPageHistory",
        &[("wikiSpaceId", text("CP2")), ("pagePath", text("One"))],
    );
    assert_eq!(column(&maps(&plain_history, "versionList"), "versionName"), ["01"]);
    // the clone's root page record is its own, not the base space's
    let roots = {
        let _authz = ctx.disable_authz();
        facade
            .find("moqui.resource.wiki.WikiPage")
            .unwrap()
            .eq("pagePath", Value::Null)
            .list()
            .unwrap()
            .iter()
            .map(|row| row.get("wikiSpaceId").map(to_plain_string).unwrap_or_default())
            .collect::<Vec<_>>()
    };
    assert!(roots.contains(&"CP2".to_string()), "{roots:?}");
}

/// clone#WikiPage names the copy, copies the pages under it when asked, writes attachments on the new path, and
/// copies aliases unless told not to. clone#WikiSpace leaves both off the call, so a child page and an alias still come.
#[test]
fn a_page_is_cloned_under_a_new_name_with_its_children_attachments_and_aliases() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    users(&facade);
    login("alice");
    space("CLN", &[]);
    let guide = save("CLN", "Guide", "# Guide\n", None);
    save("CLN", "Guide/Chapter", "chapter\n", None);
    save("CLN", "Guide/Chapter/Section", "section\n", None);
    save("CLN", "GuideExtra", "not a child\n", None);
    ctx.factory()
        .resource()
        .write_bytes(
            "dbresource://WikiSpace/CLN/Guide/_attachments/notes.txt",
            b"attached note\n",
        )
        .unwrap();
    row(
        &facade,
        "moqui.resource.wiki.WikiPageAlias",
        &[
            ("wikiSpaceId", text("CLN")),
            ("aliasPath", text("OldGuide")),
            ("wikiPageId", text(&guide)),
        ],
    );

    space("CLD", &[]);
    save("CLD", "Shelf", "# Shelf\n", None);
    let cloned = ok(
        "clone#WikiPage",
        &[
            ("baseWikiSpaceId", text("CLN")),
            ("wikiSpaceId", text("CLD")),
            ("pagePath", text("Guide")),
            ("parentPath", text("Shelf")),
            ("pageName", text("Handbook")),
            ("withChildren", Value::Bool(true)),
        ],
    );
    assert_eq!(out_text(&cloned, "pagePath"), "Shelf/Handbook");
    let handbook = one(
        &facade,
        "moqui.resource.wiki.WikiPage",
        &[("wikiSpaceId", "CLD"), ("pagePath", "Shelf/Handbook")],
    );
    assert_eq!(value_of(&handbook, "wikiPageId"), out_text(&cloned, "wikiPageId"));
    assert_ne!(value_of(&handbook, "wikiPageId"), guide);
    assert!(one(
        &facade,
        "moqui.resource.wiki.WikiPage",
        &[("wikiSpaceId", "CLD"), ("pagePath", "Shelf/Guide")]
    )
    .is_none());
    assert!(one(
        &facade,
        "moqui.resource.wiki.WikiPage",
        &[("wikiSpaceId", "CLD"), ("pagePath", "Guide")]
    )
    .is_none());
    assert!(one(
        &facade,
        "moqui.resource.wiki.WikiPage",
        &[("wikiSpaceId", "CLD"), ("pagePath", "Shelf/Handbook/Chapter")]
    )
    .is_some());
    assert!(one(
        &facade,
        "moqui.resource.wiki.WikiPage",
        &[("wikiSpaceId", "CLD"), ("pagePath", "Shelf/Handbook/Chapter/Section")]
    )
    .is_some());
    assert!(one(
        &facade,
        "moqui.resource.wiki.WikiPage",
        &[("wikiSpaceId", "CLD"), ("pagePath", "GuideExtra")]
    )
    .is_none());
    assert_eq!(
        location_text(&ctx, "dbresource://WikiSpace/CLD/Shelf/Handbook.md"),
        "# Guide\n"
    );
    assert_eq!(
        location_text(&ctx, "dbresource://WikiSpace/CLD/Shelf/Handbook/Chapter.md"),
        "chapter\n"
    );

    let attachment = ok(
        "get#WikiPageAttachment",
        &[
            ("wikiSpaceId", text("CLD")),
            ("pagePath", text("Shelf/Handbook")),
            ("filename", text("notes.txt")),
        ],
    );
    let Some(Value::Map(reference)) = attachment.get("attachmentReference") else {
        panic!("no attachment reference: {attachment:?}");
    };
    let new_attachment = "dbresource://WikiSpace/CLD/Shelf/Handbook/_attachments/notes.txt";
    assert_eq!(field(reference, "location"), new_attachment);
    assert_eq!(location_text(&ctx, new_attachment), "attached note\n");
    assert!(!exists(&ctx, "dbresource://WikiSpace/CLD/Guide/_attachments/notes.txt"));
    assert!(!exists(
        &ctx,
        "dbresource://WikiSpace/CLD/Shelf/Guide/_attachments/notes.txt"
    ));
    assert_eq!(
        location_text(&ctx, "dbresource://WikiSpace/CLN/Guide/_attachments/notes.txt"),
        "attached note\n",
        "the source file stays"
    );
    assert_eq!(
        value_of(
            &one(
                &facade,
                "moqui.resource.wiki.WikiPageAlias",
                &[("wikiSpaceId", "CLD"), ("aliasPath", "OldGuide")]
            ),
            "wikiPageId"
        ),
        out_text(&cloned, "wikiPageId"),
        "the alias is copied when copyAliases is omitted"
    );

    space("CLA", &[]);
    ok(
        "clone#WikiPage",
        &[
            ("baseWikiSpaceId", text("CLN")),
            ("wikiSpaceId", text("CLA")),
            ("pagePath", text("Guide")),
            ("copyAliases", Value::Bool(false)),
        ],
    );
    assert!(one(
        &facade,
        "moqui.resource.wiki.WikiPage",
        &[("wikiSpaceId", "CLA"), ("pagePath", "Guide")]
    )
    .is_some());
    assert!(one(
        &facade,
        "moqui.resource.wiki.WikiPage",
        &[("wikiSpaceId", "CLA"), ("pagePath", "Guide/Chapter")]
    )
    .is_none());
    assert!(one(
        &facade,
        "moqui.resource.wiki.WikiPageAlias",
        &[("wikiSpaceId", "CLA"), ("aliasPath", "OldGuide")]
    )
    .is_none());

    ok(
        "clone#WikiSpace",
        &[("baseWikiSpaceId", text("CLN")), ("wikiSpaceId", text("CLS"))],
    );
    let copied = one(
        &facade,
        "moqui.resource.wiki.WikiPage",
        &[("wikiSpaceId", "CLS"), ("pagePath", "Guide")],
    );
    assert!(one(
        &facade,
        "moqui.resource.wiki.WikiPage",
        &[("wikiSpaceId", "CLS"), ("pagePath", "Guide/Chapter")]
    )
    .is_some());
    assert_eq!(
        value_of(
            &one(
                &facade,
                "moqui.resource.wiki.WikiPageAlias",
                &[("wikiSpaceId", "CLS"), ("aliasPath", "OldGuide")]
            ),
            "wikiPageId"
        ),
        value_of(&copied, "wikiPageId")
    );
    assert_eq!(
        location_text(&ctx, "dbresource://WikiSpace/CLS/Guide/_attachments/notes.txt"),
        "attached note\n"
    );
}

/// clone#WikiPage with copyHistory replays the page's versions into a database space and a git space, with its
/// attachments but not the pages under it unless asked; its published version, history, user and alias rows follow
/// with the new version names. usePublished copies the published version's text, and withChildren copies the pages
/// under it by their files, so a page that is only a file comes too.
#[test]
fn a_page_is_cloned_with_its_history_its_published_text_and_its_file_only_children() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    users(&facade);
    login("alice");
    space("HSR", &[]);
    at(&ctx, 0);
    let doc = save("HSR", "Doc", "1\n", Some("first"));
    at(&ctx, 1);
    save("HSR", "Doc", "2\n", Some("second"));
    at(&ctx, 2);
    save("HSR", "Doc", "3\n", None);
    save("HSR", "Doc/Sub", "sub one\n", None);
    at(&ctx, 3);
    save("HSR", "Doc/Sub", "sub two\n", None);
    ok(
        "set#PublishedVersion",
        &[("wikiPageId", text(&doc)), ("publishedVersionName", text("02"))],
    );
    let resource = ctx.factory().resource();
    resource
        .write_bytes("dbresource://WikiSpace/HSR/Doc/_attachments/a.txt", b"attached\n")
        .unwrap();
    // pages that are only files, with no WikiPage row
    resource
        .write_bytes("dbresource://WikiSpace/HSR/Doc/Loose.md", b"loose\n")
        .unwrap();
    resource
        .write_bytes("dbresource://WikiSpace/HSR/Doc/Loose/Deep.md", b"deep\n")
        .unwrap();
    row(
        &facade,
        "moqui.resource.wiki.WikiPageUser",
        &[("wikiPageId", text(&doc)), ("userId", text("bob")), ("allowView", text("Y"))],
    );
    row(
        &facade,
        "moqui.resource.wiki.WikiPageAlias",
        &[("wikiSpaceId", text("HSR")), ("aliasPath", text("OldDoc")), ("wikiPageId", text(&doc))],
    );
    at(&ctx, 4);

    // with its history into a database space, under a new name, with the pages under it
    space("HDB", &[]);
    let cloned = ok(
        "clone#WikiPage",
        &[
            ("baseWikiSpaceId", text("HSR")),
            ("wikiSpaceId", text("HDB")),
            ("pagePath", text("Doc")),
            ("parentPath", text("HDB")),
            ("pageName", text("Copy")),
            ("copyHistory", Value::Bool(true)),
            ("withChildren", Value::Bool(true)),
        ],
    );
    assert_eq!(out_text(&cloned, "pagePath"), "Copy");
    assert_eq!(cloned.get("failedFiles"), Some(&Value::List(Vec::new())), "{cloned:?}");
    let copy_id = out_text(&cloned, "wikiPageId");
    assert_ne!(copy_id, doc);
    let history = ok("get#WikiPageHistory", &[("wikiPageId", text(&copy_id))]);
    let versions = maps(&history, "versionList");
    assert_eq!(column(&versions, "versionName"), ["03", "02", "01"]);
    assert!(field(&versions[2], "note").starts_with("first\n\nCopied from HSR/Doc"), "{versions:?}");
    assert_eq!(maps(&history, "historyList").len(), 3);
    let copy = one(&facade, "moqui.resource.wiki.WikiPage", &[("wikiPageId", &copy_id)]);
    assert_eq!(value_of(&copy, "publishedVersionName"), "02");
    assert_eq!(value_of(&copy, "pagePath"), "Copy");
    assert!(one(
        &facade,
        "moqui.resource.wiki.WikiPageUser",
        &[("wikiPageId", &copy_id), ("userId", "bob")]
    )
    .is_some());
    assert_eq!(
        value_of(
            &one(
                &facade,
                "moqui.resource.wiki.WikiPageAlias",
                &[("wikiSpaceId", "HDB"), ("aliasPath", "OldDoc")]
            ),
            "wikiPageId"
        ),
        copy_id
    );
    assert_eq!(
        location_text(&ctx, "dbresource://WikiSpace/HDB/Copy/_attachments/a.txt"),
        "attached\n"
    );
    let sub = ok(
        "get#WikiPageHistory",
        &[("wikiSpaceId", text("HDB")), ("pagePath", text("Copy/Sub"))],
    );
    assert_eq!(column(&maps(&sub, "versionList"), "versionName"), ["02", "01"]);
    assert_eq!(location_text(&ctx, "dbresource://WikiSpace/HDB/Copy/Sub.md"), "sub two\n");
    assert_eq!(location_text(&ctx, "dbresource://WikiSpace/HDB/Copy/Loose.md"), "loose\n");
    assert_eq!(
        location_text(&ctx, "dbresource://WikiSpace/HDB/Copy/Loose/Deep.md"),
        "deep\n"
    );
    assert!(one(
        &facade,
        "moqui.resource.wiki.WikiPage",
        &[("wikiSpaceId", "HDB"), ("pagePath", "Copy/Loose/Deep")]
    )
    .is_some());

    // into a git space: one commit per version, the published version by its new name; the page only, without children
    space("HGT", &[("repositoryKind", text("db"))]);
    let git = ok(
        "clone#WikiPage",
        &[
            ("baseWikiSpaceId", text("HSR")),
            ("wikiSpaceId", text("HGT")),
            ("pagePath", text("Doc")),
            ("parentPath", text("HGT")),
            ("copyHistory", Value::Bool(true)),
            ("copyAliases", Value::Bool(false)),
        ],
    );
    let git_id = out_text(&git, "wikiPageId");
    let commits = resource.version_history("git://WIKI_HGT/HGT/Doc.md").unwrap();
    assert_eq!(commits.len(), 3);
    assert_eq!(millis(&commits[2].version_date), BASE_TIME);
    assert_eq!(commits[2].user_id.as_deref(), Some("alice"));
    let git_page = one(&facade, "moqui.resource.wiki.WikiPage", &[("wikiPageId", &git_id)]);
    assert_eq!(value_of(&git_page, "publishedVersionName"), commits[1].version_name);
    let git_rows = maps(
        &ok("get#WikiPageHistory", &[("wikiPageId", text(&git_id))]),
        "historyList",
    );
    assert_eq!(git_rows.len(), 3);
    assert!(git_rows.iter().all(|row| field(row, "versionName").len() == 40), "{git_rows:?}");
    assert!(exists(&ctx, "git://WIKI_HGT/HGT/Doc/_attachments/a.txt"));
    assert!(!exists(&ctx, "git://WIKI_HGT/HGT/Doc/Sub.md"), "the pages under it stay out");
    assert!(!exists(&ctx, "git://WIKI_HGT/HGT/Doc/Loose.md"));
    assert!(one(
        &facade,
        "moqui.resource.wiki.WikiPageAlias",
        &[("wikiSpaceId", "HGT"), ("aliasPath", "OldDoc")]
    )
    .is_none());

    // the published version's text, as one new version with the note given; the latest text without usePublished
    space("HPB", &[]);
    let published = ok(
        "clone#WikiPage",
        &[
            ("baseWikiSpaceId", text("HSR")),
            ("wikiSpaceId", text("HPB")),
            ("pagePath", text("Doc")),
            ("parentPath", text("HPB")),
            ("usePublished", Value::Bool(true)),
            ("versionNote", text("Copied for the review")),
        ],
    );
    assert_eq!(location_text(&ctx, "dbresource://WikiSpace/HPB/Doc.md"), "2\n");
    let one_version = maps(
        &ok("get#WikiPageHistory", &[("wikiPageId", text(&out_text(&published, "wikiPageId")))]),
        "versionList",
    );
    assert_eq!(column(&one_version, "note"), ["Copied for the review"]);
    ok(
        "clone#WikiPage",
        &[
            ("baseWikiSpaceId", text("HSR")),
            ("wikiSpaceId", text("HPB")),
            ("pagePath", text("Doc")),
            ("parentPath", text("HPB")),
            ("pageName", text("Latest")),
        ],
    );
    assert_eq!(location_text(&ctx, "dbresource://WikiSpace/HPB/Latest.md"), "3\n");

    // a page that is only a file is cloned too, and so are the files under it
    space("HFO", &[]);
    let loose = ok(
        "clone#WikiPage",
        &[
            ("baseWikiSpaceId", text("HSR")),
            ("wikiSpaceId", text("HFO")),
            ("pagePath", text("Doc/Loose")),
            ("parentPath", text("HFO")),
            ("withChildren", Value::Bool(true)),
        ],
    );
    assert_eq!(out_text(&loose, "pagePath"), "Loose");
    assert_eq!(location_text(&ctx, "dbresource://WikiSpace/HFO/Loose.md"), "loose\n");
    assert_eq!(location_text(&ctx, "dbresource://WikiSpace/HFO/Loose/Deep.md"), "deep\n");
}

#[test]
fn a_review_is_requested_decided_from_the_tray_and_cancelled() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    users(&facade);
    seed_review_data(&facade);
    login("alice");
    space("REV", &[]);
    save("REV", "Policy", "one\n", None);
    let page = save("REV", "Policy", "two\n", None);
    let requested = ok(
        "request#WikiPageReview",
        &[
            ("wikiSpaceId", text("REV")),
            ("pagePath", text("Policy")),
            ("reviewerUserId", text("bob")),
            ("note", text("Please check")),
            ("link", text("/qapps/Commons/Review")),
        ],
    );
    let review = out_text(&requested, "wikiPageReviewId");
    let row_of = one(
        &facade,
        "moqui.resource.wiki.WikiPageReview",
        &[("wikiPageReviewId", &review)],
    );
    assert_eq!(value_of(&row_of, "versionName"), "02");
    assert_eq!(value_of(&row_of, "statusId"), "WprRequested");
    let message_id = value_of(&row_of, "notificationMessageId");
    let message = one(
        &facade,
        "moqui.security.user.NotificationMessage",
        &[("notificationMessageId", &message_id)],
    );
    assert_eq!(value_of(&message, "topic"), "WikiPageReview");
    assert_eq!(
        value_of(&message, "recordEntityName"),
        "moqui.resource.wiki.WikiPageReview"
    );
    assert_eq!(value_of(&message, "recordId"), review);
    assert_eq!(value_of(&message, "reasonEnumId"), "NrApproval");
    assert_eq!(
        value_of(&message, "titleText"),
        "alice Example asked you to review Policy"
    );

    let mine = ok("find#WikiPageReviews", &[("filter", text("requested"))]);
    assert_eq!(
        column(&maps(&mine, "reviewList"), "wikiPageReviewId"),
        std::slice::from_ref(&review)
    );
    login("bob");
    let to_do = maps(&ok("find#WikiPageReviews", &[]), "reviewList");
    assert_eq!(column(&to_do, "pageName"), ["Policy"]);
    assert_eq!(field(&to_do[0], "canDecide"), "true");
    assert_eq!(field(&to_do[0], "requestedByName"), "alice Example");
    login("carol");
    refused(
        "decide#WikiPageReview",
        &[("wikiPageReviewId", text(&review)), ("decision", text("approve"))],
        "not yours",
    );

    // bob approves from the tray: the version is published, alice is told
    login("bob");
    let approved = call_named(
        "org.moqui.impl.NotificationServices.approve#NotificationMessage",
        &[("notificationMessageId", text(&message_id))],
    );
    assert_eq!(errors(), "", "{approved:?}");
    let row_of = one(
        &facade,
        "moqui.resource.wiki.WikiPageReview",
        &[("wikiPageReviewId", &review)],
    );
    assert_eq!(value_of(&row_of, "statusId"), "WprApproved");
    assert_eq!(value_of(&row_of, "decidedByUserId"), "bob");
    assert_eq!(
        value_of(
            &one(&facade, "moqui.resource.wiki.WikiPage", &[("wikiPageId", &page)]),
            "publishedVersionName"
        ),
        "02"
    );
    let told = {
        let _authz = ctx.disable_authz();
        facade
            .find("moqui.security.user.NotificationMessage")
            .unwrap()
            .eq("topic", text("WikiPageReviewDecision"))
            .list()
            .unwrap()
    };
    assert_eq!(told.len(), 1);
    assert_eq!(
        told.iter()
            .next()
            .and_then(|row| row.get("titleText").map(to_plain_string))
            .unwrap_or_default(),
        "bob Example approved Policy"
    );
    refused(
        "decide#WikiPageReview",
        &[("wikiPageReviewId", text(&review)), ("decision", text("approve"))],
        "approved already",
    );

    // a second version refused from the tray with a reason: changes requested, nothing published
    login("alice");
    save("REV", "Policy", "three\n", None);
    let second = out_text(
        &ok(
            "request#WikiPageReview",
            &[("wikiPageId", text(&page)), ("reviewerUserId", text("bob"))],
        ),
        "wikiPageReviewId",
    );
    let second_message = value_of(
        &one(
            &facade,
            "moqui.resource.wiki.WikiPageReview",
            &[("wikiPageReviewId", &second)],
        ),
        "notificationMessageId",
    );
    login("bob");
    let _ = call_named(
        "org.moqui.impl.NotificationServices.refuse#NotificationMessage",
        &[("notificationMessageId", text(&second_message))],
    );
    assert!(errors().contains("reason is required"));
    let _ = call_named(
        "org.moqui.impl.NotificationServices.refuse#NotificationMessage",
        &[
            ("notificationMessageId", text(&second_message)),
            ("reason", text("Fix the typo")),
        ],
    );
    assert_eq!(errors(), "");
    let row_of = one(
        &facade,
        "moqui.resource.wiki.WikiPageReview",
        &[("wikiPageReviewId", &second)],
    );
    assert_eq!(value_of(&row_of, "statusId"), "WprChangesRequested");
    assert_eq!(value_of(&row_of, "decisionNote"), "Fix the typo");
    assert_eq!(value_of(&row_of, "baseVersionName"), "02");
    assert_eq!(
        value_of(
            &one(&facade, "moqui.resource.wiki.WikiPage", &[("wikiPageId", &page)]),
            "publishedVersionName"
        ),
        "02"
    );

    // the requester cancels, and the reviewer's approval leaves the tray; a newer request replaces an open one
    // (the in-memory test database keeps a notification another transaction wrote only while the service's own
    // transaction leaves that table alone, so the cancel is checked before the replacing request)
    login("alice");
    let third = out_text(
        &ok(
            "request#WikiPageReview",
            &[("wikiPageId", text(&page)), ("reviewerUserId", text("bob"))],
        ),
        "wikiPageReviewId",
    );
    ok("cancel#WikiPageReview", &[("wikiPageReviewId", text(&third))]);
    let cancelled = one(
        &facade,
        "moqui.resource.wiki.WikiPageReview",
        &[("wikiPageReviewId", &third)],
    );
    assert_eq!(value_of(&cancelled, "statusId"), "WprCancelled");
    let waiting = one(
        &facade,
        "moqui.security.user.NotificationMessageUser",
        &[
            ("notificationMessageId", &value_of(&cancelled, "notificationMessageId")),
            ("userId", "bob"),
        ],
    );
    assert!(
        !value_of(&waiting, "viewedDate").is_empty(),
        "the approval is no longer waiting"
    );
    let fourth = out_text(
        &ok(
            "request#WikiPageReview",
            &[("wikiPageId", text(&page)), ("reviewerUserId", text("bob"))],
        ),
        "wikiPageReviewId",
    );
    let fifth = out_text(
        &ok(
            "request#WikiPageReview",
            &[("wikiPageId", text(&page)), ("reviewerUserId", text("bob"))],
        ),
        "wikiPageReviewId",
    );
    let replaced = one(
        &facade,
        "moqui.resource.wiki.WikiPageReview",
        &[("wikiPageReviewId", &fourth)],
    );
    assert_eq!(value_of(&replaced, "statusId"), "WprCancelled");
    assert_eq!(value_of(&replaced, "decisionNote"), "Replaced by a newer request");
    login("bob");
    assert_eq!(
        column(
            &maps(&ok("find#WikiPageReviews", &[]), "reviewList"),
            "wikiPageReviewId"
        ),
        std::slice::from_ref(&fifth)
    );
    login("carol");
    refused(
        "cancel#WikiPageReview",
        &[("wikiPageReviewId", text(&fifth))],
        "Only the one who asked",
    );
    login("alice");
    ok(
        "cancel#WikiPageReview",
        &[("wikiPageReviewId", text(&fifth)), ("note", text("Not now"))],
    );
    login("bob");
    assert!(maps(&ok("find#WikiPageReviews", &[]), "reviewList").is_empty());
    let all = maps(
        &ok(
            "find#WikiPageReviews",
            &[("filter", text("all")), ("statusId", text("any"))],
        ),
        "reviewList",
    );
    assert_eq!(all.len(), 5);

    // a reviewer who may not view the page is refused
    login("alice");
    space("PRV", &[("restrictView", text("Y"))]);
    save("PRV", "Private", "private\n", None);
    refused(
        "request#WikiPageReview",
        &[
            ("wikiSpaceId", text("PRV")),
            ("pagePath", text("Private")),
            ("reviewerUserId", text("carol")),
        ],
        "may not view this page",
    );
}

#[test]
fn search_and_page_lists_leave_out_what_a_user_may_not_view() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    users(&facade);
    login("alice");
    space("PUB", &[]);
    at(&ctx, 0);
    save("PUB", "Welcome Guide", "# The welcome guide\n\nHow we work.\n", None);
    let secret = save("PUB", "Welcome Secret", "# Secret\n", None);
    {
        let _authz = ctx.disable_authz();
        let mut page = facade
            .find("moqui.resource.wiki.WikiPage")
            .unwrap()
            .eq("wikiPageId", text(&secret))
            .one()
            .unwrap()
            .unwrap();
        page.set("restrictView", text("Y"));
        page.update(&facade).unwrap();
    }
    row(
        &facade,
        "moqui.resource.wiki.WikiPageUser",
        &[
            ("wikiPageId", text(&secret)),
            ("userId", text("alice")),
            ("allowView", text("Y")),
        ],
    );
    ok("ensure#UserNotesSpace", &[]);
    save("NOTES_alice", "Welcome note", "mine\n", None);

    let found = ok("search#WikiPages", &[("text", text("welcome"))]);
    assert_eq!(out_text(&found, "usedIndex"), "false");
    let mut paths = column(&maps(&found, "pageList"), "pagePath");
    paths.sort();
    assert_eq!(paths, ["Welcome Guide", "Welcome Secret", "Welcome note"]);
    login("bob");
    let found = ok("search#WikiPages", &[("text", text("welcome"))]);
    assert_eq!(column(&maps(&found, "pageList"), "pagePath"), ["Welcome Guide"]);
    // the text of the restricted page by its path (the public wiki REST endpoint): nothing for bob, the page for alice
    let page_text = || {
        let out = ok(
            "get#PublishedWikiPageText",
            &[
                ("wikiSpaceId", text("PUB")),
                ("pagePath", text("Welcome Secret")),
                ("versionName", text("01")),
            ],
        );
        out.get("pageText").cloned().unwrap_or(Value::Null)
    };
    assert_eq!(page_text(), Value::Null, "bob may not view the restricted page");
    login("alice");
    assert!(matches!(page_text(), Value::String(body) if body.contains("Secret")));
    login("bob");
    let pages = ok(
        "get#WikiSpacePages",
        &[("wikiSpaceId", text("PUB")), ("includeDetail", Value::Bool(true))],
    );
    let flat = maps(&pages, "allChildFileFlatList");
    assert_eq!(
        column(&flat, "path"),
        ["", "Welcome Guide"],
        "the restricted page is left out"
    );
    assert_eq!(field(&flat[1], "title"), "The welcome guide");
    assert_eq!(field(&flat[1], "snippet"), "How we work.");
    assert_eq!(field(&flat[1], "modifiedByUserId"), "alice");
    refused(
        "get#WikiSpacePages",
        &[("wikiSpaceId", text("NOTES_alice"))],
        "not found",
    );

    // the search document of a page carries its title and last change
    let guide = one(
        &facade,
        "moqui.resource.wiki.WikiPage",
        &[("wikiSpaceId", "PUB"), ("pagePath", "Welcome Guide")],
    );
    let mut document = IndexMap::new();
    document.insert("wikiPageId".to_string(), text(&value_of(&guide, "wikiPageId")));
    document.insert("wikiSpaceId".to_string(), text("PUB"));
    document.insert("pagePath".to_string(), text("Welcome Guide"));
    let out = call_named(
        &format!("{WIKI}.get#WikiPageManualDocumentData"),
        &[
            ("dataDocumentId", text("MantleWikiPage")),
            ("document", Value::Map(document)),
        ],
    );
    let Some(Value::Map(document)) = out.get("document") else {
        panic!("no document: {out:?}");
    };
    assert_eq!(field(document, "title"), "The welcome guide");
    assert_eq!(field(document, "pageName"), "Welcome Guide");
    assert_eq!(field(document, "updatedByUserId"), "alice");
    assert_eq!(field(document, "updatedDate"), BASE_TIME.to_string());
    assert_eq!(field(document, "restrictView"), "N");
    assert!(field(document, "content").contains("How we work."));
}

/// A member of a space as create#WikiSpace takes one in memberList.
fn member(user_id: &str, view: &str, update: &str, admin: &str) -> Value {
    let mut map = IndexMap::new();
    for (key, value) in [
        ("userId", user_id),
        ("allowView", view),
        ("allowUpdate", update),
        ("allowAdmin", admin),
    ] {
        map.insert(key.to_string(), text(value));
    }
    Value::Map(map)
}

fn set_field(facade: &EntityFacade, entity: &str, pk: (&str, &str), name: &str, value: Value) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut found = facade
        .find(entity)
        .unwrap()
        .eq(pk.0, text(pk.1))
        .one()
        .unwrap()
        .unwrap_or_else(|| panic!("no {entity} {pk:?}"));
    found.set(name, value);
    found.update(facade).unwrap();
}

fn location_text(ctx: &ExecutionContext, location: &str) -> String {
    ctx.factory().resource().location_text(location).unwrap_or_default()
}

/// A page is written and read at its exact path (parent path and page name), as move#WikiPage finds it: a new
/// top-level Travel does not write over Policies/Travel (findChildFile searches the tree for the name), a record at
/// the top-level path with no file yet reads no other page's text, and its attachments and delete are its own.
#[test]
fn a_page_is_written_and_read_at_its_exact_path_never_over_a_page_of_that_name_elsewhere() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    users(&facade);
    login("alice");
    space("EXA", &[]);
    save("EXA", "Policies", "# Policies\n", None);
    let policy = save("EXA", "Policies/Travel", "# Travel policy\n", None);
    ok(
        "set#PublishedVersion",
        &[("wikiPageId", text(&policy)), ("publishedVersionName", text("01"))],
    );
    let nested = "dbresource://WikiSpace/EXA/Policies/Travel.md";

    // a new top-level page of that name is its own file; the nested page keeps its text
    let top = save("EXA", "Travel", "# Travel at the top\n", None);
    assert_ne!(top, policy);
    assert_eq!(location_text(&ctx, nested), "# Travel policy\n", "not written over");
    assert_eq!(
        location_text(&ctx, "dbresource://WikiSpace/EXA/Travel.md"),
        "# Travel at the top\n"
    );

    // a record at a top-level path with no file yet (get#WikiPageId makes one): no other page's text or file
    save("EXA", "Policies/Lodging", "# Lodging policy\n", None);
    let record = out_text(
        &ok(
            "get#WikiPageId",
            &[
                ("wikiSpaceId", text("EXA")),
                ("pagePath", text("Lodging")),
                ("createIfMissing", Value::Bool(true)),
            ],
        ),
        "wikiPageId",
    );
    set_field(
        &facade,
        "moqui.resource.wiki.WikiPage",
        ("wikiPageId", &record),
        "publishedVersionName",
        text("01"),
    );
    let published = ok(
        "get#PublishedWikiPageText",
        &[("wikiSpaceId", text("EXA")), ("pagePath", text("Lodging"))],
    );
    assert_eq!(
        published.get("pageText").cloned().unwrap_or(Value::Null),
        Value::Null,
        "not the text of Policies/Lodging"
    );
    let info = ok(
        "get#WikiPageInfo",
        &[("wikiSpaceId", text("EXA")), ("pagePath", text("Lodging"))],
    );
    assert_eq!(out_text(&info, "pageLocation"), "dbresource://WikiSpace/EXA/Lodging");
    assert_eq!(
        save("EXA", "Lodging", "# Lodging\n", None),
        record,
        "the page takes the record at its path"
    );
    assert_eq!(
        location_text(&ctx, "dbresource://WikiSpace/EXA/Policies/Lodging.md"),
        "# Lodging policy\n"
    );
    let info = ok(
        "get#WikiPageInfo",
        &[("wikiSpaceId", text("EXA")), ("pagePath", text("Travel"))],
    );
    assert_eq!(out_text(&info, "pageLocation"), "dbresource://WikiSpace/EXA/Travel.md");
    assert_eq!(out_text(&info, "parentPath"), "");
    let history = maps(
        &ok("get#WikiPageHistory", &[("wikiPageId", text(&policy))]),
        "versionList",
    );
    assert_eq!(history.len(), 1, "the nested page has its one version");

    // the attachments of a page by its id are its own
    let attachment = ok(
        "get#WikiPageAttachment",
        &[("wikiPageId", text(&top)), ("filename", text("map.png"))],
    );
    let Some(Value::Map(reference)) = attachment.get("attachmentReference") else {
        panic!("no attachment reference: {attachment:?}");
    };
    assert_eq!(
        field(reference, "location"),
        "dbresource://WikiSpace/EXA/Travel/_attachments/map.png"
    );
    refused(
        "get#WikiPageAttachment",
        &[("wikiPageId", text(&top)), ("filename", text("../Policies/Travel.md"))],
        "is a name, not a path",
    );

    // deleting the top-level page leaves the nested one; a path out of the space is no page
    ok(
        "delete#WikiPage",
        &[("wikiSpaceId", text("EXA")), ("pagePath", text("Travel"))],
    );
    assert!(!exists(&ctx, "dbresource://WikiSpace/EXA/Travel.md"));
    assert_eq!(location_text(&ctx, nested), "# Travel policy\n");
    refused(
        "update#WikiPage",
        &[
            ("wikiSpaceId", text("EXA")),
            ("parentPath", text("..")),
            ("pageName", text("Escape")),
            ("pageText", text("out")),
        ],
        "no . or .. in it",
    );
    refused(
        "get#WikiPageInfo",
        &[("wikiSpaceId", text("EXA")), ("pagePath", text("../EXA"))],
        "no . or .. in it",
    );
}

/// The writes ask get#WikiPageAccess: update#WikiPage, delete#WikiPage, set#PublishedVersion and the attachment
/// services refuse a user who may not update (a viewer, a restricted page the user may not view, a read-only mirror),
/// and the reads (get#WikiPageInfo and through it the history) let the space's administrators and the ADMIN group see
/// what they may; internal work with no user and authorization disabled still does everything.
#[test]
fn wiki_writes_and_reads_follow_the_access_rule_with_its_admin_overrides() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    users(&facade);
    for id in ["dave", "erin"] {
        row(
            &facade,
            "moqui.security.UserAccount",
            &[("userId", text(id)), ("username", text(id))],
        );
    }
    row(
        &facade,
        "moqui.security.UserGroup",
        &[("userGroupId", text("ADMIN")), ("description", text("Administrators"))],
    );
    row(
        &facade,
        "moqui.security.UserGroupMember",
        &[
            ("userGroupId", text("ADMIN")),
            ("userId", text("dave")),
            ("fromDate", Value::I64(0)),
        ],
    );
    login("alice");
    // bob views; erin administers the space without view or update rows; carol and dave are not members
    space(
        "ACC",
        &[
            ("restrictView", text("Y")),
            ("restrictUpdate", text("Y")),
            (
                "memberList",
                Value::List(vec![member("bob", "Y", "N", "N"), member("erin", "N", "N", "Y")]),
            ),
        ],
    );
    let plan = save("ACC", "Plan", "# Plan\n", None);
    let hidden = save("ACC", "Hidden", "# Hidden\n", None);
    set_field(
        &facade,
        "moqui.resource.wiki.WikiPage",
        ("wikiPageId", &hidden),
        "restrictView",
        text("Y"),
    );
    row(
        &facade,
        "moqui.resource.wiki.WikiPageUser",
        &[
            ("wikiPageId", text(&hidden)),
            ("userId", text("alice")),
            ("allowView", text("Y")),
        ],
    );
    ctx.factory()
        .resource()
        .write_bytes("dbresource://WikiSpace/ACC/Plan/_attachments/a.txt", b"attached")
        .unwrap();
    let rewrite = |page: &str, name: &str, body: &str| -> Vec<(&'static str, Value)> {
        vec![
            ("wikiSpaceId", text("ACC")),
            ("wikiPageId", text(page)),
            ("pageName", text(name)),
            ("pageText", text(body)),
        ]
    };
    let plan_attachment = [("wikiPageId", text(&plan)), ("filename", text("a.txt"))];

    // a viewer reads, and changes nothing
    login("bob");
    ok("get#WikiPageInfo", &[("wikiPageId", text(&plan))]);
    refused(
        "update#WikiPage",
        &rewrite(&plan, "Plan", "bob's\n"),
        "You may not change this page",
    );
    refused(
        "update#WikiPage",
        &[
            ("wikiSpaceId", text("ACC")),
            ("pageName", text("New")),
            ("pageText", text("new\n")),
        ],
        "You may not add pages to this space",
    );
    assert!(one(
        &facade,
        "moqui.resource.wiki.WikiPage",
        &[("wikiSpaceId", "ACC"), ("pagePath", "New")]
    )
    .is_none());
    refused(
        "set#PublishedVersion",
        &[("wikiPageId", text(&plan)), ("publishedVersionName", text("01"))],
        "may not publish",
    );
    refused("delete#WikiPage", &[("wikiPageId", text(&plan))], "may not delete");
    ok("get#WikiPageAttachment", &plan_attachment);
    refused(
        "delete#WikiPageAttachment",
        &plan_attachment,
        "may not change the files of this page",
    );
    assert!(exists(&ctx, "dbresource://WikiSpace/ACC/Plan/_attachments/a.txt"));
    // a restricted page the viewer may not view is not theirs to read or write: each refusal is "Page not found", with
    // no path or name of the page, whatever else would refuse it (its child page here)
    login("alice");
    save("ACC", "Hidden/Inner", "# Inner\n", None);
    login("bob");
    let not_found = |name: &str, parameters: &[(&str, Value)]| {
        let _ = call_named(&format!("{WIKI}.{name}"), parameters);
        assert_eq!(errors(), "Page not found", "{name}");
    };
    let hidden_id = [("wikiPageId", text(&hidden))];
    for name in [
        "get#WikiPageInfo",
        "get#WikiPageHistory",
        "delete#WikiPage",
        "get#WikiPageBacklinks",
        "get#WikiPageLinks",
    ] {
        not_found(name, &hidden_id);
    }
    not_found("update#WikiPage", &rewrite(&hidden, "Hidden", "bob's\n"));
    not_found(
        "move#WikiPage",
        &[("wikiPageId", text(&hidden)), ("toPageName", text("Shown"))],
    );
    not_found(
        "set#PublishedVersion",
        &[("wikiPageId", text(&hidden)), ("publishedVersionName", text("01"))],
    );
    not_found(
        "restore#WikiPageVersion",
        &[("wikiPageId", text(&hidden)), ("versionName", text("01"))],
    );
    not_found(
        "get#WikiPageAttachment",
        &[("wikiPageId", text(&hidden)), ("filename", text("a.txt"))],
    );
    not_found(
        "delete#WikiPage",
        &[("wikiSpaceId", text("ACC")), ("pagePath", text("Hidden"))],
    );
    refused(
        "update#WikiPage",
        &[
            ("wikiSpaceId", text("ACC")),
            ("parentPath", text("Hidden")),
            ("pageName", text("Under")),
            ("pageText", text("x\n")),
        ],
        "Parent page not found",
    );
    // a user who may view it gets the reason, in plain words
    login("alice");
    refused(
        "delete#WikiPage",
        &hidden_id,
        "This page has pages under it: move or delete them first",
    );
    ok(
        "delete#WikiPage",
        &[("wikiSpaceId", text("ACC")), ("pagePath", text("Hidden/Inner"))],
    );
    login("bob");
    assert_eq!(
        location_text(&ctx, "dbresource://WikiSpace/ACC/Hidden.md"),
        "# Hidden\n"
    );

    // not a member of the restricted space: nothing
    login("carol");
    refused("get#WikiPageInfo", &[("wikiPageId", text(&plan))], "Page not found");
    refused("get#WikiPageAttachment", &plan_attachment, "not found");
    refused("update#WikiPage", &rewrite(&plan, "Plan", "carol's\n"), "not found");

    // the space's administrator views and updates its pages, but a restricted page only as its member
    login("erin");
    ok("get#WikiPageInfo", &[("wikiPageId", text(&plan))]);
    ok("update#WikiPage", &rewrite(&plan, "Plan", "# Plan\n\nBy erin.\n"));
    refused("get#WikiPageInfo", &[("wikiPageId", text(&hidden))], "Page not found");

    // the ADMIN group sees and changes everything
    login("dave");
    ok("get#WikiPageInfo", &[("wikiPageId", text(&hidden))]);
    let history = maps(
        &ok("get#WikiPageHistory", &[("wikiPageId", text(&hidden))]),
        "versionList",
    );
    assert_eq!(history.len(), 1);
    ok("update#WikiPage", &rewrite(&hidden, "Hidden", "# Hidden\n\nBy dave.\n"));
    let published = ok(
        "set#PublishedVersion",
        &[("wikiPageId", text(&plan)), ("publishedVersionName", text("02"))],
    );
    assert_eq!(out_text(&published, "publishedVersionName"), "02");
    refused(
        "set#PublishedVersion",
        &[("wikiPageId", text(&plan)), ("publishedVersionName", text("99"))],
        "This page has no version 99",
    );
    ok("delete#WikiPageAttachment", &plan_attachment);
    assert!(!exists(&ctx, "dbresource://WikiSpace/ACC/Plan/_attachments/a.txt"));

    // a mirror takes no writes, whoever asks
    row(
        &facade,
        "moqui.resource.GitRepository",
        &[
            ("repositoryId", text("WIKI_MIR")),
            ("repositoryKind", text("remote")),
            ("remoteUrl", text("https://example.com/mirror.git")),
        ],
    );
    row(
        &facade,
        "moqui.resource.wiki.WikiSpace",
        &[
            ("wikiSpaceId", text("MIR")),
            ("rootPageLocation", text("git://WIKI_MIR/MIR.md")),
        ],
    );
    refused(
        "update#WikiPage",
        &[
            ("wikiSpaceId", text("MIR")),
            ("pageName", text("Page")),
            ("pageText", text("x\n")),
        ],
        "read-only mirror",
    );

    // internal work: no user, authorization disabled
    {
        let _authz = ctx.disable_authz();
        crate::user::logout().unwrap();
        ok(
            "update#WikiPage",
            &rewrite(&hidden, "Hidden", "# Hidden\n\nInternal.\n"),
        );
        ok(
            "set#PublishedVersion",
            &[("wikiPageId", text(&hidden)), ("publishedVersionName", text("03"))],
        );
        ok("delete#WikiPage", &[("wikiPageId", text(&plan))]);
    }
    assert!(!exists(&ctx, "dbresource://WikiSpace/ACC/Plan.md"));
}

/// delete#WikiPage by its id takes the framework rows of the page (aliases, history, its links, reviews; the links to
/// it stay, to a page not there), and a page record without a file goes too; a decision from the tray tells the
/// requester with the request's link, so their notification opens the page.
#[test]
fn a_page_is_deleted_with_its_rows_and_a_decision_opens_the_requests_link() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    users(&facade);
    seed_review_data(&facade);
    login("alice");
    space("DEL", &[]);
    let alpha = save("DEL", "Alpha", "# Alpha, see [beta](Beta)\n", None);
    let beta = save("DEL", "Beta", "# Beta, see [alpha](Alpha)\n", None);
    ok(
        "update#WikiPage",
        &[
            ("wikiSpaceId", text("DEL")),
            ("wikiPageId", text(&alpha)),
            ("pageName", text("First")),
            ("pageText", text("# Alpha, see [beta](Beta)\n")),
        ],
    );
    let link = "/qapps/Commons/Knowledge?wikiPageId=First&review=open";
    let requested = ok(
        "request#WikiPageReview",
        &[
            ("wikiPageId", text(&alpha)),
            ("reviewerUserId", text("bob")),
            ("link", text(link)),
        ],
    );
    let review = out_text(&requested, "wikiPageReviewId");
    let message_id = value_of(
        &one(
            &facade,
            "moqui.resource.wiki.WikiPageReview",
            &[("wikiPageReviewId", &review)],
        ),
        "notificationMessageId",
    );
    login("bob");
    let approved = call_named(
        "org.moqui.impl.NotificationServices.approve#NotificationMessage",
        &[("notificationMessageId", text(&message_id))],
    );
    assert_eq!(errors(), "", "{approved:?}");
    let told = {
        let _authz = ctx.disable_authz();
        facade
            .find("moqui.security.user.NotificationMessage")
            .unwrap()
            .eq("topic", text("WikiPageReviewDecision"))
            .list()
            .unwrap()
    };
    assert_eq!(told.len(), 1);
    assert_eq!(
        told.iter()
            .next()
            .and_then(|row| row.get("linkText").map(to_plain_string))
            .unwrap_or_default(),
        link
    );

    // the page by its id, with its rows
    login("alice");
    ok("delete#WikiPage", &[("wikiPageId", text(&alpha))]);
    assert!(!exists(&ctx, "dbresource://WikiSpace/DEL/First.md"));
    for entity in [
        "moqui.resource.wiki.WikiPage",
        "moqui.resource.wiki.WikiPageHistory",
        "moqui.resource.wiki.WikiPageLink",
        "moqui.resource.wiki.WikiPageReview",
    ] {
        let _authz = ctx.disable_authz();
        let rows = facade
            .find(entity)
            .unwrap()
            .eq("wikiPageId", text(&alpha))
            .list()
            .unwrap();
        assert!(rows.is_empty(), "{entity}");
    }
    assert!(one(
        &facade,
        "moqui.resource.wiki.WikiPageAlias",
        &[("wikiSpaceId", "DEL"), ("aliasPath", "Alpha")]
    )
    .is_none());
    let links = maps(&ok("get#WikiPageLinks", &[("wikiPageId", text(&beta))]), "linkList");
    assert_eq!(column(&links, "found"), ["false"]);

    // a page record without a file; the root page is never deleted
    let ghost = out_text(
        &ok(
            "get#WikiPageId",
            &[
                ("wikiSpaceId", text("DEL")),
                ("pagePath", text("Ghost")),
                ("createIfMissing", Value::Bool(true)),
            ],
        ),
        "wikiPageId",
    );
    ok("delete#WikiPage", &[("wikiPageId", text(&ghost))]);
    assert!(one(&facade, "moqui.resource.wiki.WikiPage", &[("wikiPageId", &ghost)]).is_none());
    refused(
        "delete#WikiPage",
        &[("wikiSpaceId", text("DEL")), ("pagePath", text(""))],
        "The home page of a space is not deleted",
    );
}
