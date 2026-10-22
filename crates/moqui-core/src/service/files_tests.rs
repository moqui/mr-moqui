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

//! `org.moqui.impl.FileServices` over the resource facade, on both storages: who reaches a drive and with what
//! permission, the personal drive on first use, uploads as one commit on git and as versions on the database tree,
//! version notes, change sets, the git path index (modified by, history across a move), trash and restore, shares and
//! public links (password, expiry, revoke, opened count, the upload-only drop), quotas, a records drive's folder map,
//! image, PDF and spreadsheet previews, the search documents, and their extracted text.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use moqui_lib::object::{to_plain_string, HostObject, Value};

use crate::context::{ExecutionContext, ExecutionContextFactory, ResourceChange};
use crate::entity::EntityFacade;
use crate::web::multipart::UploadedFile;

const SERVICES: &str = "org.moqui.impl.FileServices";

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
    let paths = vec![
        framework.join("impl/FileServices.xml"),
        framework.join("impl/GitServices.xml"),
    ];
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

fn user(facade: &EntityFacade, user_id: &str) {
    row(
        facade,
        "moqui.security.UserAccount",
        &[
            ("userId", text(user_id)),
            ("username", text(user_id)),
            ("userFullName", text(&format!("{user_id} Example"))),
            ("partyId", text(&format!("P_{user_id}"))),
        ],
    );
}

fn users(facade: &EntityFacade) {
    for id in ["alice", "bob", "carol", "dave", "eve"] {
        user(facade, id);
    }
}

fn login(user_id: &str) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    crate::user::internal_login_user(user_id).unwrap();
}

fn logout() {
    crate::user::logout().unwrap();
}

fn call(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    crate::service::sync(&format!("{SERVICES}.{name}"), &parameters).unwrap_or_else(|err| panic!("{name}: {err}"))
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
    let out = call(name, parameters);
    let errors = errors();
    assert!(errors.is_empty(), "{name}: {errors}");
    out
}

fn refused(name: &str, parameters: &[(&str, Value)], expected: &str) {
    let _ = call(name, parameters);
    let errors = errors();
    assert!(
        errors.contains(expected),
        "{name}: expected an error with {expected:?}, got {errors:?}"
    );
}

fn out_text(out: &BTreeMap<String, Value>, key: &str) -> String {
    out.get(key).map(to_plain_string).unwrap_or_default()
}

fn number(value: Option<&Value>) -> i64 {
    value
        .map(to_plain_string)
        .and_then(|text| text.parse().ok())
        .unwrap_or(-1)
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

fn flag(map: &IndexMap<String, Value>, key: &str) -> bool {
    matches!(map.get(key), Some(Value::Bool(true)))
}

fn upload(name: &str, bytes: &[u8]) -> Value {
    Value::Object(HostObject::new(UploadedFile::in_memory(
        "file",
        name,
        "application/octet-stream",
        bytes.to_vec(),
    )))
}

fn files(items: &[(&str, &str)]) -> Value {
    Value::List(items.iter().map(|(name, body)| upload(name, body.as_bytes())).collect())
}

fn names(out: &BTreeMap<String, Value>) -> Vec<String> {
    maps(out, "fileList").iter().map(|row| field(row, "name")).collect()
}

/// A drive of alice's, made through create#Drive with `storage`; its id.
fn drive(storage: &str, name: &str, extra: &[(&str, Value)]) -> String {
    let mut parameters = vec![("driveName", text(name)), ("storage", text(storage))];
    parameters.extend(extra.iter().cloned());
    out_text(&ok("create#Drive", &parameters), "driveId")
}

fn level(drive_id: &str, path: Option<&str>) -> i64 {
    let mut parameters = vec![("driveId", text(drive_id))];
    if let Some(path) = path {
        parameters.push(("path", text(path)));
    }
    number(ok("get#DriveAccess", &parameters).get("level"))
}

fn read(location: &str) -> String {
    ExecutionContext::with_current(|ec| ec.factory().resource().get(location).unwrap().read_text().unwrap()).unwrap()
}

#[test]
fn a_drive_is_reached_by_its_owner_and_members_by_permission_and_not_by_a_stranger() {
    let (context, facade) = setup();
    let _entered = context.enter();
    install();
    users(&facade);
    row(
        &facade,
        "moqui.security.UserGroup",
        &[("userGroupId", text("TEAM")), ("description", text("Team"))],
    );
    row(
        &facade,
        "moqui.security.UserGroupMember",
        &[
            ("userGroupId", text("TEAM")),
            ("userId", text("eve")),
            ("fromDate", Value::I64(0)),
        ],
    );
    login("alice");
    let team = drive(
        "folder",
        "Team",
        &[(
            "memberList",
            Value::List(vec![Value::Map(IndexMap::from([
                ("userId".to_string(), text("bob")),
                ("permissionEnumId".to_string(), text("RdpEdit")),
            ]))]),
        )],
    );
    ok(
        "add#DriveMember",
        &[("driveId", text(&team)), ("userId", text("carol"))],
    );
    ok(
        "add#DriveMember",
        &[
            ("driveId", text(&team)),
            ("userGroupId", text("TEAM")),
            ("permissionEnumId", text("RdpView")),
        ],
    );
    assert_eq!(level(&team, None), 3, "the owner manages");
    ok(
        "upload#Resource",
        &[("driveId", text(&team)), ("file", upload("plan.txt", b"plan"))],
    );

    login("bob");
    let access = ok("get#DriveAccess", &[("driveId", text(&team))]);
    assert_eq!(number(access.get("level")), 2);
    assert_eq!(access.get("canWrite"), Some(&Value::Bool(true)));
    ok(
        "upload#Resource",
        &[("driveId", text(&team)), ("file", upload("bob.txt", b"bob"))],
    );
    refused(
        "add#DriveMember",
        &[("driveId", text(&team)), ("userId", text("dave"))],
        "You may not change this drive",
    );

    login("carol");
    assert_eq!(level(&team, None), 1);
    assert_eq!(
        names(&ok("list#Folder", &[("driveId", text(&team))])),
        ["bob.txt", "plan.txt"]
    );
    refused(
        "upload#Resource",
        &[("driveId", text(&team)), ("file", upload("c.txt", b"c"))],
        "You may not upload",
    );
    refused(
        "trash#Resource",
        &[("driveId", text(&team)), ("path", text("plan.txt"))],
        "",
    );

    login("eve");
    assert_eq!(level(&team, None), 1, "a member through a user group");

    login("dave");
    assert_eq!(level(&team, None), 0);
    refused("list#Folder", &[("driveId", text(&team))], "Folder not found");
    refused(
        "get#Versions",
        &[("driveId", text(&team)), ("path", text("plan.txt"))],
        "File not found",
    );
    let listed: Vec<String> = maps(&ok("get#UserDrives", &[]), "driveList")
        .iter()
        .map(|drive| field(drive, "driveId"))
        .collect();
    assert!(!listed.contains(&team), "a stranger's list leaves the drive out");
    refused(
        "get#DriveAccess",
        &[("driveId", text(&team)), ("userId", text("alice"))],
        "not yours to see",
    );

    // a member removed reaches it no more
    login("alice");
    ok(
        "remove#DriveMember",
        &[("driveId", text(&team)), ("userId", text("carol"))],
    );
    login("carol");
    assert_eq!(level(&team, None), 0);
}

#[test]
fn the_personal_drive_is_made_on_first_use_and_listed_first() {
    let (context, facade) = setup();
    let _entered = context.enter();
    install();
    users(&facade);
    login("alice");
    let first = ok("get#UserDrives", &[]);
    let personal = out_text(&first, "personalDriveId");
    assert!(!personal.is_empty());
    let drives = maps(&first, "driveList");
    assert_eq!(field(&drives[0], "driveId"), personal);
    assert_eq!(field(&drives[0], "rootLocation"), "dbresource://drive/personal/alice");
    assert_eq!(field(&drives[0], "driveKindEnumId"), "RdkPersonal");
    assert_eq!(
        drives[0].get("usedBytes"),
        Some(&Value::Null),
        "a drive is sized only when asked"
    );
    let sized = maps(
        &ok("get#UserDrives", &[("includeUsage", Value::Bool(true))]),
        "driveList",
    );
    assert_eq!(number(sized[0].get("usedBytes")), 0);
    let again = ok("ensure#PersonalDrive", &[]);
    assert_eq!(out_text(&again, "driveId"), personal);
    assert_eq!(again.get("created"), Some(&Value::Bool(false)));
    refused(
        "add#DriveMember",
        &[("driveId", text(&personal)), ("userId", text("bob"))],
        "A personal drive has no members",
    );
    refused("archive#Drive", &[("driveId", text(&personal))], "cannot be archived");
    ok(
        "upload#Resource",
        &[("driveId", text(&personal)), ("file", upload("a.txt", b"12345"))],
    );
    let used = maps(
        &ok("get#UserDrives", &[("includeUsage", Value::Bool(true))]),
        "driveList",
    );
    assert_eq!(number(used[0].get("usedBytes")), 5);
    let usage = maps(&ok("get#DriveUsage", &[("driveId", text(&personal))]), "usageList");
    assert_eq!(number(usage[0].get("usedBytes")), 5);
    assert_eq!(usage[0].get("usedPercent"), Some(&Value::Null), "no quota");
    login("bob");
    assert_ne!(out_text(&ok("ensure#PersonalDrive", &[]), "driveId"), personal);
    refused("ensure#PersonalDrive", &[("userId", text("alice"))], "not yours");
    refused("get#DriveUsage", &[("driveId", text(&personal))], "Drive not found");
}

#[test]
fn an_upload_to_a_git_drive_is_one_commit_with_its_version_note() {
    let (context, facade) = setup();
    let _entered = context.enter();
    install();
    users(&facade);
    login("alice");
    let docs = drive("gitDb", "Docs", &[]);
    assert_eq!(
        out_text(
            &ok("get#DriveAccess", &[("driveId", text(&docs))]).get("drive").map_or(
                BTreeMap::new(),
                |drive| match drive {
                    Value::Map(map) => map.iter().map(|(key, value)| (key.clone(), value.clone())).collect(),
                    _ => BTreeMap::new(),
                }
            ),
            "storage"
        ),
        "git-db"
    );
    let uploaded = ok(
        "upload#Resource",
        &[
            ("driveId", text(&docs)),
            ("path", text("specs")),
            (
                "fileList",
                files(&[("a.md", "alpha\n"), ("b.md", "beta\n"), ("c.md", "gamma\n")]),
            ),
            ("versionNote", text("First drafts")),
        ],
    );
    let commit = out_text(&uploaded, "versionName");
    assert_eq!(commit.len(), 40, "one commit: {uploaded:?}");
    let outcomes: Vec<String> = maps(&uploaded, "uploadedList")
        .iter()
        .map(|row| field(row, "outcome"))
        .collect();
    assert_eq!(outcomes, ["created", "created", "created"]);
    let resources = context.factory().resource();
    let log = resources.change_log(&format!("git://DRIVE_{docs}/"), 10).unwrap();
    assert_eq!(log.len(), 1, "the three files are one commit");
    assert_eq!(log[0].version.note.as_deref(), Some("First drafts"));
    assert_eq!(log[0].paths, ["specs/a.md", "specs/b.md", "specs/c.md"]);

    ok(
        "add#DriveMember",
        &[
            ("driveId", text(&docs)),
            ("userId", text("bob")),
            ("permissionEnumId", text("RdpEdit")),
        ],
    );
    login("bob");
    ok(
        "upload#Resource",
        &[
            ("driveId", text(&docs)),
            ("path", text("specs")),
            ("file", upload("b.md", b"beta two\n")),
            ("versionNote", text("Bob's edit")),
        ],
    );
    let listed = ok("list#Folder", &[("driveId", text(&docs)), ("path", text("specs"))]);
    let rows = maps(&listed, "fileList");
    let by_name = |name: &str| rows.iter().find(|row| field(row, "name") == name).unwrap().clone();
    assert_eq!(field(&by_name("a.md"), "modifiedByUserId"), "alice");
    assert_eq!(field(&by_name("a.md"), "versionName"), commit);
    assert_eq!(field(&by_name("a.md"), "versionNote"), "First drafts");
    assert_eq!(
        field(&by_name("b.md"), "modifiedByUserId"),
        "bob",
        "the path index: b.md's own commit"
    );
    assert_eq!(field(&by_name("b.md"), "modifiedByName"), "bob Example");
    assert_eq!(number(by_name("b.md").get("size")), 9);
    let versions = maps(
        &ok(
            "get#Versions",
            &[("driveId", text(&docs)), ("path", text("specs/b.md"))],
        ),
        "versionList",
    );
    let notes: Vec<String> = versions.iter().map(|version| field(version, "note")).collect();
    assert_eq!(notes, ["Bob's edit", "First drafts"]);
    assert_eq!(field(&versions[0], "shortName").len(), 10);
    let diff = ok(
        "get#VersionDiff",
        &[
            ("driveId", text(&docs)),
            ("path", text("specs/b.md")),
            ("fromVersion", text(&field(&versions[1], "versionName"))),
        ],
    );
    assert!(
        to_plain_string(diff.get("diff").unwrap()).contains("moqui-diff"),
        "{diff:?}"
    );
    // restore writes the old version as the newest commit
    let restored = ok(
        "restore#Version",
        &[
            ("driveId", text(&docs)),
            ("path", text("specs/b.md")),
            ("versionName", text(&field(&versions[1], "versionName"))),
        ],
    );
    assert_eq!(out_text(&restored, "newVersionName").len(), 40);
    assert_eq!(read(&format!("git://DRIVE_{docs}/specs/b.md")), "beta\n");
    let versions = maps(
        &ok(
            "get#Versions",
            &[("driveId", text(&docs)), ("path", text("specs/b.md"))],
        ),
        "versionList",
    );
    assert_eq!(versions.len(), 3);
    assert!(field(&versions[0], "note").starts_with("Restored version "));
}

#[test]
fn uploads_to_a_folder_drive_keep_versions_with_notes_and_resolve_conflicts() {
    let (context, facade) = setup();
    let _entered = context.enter();
    install();
    users(&facade);
    login("alice");
    let team = drive("folder", "Team", &[]);
    ok(
        "upload#Resource",
        &[
            ("driveId", text(&team)),
            ("file", upload("a.txt", b"one\n")),
            ("versionNote", text("first")),
        ],
    );
    let replaced = ok(
        "upload#Resource",
        &[
            ("driveId", text(&team)),
            ("file", upload("a.txt", b"two\n")),
            ("versionNote", text("second")),
        ],
    );
    assert_eq!(field(&maps(&replaced, "uploadedList")[0], "outcome"), "replaced");
    let both = ok(
        "upload#Resource",
        &[
            ("driveId", text(&team)),
            ("file", upload("a.txt", b"three\n")),
            ("conflictPolicy", text("keepBoth")),
        ],
    );
    assert_eq!(field(&maps(&both, "uploadedList")[0], "path"), "a (2).txt");
    let skipped = ok(
        "upload#Resource",
        &[
            ("driveId", text(&team)),
            ("file", upload("a.txt", b"four\n")),
            ("conflictPolicy", text("skip")),
        ],
    );
    assert_eq!(field(&maps(&skipped, "uploadedList")[0], "outcome"), "skipped");
    let root = format!("dbresource://drive/shared/{team}");
    assert_eq!(read(&format!("{root}/a.txt")), "two\n");
    let versions = maps(
        &ok("get#Versions", &[("driveId", text(&team)), ("path", text("a.txt"))]),
        "versionList",
    );
    let summary: Vec<(String, String)> = versions
        .iter()
        .map(|v| (field(v, "versionName"), field(v, "note")))
        .collect();
    assert_eq!(
        summary,
        [
            ("02".to_string(), "second".to_string()),
            ("01".to_string(), "first".to_string())
        ]
    );
    // a folder upload with relative paths, and a bad name, in one call
    let folder = ok(
        "upload#Resource",
        &[
            ("driveId", text(&team)),
            ("fileList", files(&[("x.txt", "x"), ("y.txt", "y"), ("z.txt", "z")])),
            (
                "relativePathList",
                Value::List(vec![text("photos/x.txt"), text("photos/2026/y.txt"), text("../z.txt")]),
            ),
        ],
    );
    let outcomes: Vec<String> = maps(&folder, "uploadedList")
        .iter()
        .map(|row| field(row, "outcome"))
        .collect();
    assert_eq!(outcomes, ["created", "created", "failed"], "{folder:?}");
    assert_eq!(read(&format!("{root}/photos/2026/y.txt")), "y");
    let listed = names(&ok("list#Folder", &[("driveId", text(&team))]));
    assert_eq!(listed, ["photos", "a (2).txt", "a.txt"], "folders first");
    let restored = ok(
        "restore#Version",
        &[
            ("driveId", text(&team)),
            ("path", text("a.txt")),
            ("versionName", text("01")),
        ],
    );
    assert_eq!(out_text(&restored, "newVersionName"), "03");
    assert_eq!(read(&format!("{root}/a.txt")), "one\n");
}

#[test]
fn a_change_set_is_one_commit_on_git_and_the_plain_changes_on_the_database_tree() {
    let (context, facade) = setup();
    let _entered = context.enter();
    users(&facade);
    login("alice");
    row(
        &facade,
        "moqui.resource.GitRepository",
        &[("repositoryId", text("cs")), ("repositoryKind", text("db"))],
    );
    let resources = context.factory().resource();
    let write = |path: &str, body: &str| ResourceChange::Write {
        path: path.to_string(),
        bytes: body.as_bytes().to_vec(),
    };
    resources
        .apply_change_set(
            "git://cs/",
            &[
                write("dir/one.txt", "1"),
                write("dir/two.txt", "2"),
                write("keep.txt", "k"),
            ],
            Some("Start"),
        )
        .unwrap();
    let outcome = resources
        .apply_change_set(
            "git://cs/",
            &[
                write("new.txt", "n"),
                ResourceChange::Move {
                    from: "keep.txt".to_string(),
                    to: "kept/keep.txt".to_string(),
                },
                ResourceChange::Move {
                    from: "missing.txt".to_string(),
                    to: "x.txt".to_string(),
                },
                ResourceChange::Delete {
                    path: "dir".to_string(),
                },
                ResourceChange::MakeDirectory {
                    path: "empty".to_string(),
                },
                ResourceChange::Delete {
                    path: "../escape".to_string(),
                },
            ],
            Some("Tidy up"),
        )
        .unwrap();
    assert_eq!(outcome.applied, ["new.txt", "kept/keep.txt", "dir", "empty"]);
    let failed: Vec<&str> = outcome.failures.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(failed, ["x.txt", "../escape"]);
    let log = resources.change_log("git://cs/", 10).unwrap();
    assert_eq!(log.len(), 2, "each change set is one commit");
    assert_eq!(log[0].version.note.as_deref(), Some("Tidy up"));
    assert_eq!(log[0].version.version_name, outcome.version_name.clone().unwrap());
    let names: Vec<String> = resources
        .list("git://cs/")
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(names, ["empty", "kept", "new.txt"], "the folder delete took its files");
    // the database tree: each change, the folder delete with the files under it, the message as the version note
    resources
        .apply_change_set(
            "dbresource://cs",
            &[write("dir/one.txt", "1"), write("dir/sub/two.txt", "2")],
            Some("Up"),
        )
        .unwrap();
    assert_eq!(
        resources
            .current_version("dbresource://cs/dir/one.txt")
            .unwrap()
            .unwrap()
            .note
            .as_deref(),
        Some("Up")
    );
    let outcome = resources
        .apply_change_set(
            "dbresource://cs",
            &[
                ResourceChange::Move {
                    from: "dir/one.txt".to_string(),
                    to: "one.txt".to_string(),
                },
                ResourceChange::Delete {
                    path: "dir".to_string(),
                },
            ],
            None,
        )
        .unwrap();
    assert_eq!(outcome.applied, ["one.txt", "dir"]);
    assert!(resources.stat("dbresource://cs/dir/sub/two.txt").unwrap().is_none());
    assert!(resources.stat("dbresource://cs/dir").unwrap().is_none());
    assert_eq!(read("dbresource://cs/one.txt"), "1");
    let histories = facade
        .find("moqui.resource.DbResourceFileHistory")
        .unwrap()
        .list()
        .unwrap();
    assert_eq!(
        histories.len(),
        1,
        "the deleted file's history went with it; the moved one kept its own"
    );
}

#[test]
fn version_notes_are_kept_on_both_storages_and_read_back_from_the_script_api() {
    let (context, facade) = setup();
    let _entered = context.enter();
    users(&facade);
    login("alice");
    row(
        &facade,
        "moqui.resource.GitRepository",
        &[("repositoryId", text("vn")), ("repositoryKind", text("db"))],
    );
    let resources = context.factory().resource();
    resources
        .write_bytes_noted("git://vn/a.md", b"one", Some("First"))
        .unwrap();
    resources.write_bytes_noted("git://vn/a.md", b"two", None).unwrap();
    let notes: Vec<Option<String>> = resources
        .version_history("git://vn/a.md")
        .unwrap()
        .into_iter()
        .map(|version| version.note)
        .collect();
    assert_eq!(notes, [Some("Update a.md".to_string()), Some("First".to_string())]);
    resources
        .write_bytes_noted("dbresource://vn/a.md", b"one", Some("First"))
        .unwrap();
    resources
        .write_bytes_noted("dbresource://vn/a.md", b"two", Some("Second"))
        .unwrap();
    resources.write_bytes("dbresource://vn/a.md", b"three").unwrap();
    let notes: Vec<Option<String>> = resources
        .version_history("dbresource://vn/a.md")
        .unwrap()
        .into_iter()
        .map(|version| version.note)
        .collect();
    assert_eq!(notes, [None, Some("Second".to_string()), Some("First".to_string())]);
    let file = facade
        .find("moqui.resource.DbResourceFile")
        .unwrap()
        .list()
        .unwrap()
        .into_values()
        .pop()
        .unwrap();
    assert_eq!(number(file.get("fileSize")), 5);
    assert_eq!(resources.get("dbresource://vn/a.md").unwrap().size(), Some(5));
    // a file over the blob threshold is a content blob, and its notes stay with its versions as an inline file's do
    let (big_one, big_two) = (vec![7u8; 70_000], vec![8u8; 70_001]);
    resources
        .write_bytes_noted("dbresource://vn/big.bin", &big_one, Some("Big one"))
        .unwrap();
    resources
        .write_bytes_noted("dbresource://vn/big.bin", &big_two, Some("Big two"))
        .unwrap();
    let versions = resources.version_history("dbresource://vn/big.bin").unwrap();
    let notes: Vec<Option<String>> = versions.iter().map(|version| version.note.clone()).collect();
    assert_eq!(notes, [Some("Big two".to_string()), Some("Big one".to_string())]);
    assert_eq!(
        resources
            .read_version("dbresource://vn/big.bin", &versions[1].version_name)
            .unwrap(),
        Some(big_one)
    );
    assert_eq!(resources.get("dbresource://vn/big.bin").unwrap().size(), Some(70_001));
    let blobs = facade
        .find("moqui.resource.DbResourceFile")
        .unwrap()
        .list()
        .unwrap()
        .into_values()
        .into_iter()
        .filter(|row| row.get("blobId").is_some_and(|id| !id.is_null()))
        .count();
    assert_eq!(blobs, 1);
    let out = crate::script::eval_value(
        r#"(() => {
        const ref = ec.resource.getLocationReference("dbresource://vn/b.md")
        ref.putText("bee", "A note")
        const copy = ec.resource.getLocationReference("dbresource://vn/e.md")
        copy.putStream(ref.openStream(), "Copied")
        const copied = copy.getText() + ":" + copy.getVersionHistory()[0].note
        ec.resource.getLocationReference("git://vn/b.md").putText("bee", "Git note")
        const cs = ec.resource.makeChangeSet("git://vn").write("c.md", "sea").write("d.md", new Uint8Array([100, 101])).commit("Two files")
        const versions = ec.resource.getLocationReference("dbresource://vn/b.md").getVersionHistory()
        const git = ec.resource.getLocationReference("git://vn/b.md").getCurrentVersion()
        const children = ec.resource.getChildChanges("git://vn")
        return [versions[0].note, versions[0].getNote(), git.note, cs.applied.length, cs.versionName.length,
            children.find((child) => child.name === "c.md").note, ec.resource.getLastChange("git://vn/d.md").size, copied].join("|")
        })()"#,
    )
    .unwrap();
    assert_eq!(
        to_plain_string(&out),
        "A note|A note|Git note|2|40|Two files|2|bee:Copied"
    );
}

#[test]
fn the_path_index_gives_each_file_its_own_last_change_and_history_across_a_move() {
    let (context, facade) = setup();
    let _entered = context.enter();
    users(&facade);
    row(
        &facade,
        "moqui.resource.GitRepository",
        &[("repositoryId", text("pi")), ("repositoryKind", text("db"))],
    );
    let resources = context.factory().resource();
    login("alice");
    resources
        .write_bytes_noted("git://pi/a.txt", b"a1", Some("a one"))
        .unwrap();
    login("bob");
    resources
        .write_bytes_noted("git://pi/b.txt", b"b1", Some("b one"))
        .unwrap();
    login("alice");
    resources
        .write_bytes_noted("git://pi/a.txt", b"a2", Some("a two"))
        .unwrap();
    let changes = resources.child_changes("git://pi/").unwrap();
    let users: Vec<(String, Option<String>, Option<String>)> = changes
        .iter()
        .map(|(name, change)| (name.clone(), change.user_id.clone(), change.note.clone()))
        .collect();
    assert_eq!(
        users,
        [
            (
                "a.txt".to_string(),
                Some("alice".to_string()),
                Some("a two".to_string())
            ),
            ("b.txt".to_string(), Some("bob".to_string()), Some("b one".to_string())),
        ]
    );
    let b_commit = changes[1].1.version_name.clone().unwrap();
    assert_ne!(
        Some(b_commit.clone()),
        changes[0].1.version_name,
        "not the head commit for every file"
    );
    login("carol");
    resources.rename("git://pi/a.txt", "git://pi/docs/a-moved.txt").unwrap();
    let history = resources.version_history("git://pi/docs/a-moved.txt").unwrap();
    let notes: Vec<Option<String>> = history.iter().map(|version| version.note.clone()).collect();
    assert_eq!(
        notes,
        [
            Some("Move a.txt to docs/a-moved.txt".to_string()),
            Some("a two".to_string()),
            Some("a one".to_string())
        ]
    );
    let oldest = &history[2].version_name;
    assert_eq!(
        resources.read_version("git://pi/docs/a-moved.txt", oldest).unwrap(),
        Some(b"a1".to_vec())
    );
    assert_eq!(
        resources
            .version_text("git://pi/docs/a-moved.txt", &oldest[..8])
            .unwrap()
            .as_deref(),
        Some("a1")
    );
    let moved = resources.last_change("git://pi/docs/a-moved.txt").unwrap().unwrap();
    assert_eq!(moved.user_id.as_deref(), Some("carol"));
    assert_eq!(
        resources.last_change("git://pi/b.txt").unwrap().unwrap().version_name,
        Some(b_commit)
    );
    let folder = resources.last_change("git://pi/docs").unwrap().unwrap();
    assert!(!folder.is_file);
    let log = resources.change_log("git://pi/docs", 10).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].paths, ["a-moved.txt"]);
    // the stat of a file is its own commit's time; the head moved on and the index followed
    login("dave");
    resources.write_bytes("git://pi/c.txt", b"c").unwrap();
    assert_eq!(
        resources
            .last_change("git://pi/c.txt")
            .unwrap()
            .unwrap()
            .user_id
            .as_deref(),
        Some("dave")
    );
    assert_eq!(
        resources
            .last_change("git://pi/b.txt")
            .unwrap()
            .unwrap()
            .user_id
            .as_deref(),
        Some("bob")
    );
}

#[test]
fn trash_and_restore_on_the_database_tree_and_on_git() {
    let (context, facade) = setup();
    let _entered = context.enter();
    install();
    users(&facade);
    login("alice");
    for storage in ["folder", "gitDb"] {
        let id = drive(storage, &format!("Trash {storage}"), &[]);
        ok(
            "upload#Resource",
            &[
                ("driveId", text(&id)),
                ("fileList", files(&[("a.txt", "one"), ("keep.txt", "k")])),
                ("versionNote", text("v1")),
            ],
        );
        ok(
            "upload#Resource",
            &[
                ("driveId", text(&id)),
                ("file", upload("a.txt", b"two")),
                ("versionNote", text("v2")),
            ],
        );
        ok(
            "upload#Resource",
            &[
                ("driveId", text(&id)),
                ("path", text("box")),
                ("fileList", files(&[("x.txt", "x"), ("y.txt", "y")])),
            ],
        );
        let trashed = ok(
            "trash#Resource",
            &[
                ("driveId", text(&id)),
                ("pathList", Value::List(vec![text("a.txt"), text("box")])),
            ],
        );
        let trash_ids: Vec<String> = maps(&trashed, "trashList")
            .iter()
            .map(|row| field(row, "trashId"))
            .collect();
        assert_eq!(trash_ids.len(), 2, "{storage}: {trashed:?}");
        assert_eq!(
            names(&ok("list#Folder", &[("driveId", text(&id))])),
            ["keep.txt"],
            "{storage}: the trash folder is hidden"
        );
        let in_trash = maps(&ok("list#Trash", &[("driveId", text(&id))]), "trashList");
        assert_eq!(in_trash.len(), 2);
        refused(
            "get#Versions",
            &[("driveId", text(&id)), ("path", text("nothing/../a.txt"))],
            "File not found",
        );
        ok("restore#Resource", &[("trashId", text(&trash_ids[0]))]);
        let versions = maps(
            &ok("get#Versions", &[("driveId", text(&id)), ("path", text("a.txt"))]),
            "versionList",
        );
        if storage == "folder" {
            let notes: Vec<String> = versions.iter().map(|version| field(version, "note")).collect();
            assert_eq!(notes, ["v2", "v1"], "the history moved with the file");
        } else {
            assert_eq!(field(&versions[0], "note"), "Restore a.txt from the trash");
            assert!(
                versions.len() >= 3,
                "the older commits stay in the history: {versions:?}"
            );
        }
        let root = if storage == "folder" {
            format!("dbresource://drive/shared/{id}")
        } else {
            format!("git://DRIVE_{id}")
        };
        assert_eq!(read(&format!("{root}/a.txt")), "two");
        ok("restore#Resource", &[("trashId", text(&trash_ids[1]))]);
        assert_eq!(
            read(&format!("{root}/box/y.txt")),
            "y",
            "{storage}: a folder comes back with its files"
        );
        // trash again and empty it
        ok("trash#Resource", &[("driveId", text(&id)), ("path", text("box"))]);
        ok(
            "upload#Resource",
            &[("driveId", text(&id)), ("file", upload("box", b"file now"))],
        );
        let blocked = maps(&ok("list#Trash", &[("driveId", text(&id))]), "trashList");
        refused(
            "restore#Resource",
            &[("trashId", text(&field(&blocked[0], "trashId")))],
            "Something else is at that path",
        );
        let emptied = ok("empty#Trash", &[("driveId", text(&id))]);
        assert_eq!(number(emptied.get("deletedCount")), 1);
        assert!(maps(&ok("list#Trash", &[("driveId", text(&id))]), "trashList").is_empty());
        if storage == "folder" {
            let trash = format!("{root}/.trash");
            if context.factory().resource().stat(&trash).unwrap().is_some() {
                assert!(
                    context.factory().resource().list(&trash).unwrap().is_empty(),
                    "the trash copies are gone"
                );
            }
        }
    }
}

#[test]
fn shares_and_public_links_with_password_expiry_revoke_and_a_file_drop() {
    let (context, facade) = setup();
    let _entered = context.enter();
    install();
    users(&facade);
    login("alice");
    let team = drive("folder", "Team", &[]);
    ok(
        "upload#Resource",
        &[
            ("driveId", text(&team)),
            ("path", text("pub")),
            ("fileList", files(&[("a.txt", "A"), ("b.txt", "B")])),
        ],
    );
    ok(
        "upload#Resource",
        &[("driveId", text(&team)), ("file", upload("secret.txt", b"S"))],
    );
    ok("create#Folder", &[("driveId", text(&team)), ("name", text("drop"))]);
    // a share with dave of one folder
    let shared = ok(
        "share#Resource",
        &[
            ("driveId", text(&team)),
            ("path", text("pub")),
            ("sharedWithUserId", text("dave")),
        ],
    );
    let share_id = out_text(&shared, "shareId");
    login("dave");
    assert_eq!(level(&team, Some("pub")), 1);
    assert_eq!(
        level(&team, Some("pub/a.txt")),
        1,
        "a share covers what is under its folder"
    );
    assert_eq!(level(&team, Some("secret.txt")), 0);
    assert_eq!(
        names(&ok("list#Folder", &[("driveId", text(&team)), ("path", text("pub"))])),
        ["a.txt", "b.txt"]
    );
    refused("list#Folder", &[("driveId", text(&team))], "Folder not found");
    refused(
        "list#Folder",
        &[("driveId", text(&team)), ("path", text("pub/../"))],
        "Folder not found",
    );
    let incoming = maps(&ok("get#Shares", &[]), "incomingList");
    assert_eq!(incoming.len(), 1);
    assert!(flag(&incoming[0], "pending"));
    ok("accept#Share", &[("shareId", text(&share_id))]);
    ok("decline#Share", &[("shareId", text(&share_id))]);
    assert_eq!(level(&team, Some("pub")), 0, "a declined share grants nothing");
    login("bob");
    refused("accept#Share", &[("shareId", text(&share_id))], "Share not found");

    // a link with a password
    login("alice");
    let link = ok(
        "share#Resource",
        &[
            ("driveId", text(&team)),
            ("path", text("pub")),
            ("isLink", text("Y")),
            ("password", text("open sesame")),
            ("note", text("For you")),
        ],
    );
    let token = out_text(&link, "linkToken");
    assert_eq!(out_text(&link, "linkPath"), format!("/s/{token}"));
    let stored = facade
        .find("moqui.resource.ResourceShare")
        .unwrap()
        .eq("shareId", text(&out_text(&link, "shareId")))
        .one()
        .unwrap()
        .unwrap();
    assert_ne!(
        stored.get("linkTokenHash").map(to_plain_string).unwrap_or_default(),
        token,
        "only the hash is kept"
    );
    assert!(!to_plain_string(stored.get("passwordHash").unwrap()).contains("open sesame"));
    let links = maps(&ok("get#Shares", &[]), "linkList");
    assert_eq!(links.len(), 1);
    assert!(flag(&links[0], "hasPassword"));
    assert!(!links[0].contains_key("linkToken"));
    {
        logout();
        let check = ok("check#PublicShareToken", &[("token", text(&token))]);
        assert_eq!(check.get("valid"), Some(&Value::Bool(true)));
        assert!(to_plain_string(check.get("routeContext").unwrap()).contains("For you"));
        assert_eq!(
            ok("check#PublicShareToken", &[("token", text("nonsense-token-0000000"))]).get("valid"),
            Some(&Value::Bool(false))
        );
        let locked = ok("open#PublicShare", &[("token", text(&token))]);
        assert_eq!(locked.get("passwordOk"), Some(&Value::Bool(false)));
        refused(
            "open#PublicShare",
            &[("token", text(&token)), ("password", text("wrong"))],
            "The password is not right",
        );
        refused("list#PublicShare", &[("token", text(&token))], "Enter the password");
        let opened = ok(
            "open#PublicShare",
            &[("token", text(&token)), ("password", text("open sesame"))],
        );
        let grant = out_text(&opened, "grant");
        let listed = ok("list#PublicShare", &[("token", text(&token)), ("grant", text(&grant))]);
        assert_eq!(names(&listed), ["a.txt", "b.txt"]);
        refused(
            "list#PublicShare",
            &[("token", text(&token)), ("grant", text(&grant)), ("path", text(".."))],
            "Not found",
        );
        let file = ok(
            "download#PublicShare",
            &[
                ("token", text(&token)),
                ("grant", text(&grant)),
                ("path", text("a.txt")),
            ],
        );
        assert_eq!(read(&out_text(&file, "location")), "A");
        let zip = ok(
            "download#PublicShare",
            &[("token", text(&token)), ("grant", text(&grant))],
        );
        assert_eq!(out_text(&zip, "contentType"), "application/zip");
        let _ = std::fs::remove_file(out_text(&zip, "location").replace("runtime://", "./"));
        refused(
            "upload#PublicShare",
            &[
                ("token", text(&token)),
                ("grant", text(&grant)),
                ("file", upload("x.txt", b"x")),
            ],
            "This link takes no files",
        );
        let parameters = BTreeMap::from([("driveId".to_string(), text(&team)), ("path".to_string(), text("pub"))]);
        let anonymous = crate::service::sync(&format!("{SERVICES}.list#Folder"), &parameters);
        assert!(
            anonymous.is_err() || !errors().is_empty(),
            "the drive services need a user"
        );
    }
    login("alice");
    let links = maps(&ok("get#Shares", &[]), "linkList");
    assert_eq!(number(links[0].get("openedCount")), 1);
    ok("revoke#Share", &[("shareId", text(&out_text(&link, "shareId")))]);
    assert_eq!(
        ok("check#PublicShareToken", &[("token", text(&token))]).get("valid"),
        Some(&Value::Bool(false))
    );
    refused(
        "open#PublicShare",
        &[("token", text(&token)), ("password", text("open sesame"))],
        "expired or was not found",
    );

    // an expired link, and an upload-only file drop
    let expired = ok(
        "share#Resource",
        &[
            ("driveId", text(&team)),
            ("path", text("pub/a.txt")),
            ("isLink", text("Y")),
            ("expireDate", Value::I64(crate::user::now_millis() - 60_000)),
        ],
    );
    assert_eq!(
        ok(
            "check#PublicShareToken",
            &[("token", text(&out_text(&expired, "linkToken")))]
        )
        .get("valid"),
        Some(&Value::Bool(false))
    );
    refused(
        "share#Resource",
        &[
            ("driveId", text(&team)),
            ("path", text("secret.txt")),
            ("isLink", text("Y")),
            ("permissionEnumId", text("RspUpload")),
        ],
        "Upload only is for a folder",
    );
    let drop = ok(
        "share#Resource",
        &[
            ("driveId", text(&team)),
            ("path", text("drop")),
            ("isLink", text("Y")),
            ("permissionEnumId", text("RspUpload")),
        ],
    );
    let drop_token = out_text(&drop, "linkToken");
    {
        logout();
        let opened = ok("open#PublicShare", &[("token", text(&drop_token))]);
        let grant = out_text(&opened, "grant");
        let listed = ok(
            "list#PublicShare",
            &[("token", text(&drop_token)), ("grant", text(&grant))],
        );
        assert_eq!(listed.get("uploadOnly"), Some(&Value::Bool(true)));
        assert!(
            maps(&listed, "fileList").is_empty(),
            "an upload-only link lists nothing"
        );
        let dropped = ok(
            "upload#PublicShare",
            &[
                ("token", text(&drop_token)),
                ("fileList", files(&[("report.txt", "r1"), ("report.txt", "r2")])),
                ("uploaderName", text("Zed")),
            ],
        );
        let paths: Vec<String> = maps(&dropped, "uploadedList")
            .iter()
            .map(|row| field(row, "path"))
            .collect();
        assert_eq!(paths, ["report.txt", "report (2).txt"], "never replaces");
        refused(
            "download#PublicShare",
            &[("token", text(&drop_token)), ("path", text("report.txt"))],
            "Not found",
        );
    }
    login("alice");
    let versions = maps(
        &ok(
            "get#Versions",
            &[("driveId", text(&team)), ("path", text("drop/report.txt"))],
        ),
        "versionList",
    );
    assert_eq!(field(&versions[0], "note"), "Uploaded by Zed through a link");
    let activity = maps(
        &ok("get#Activity", &[("driveId", text(&team)), ("path", text("pub"))]),
        "activityList",
    );
    let actions: Vec<String> = activity.iter().map(|row| field(row, "action")).collect();
    assert!(
        actions.contains(&"share".to_string())
            && actions.contains(&"link".to_string())
            && actions.contains(&"revoke".to_string()),
        "{actions:?}"
    );
    assert!(actions.contains(&"version".to_string()), "{actions:?}");
}

/// The drive's hidden trash folder is not reached by its path: a public link to the whole drive (no share path) lists,
/// downloads and uploads nothing under `.trash`, a member or a share reads it only through list#Trash, nothing is uploaded
/// into it or shared from it, and its owner still opens it.
#[test]
fn the_trash_is_not_reached_through_a_link_to_the_whole_drive_or_by_its_path() {
    let (context, facade) = setup();
    let _entered = context.enter();
    install();
    users(&facade);
    login("alice");
    let bin = drive("folder", "Bin", &[]);
    ok(
        "upload#Resource",
        &[
            ("driveId", text(&bin)),
            ("fileList", files(&[("a.txt", "A"), ("secret.txt", "S")])),
        ],
    );
    let trashed = ok(
        "trash#Resource",
        &[("driveId", text(&bin)), ("path", text("secret.txt"))],
    );
    let trash_id = field(&maps(&trashed, "trashList")[0], "trashId");
    let in_trash = format!(".trash/{trash_id}/secret.txt");
    assert_eq!(
        read(&format!("dbresource://drive/shared/{bin}/{in_trash}")),
        "S",
        "the trash copy is where the paths below look"
    );
    ok("add#DriveMember", &[("driveId", text(&bin)), ("userId", text("bob"))]);
    let link = ok(
        "share#Resource",
        &[
            ("driveId", text(&bin)),
            ("isLink", text("Y")),
            ("permissionEnumId", text("RspEdit")),
        ],
    );
    let token = out_text(&link, "linkToken");
    refused(
        "share#Resource",
        &[
            ("driveId", text(&bin)),
            ("path", text(&in_trash)),
            ("isLink", text("Y")),
        ],
        "File not found",
    );
    refused(
        "upload#Resource",
        &[
            ("driveId", text(&bin)),
            ("path", text(".trash")),
            ("file", upload("x.txt", b"x")),
        ],
        "Folder not found",
    );
    // the owner opens the trash folder by its path
    assert_eq!(
        names(&ok("list#Folder", &[("driveId", text(&bin)), ("path", text(".trash"))])),
        [trash_id.as_str()]
    );

    logout();
    assert_eq!(names(&ok("list#PublicShare", &[("token", text(&token))])), ["a.txt"]);
    for path in [
        ".trash",
        ".trash/",
        &format!(".trash/{trash_id}"),
        &format!("./.trash/{trash_id}"),
    ] {
        refused(
            "list#PublicShare",
            &[("token", text(&token)), ("path", text(path))],
            "Not found",
        );
    }
    refused(
        "download#PublicShare",
        &[("token", text(&token)), ("path", text(&in_trash))],
        "Not found",
    );
    refused(
        "download#PublicShare",
        &[("token", text(&token)), ("path", text(".trash"))],
        "Not found",
    );
    refused(
        "upload#PublicShare",
        &[
            ("token", text(&token)),
            ("path", text(".trash")),
            ("file", upload("drop.txt", b"d")),
        ],
        "Not found",
    );
    let file = ok(
        "download#PublicShare",
        &[("token", text(&token)), ("path", text("a.txt"))],
    );
    assert_eq!(read(&out_text(&file, "location")), "A");

    // a member sees the trash through list#Trash, not by its path
    login("bob");
    assert_eq!(
        maps(&ok("list#Trash", &[("driveId", text(&bin))]), "trashList").len(),
        1
    );
    refused(
        "list#Folder",
        &[("driveId", text(&bin)), ("path", text(".trash"))],
        "Folder not found",
    );
    refused(
        "get#ResourceInfo",
        &[("driveId", text(&bin)), ("path", text(&in_trash))],
        "File not found",
    );
    let found = ok(
        "find#Resources",
        &[
            ("driveId", text(&bin)),
            ("path", text(".trash")),
            ("text", text("secret")),
        ],
    );
    assert!(maps(&found, "resultList").is_empty(), "{found:?}");
    let everywhere = ok("find#Resources", &[("text", text("secret"))]);
    assert!(maps(&everywhere, "resultList").is_empty(), "{everywhere:?}");
}

#[test]
fn an_upload_past_the_quota_is_refused_whole() {
    let (context, facade) = setup();
    let _entered = context.enter();
    install();
    users(&facade);
    login("alice");
    let small = drive("folder", "Small", &[("quotaBytes", Value::I64(10))]);
    ok(
        "upload#Resource",
        &[("driveId", text(&small)), ("file", upload("a.txt", b"12345"))],
    );
    refused(
        "upload#Resource",
        &[
            ("driveId", text(&small)),
            ("fileList", files(&[("b.txt", "123"), ("c.txt", "456")])),
        ],
        "would pass the quota of drive",
    );
    assert_eq!(
        names(&ok("list#Folder", &[("driveId", text(&small))])),
        ["a.txt"],
        "nothing of the upload was written"
    );
    // replacing a file counts only the difference
    ok(
        "upload#Resource",
        &[("driveId", text(&small)), ("file", upload("a.txt", b"1234567890"))],
    );
    let git = drive("gitDb", "Small git", &[("quotaBytes", Value::I64(4))]);
    refused(
        "upload#Resource",
        &[("driveId", text(&git)), ("file", upload("a.txt", b"12345"))],
        "quota",
    );
}

/// The stub of the record visibility hook (a record whose id starts with OPEN is visible), counting its calls in the
/// execution context's shared map so a test sees that each record is asked once per request.
const RECORD_HOOK: &str = r#"<services>
    <service verb="check" noun="Visible">
        <in-parameters><parameter name="recordEntityName"/><parameter name="recordId"/><parameter name="checkOnly" type="Boolean"/></in-parameters>
        <out-parameters><parameter name="recordVisible" type="Boolean"/></out-parameters>
        <actions><script><![CDATA[
            ec.context.sharedMap['test.hookCalls'] = Number(ec.context.sharedMap['test.hookCalls'] || 0) + 1
            recordVisible = checkOnly === true && String(recordId).startsWith('OPEN')
        ]]></script></actions>
    </service>
</services>"#;

/// The framework's file services with the chat services (the record visibility hook) and the stub hook.
fn install_with_record_hook() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-files-records-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("service/test")).unwrap();
    std::fs::write(dir.join("service/test/DriveStub.xml"), RECORD_HOOK).unwrap();
    let framework = crate::context::framework_dir().join("service/org/moqui");
    let paths = vec![
        framework.join("impl/FileServices.xml"),
        framework.join("impl/GitServices.xml"),
        framework.join("impl/ChatServices.xml"),
        dir.join("service/test/DriveStub.xml"),
    ];
    let failures = crate::service::lint(&paths).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    dir
}

fn hook_calls(context: &ExecutionContext) -> i64 {
    number(context.shared_map().get("test.hookCalls")).max(0)
}

#[test]
fn a_records_drive_shows_its_folder_map_and_only_the_records_a_user_may_see() {
    let (context, facade) = setup();
    let _entered = context.enter();
    let dir = install_with_record_hook();
    users(&facade);
    let records = {
        let _authz = context.disable_authz();
        login("alice");
        out_text(
            &ok(
                "create#Drive",
                &[
                    ("driveName", text("Records")),
                    ("driveKindEnumId", text("RdkRecords")),
                    ("rootLocation", text("dbresource://mantle/content")),
                    (
                        "memberList",
                        Value::List(vec![
                            Value::Map(IndexMap::from([("userGroupId".to_string(), text("ALL_USERS"))])),
                            Value::Map(IndexMap::from([
                                ("userId".to_string(), text("eve")),
                                ("permissionEnumId".to_string(), text("RdpEdit")),
                            ])),
                        ]),
                    ),
                ],
            ),
            "driveId",
        )
    };
    for (path, entity, id_field, label, sequence) in [
        ("party", "mantle.party.Party", "partyId", "Parties", 1),
        (
            "CommunicationEvent",
            "mantle.party.communication.CommunicationEvent",
            "communicationEventId",
            "Messages",
            2,
        ),
        ("asset", "mantle.product.asset.Asset", "assetId", "Assets", 3),
        (
            "asset/*/maintenance",
            "mantle.product.asset.AssetMaintenance",
            "assetMaintenanceId",
            "Asset maintenance",
            4,
        ),
    ] {
        row(
            &facade,
            "moqui.resource.ResourceDriveRecordFolder",
            &[
                ("driveId", text(&records)),
                ("folderPath", text(path)),
                ("recordEntityName", text(entity)),
                ("idField", text(id_field)),
                ("label", text(label)),
                ("sequenceNum", Value::I64(sequence)),
            ],
        );
    }
    let resources = context.factory().resource();
    for (path, body) in [
        ("party/OPEN1/content_1/contract.pdf", "%PDF open"),
        ("party/SECRET1/content_2/secret.pdf", "%PDF secret"),
        ("asset/OPENA/maintenance/OPENM/content_3/photo.png", "png"),
        ("asset/OPENA/maintenance/SECRETM/content_4/hidden.png", "png"),
        ("asset/SECRETA/maintenance/OPENM2/content_5/inside.png", "png"),
        ("stray/unmapped.txt", "not in the map"),
    ] {
        resources
            .write_bytes(&format!("dbresource://mantle/content/{path}"), body.as_bytes())
            .unwrap();
    }
    let list = |path: &str| names(&ok("list#Folder", &[("driveId", text(&records)), ("path", text(path))]));
    let info = |path: &str| ok("get#ResourceInfo", &[("driveId", text(&records)), ("path", text(path))]);
    let not_found = |path: &str| {
        refused(
            "get#ResourceInfo",
            &[("driveId", text(&records)), ("path", text(path))],
            "File not found",
        )
    };

    // no hook in the theme: a member reaches no record folder (only the map itself), a manager every path
    login("carol");
    assert_eq!(
        list(""),
        ["Parties", "Messages", "Assets"],
        "a pattern folder is not at the top"
    );
    assert!(list("party").is_empty());
    not_found("party/OPEN1/content_1/contract.pdf");
    login("alice");
    assert_eq!(list("party"), ["OPEN1", "SECRET1"]);
    // a manager's list of a mapped folder is limited to the newest recordLimit record folders too, the rest counted
    let managed = ok(
        "list#Folder",
        &[
            ("driveId", text(&records)),
            ("path", text("party")),
            ("recordLimit", Value::I64(1)),
        ],
    );
    assert_eq!(number(managed.get("moreRecordCount")), 1, "{managed:?}");
    assert_eq!(maps(&managed, "fileList").len(), 1, "{managed:?}");
    assert_eq!(list("stray"), ["unmapped.txt"]);
    let secret = info("party/SECRET1/content_2/secret.pdf");
    let records_of = |out: &BTreeMap<String, Value>| -> Vec<String> {
        match out.get("resource") {
            Some(Value::Map(resource)) => match resource.get("recordList") {
                Some(Value::List(items)) => items
                    .iter()
                    .map(|item| match item {
                        Value::Map(map) => format!("{} {}", field(map, "recordEntityName"), field(map, "recordId")),
                        _ => String::new(),
                    })
                    .collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    };
    assert_eq!(records_of(&secret), ["mantle.party.Party SECRET1"]);

    // the theme names the hook: a member sees the records it answers for, each record asked once per request
    row(
        &facade,
        "moqui.screen.ScreenThemeResource",
        &[
            ("screenThemeId", text("DEFAULT_QUASAR")),
            ("sequenceNum", Value::I64(990)),
            ("resourceTypeEnumId", text("STRT_RECORD_VISIBLE_SERVICE")),
            ("resourceValue", text("test.DriveStub.check#Visible")),
        ],
    );
    login("bob");
    let before = hook_calls(&context);
    let parties = ok("list#Folder", &[("driveId", text(&records)), ("path", text("party"))]);
    assert_eq!(names(&parties), ["OPEN1"]);
    assert_eq!(field(&maps(&parties, "fileList")[0], "recordId"), "OPEN1");
    assert_eq!(hook_calls(&context) - before, 2, "OPEN1 and SECRET1 asked");
    let contract = info("party/OPEN1/content_1/contract.pdf");
    assert_eq!(records_of(&contract), ["mantle.party.Party OPEN1"]);
    assert_eq!(
        hook_calls(&context) - before,
        2,
        "a record already asked in this request is not asked again"
    );
    not_found("party/SECRET1/content_2/secret.pdf");
    refused(
        "list#Folder",
        &[("driveId", text(&records)), ("path", text("party/SECRET1"))],
        "Folder not found",
    );
    refused(
        "list#Folder",
        &[("driveId", text(&records)), ("path", text("stray"))],
        "Folder not found",
    );
    not_found("stray/unmapped.txt");
    assert_eq!(
        number(ok("get#DriveAccess", &[("driveId", text(&records))]).get("level")),
        1
    );
    assert_eq!(level(&records, Some("party/SECRET1")), 0);
    assert_eq!(level(&records, Some("party/OPEN1")), 1);
    // a record inside another: both must be visible
    assert_eq!(list("asset/OPENA/maintenance"), ["OPENM"]);
    not_found("asset/OPENA/maintenance/SECRETM/content_4/hidden.png");
    not_found("asset/SECRETA/maintenance/OPENM2/content_5/inside.png");
    let nested = info("asset/OPENA/maintenance/OPENM/content_3/photo.png");
    assert_eq!(
        records_of(&nested),
        [
            "mantle.product.asset.AssetMaintenance OPENM",
            "mantle.product.asset.Asset OPENA"
        ],
        "innermost first"
    );
    // a mapped folder's record folders: the newest recordLimit are asked, the rest counted
    let limited = ok(
        "list#Folder",
        &[
            ("driveId", text(&records)),
            ("path", text("party")),
            ("recordLimit", Value::I64(1)),
        ],
    );
    assert_eq!(number(limited.get("moreRecordCount")), 1);
    assert!(maps(&limited, "fileList").len() <= 1);
    // search (a walk here), the zip, the trash, favorites
    let found = ok("find#Resources", &[("text", text("pdf"))]);
    let paths: Vec<String> = maps(&found, "resultList")
        .iter()
        .map(|row| field(row, "path"))
        .collect();
    assert_eq!(paths, ["party/OPEN1/content_1/contract.pdf"]);
    let zip = ok("download#Zip", &[("driveId", text(&records)), ("path", text("asset"))]);
    assert_eq!(number(zip.get("fileCount")), 1, "only OPENA's OPENM photo");
    refused(
        "download#Zip",
        &[("driveId", text(&records)), ("path", text("party/SECRET1"))],
        "File not found",
    );
    row(
        &facade,
        "moqui.resource.ResourceFavorite",
        &[
            ("userId", text("bob")),
            ("driveId", text(&records)),
            ("path", text("party/SECRET1/content_2/secret.pdf")),
        ],
    );
    ok(
        "set#Favorite",
        &[
            ("driveId", text(&records)),
            ("path", text("party/OPEN1/content_1/contract.pdf")),
        ],
    );
    let favorites: Vec<String> = maps(&ok("get#Favorites", &[]), "favoriteList")
        .iter()
        .map(|row| field(row, "path"))
        .collect();
    assert_eq!(favorites, ["party/OPEN1/content_1/contract.pdf"]);
    login("alice");
    // a share does not pass the gate
    ok(
        "share#Resource",
        &[
            ("driveId", text(&records)),
            ("path", text("party/SECRET1")),
            ("sharedWithUserId", text("bob")),
        ],
    );
    ok(
        "trash#Resource",
        &[
            ("driveId", text(&records)),
            ("path", text("party/SECRET1/content_2/secret.pdf")),
        ],
    );
    assert_eq!(
        maps(&ok("list#Trash", &[("driveId", text(&records))]), "trashList").len(),
        1
    );
    login("bob");
    not_found("party/SECRET1");
    assert!(maps(&ok("list#Trash", &[("driveId", text(&records))]), "trashList").is_empty());
    // writes keep their rules, and reach only what the user may see
    login("eve");
    refused(
        "upload#Resource",
        &[
            ("driveId", text(&records)),
            ("path", text("party/SECRET1/content_2")),
            ("file", upload("more.pdf", b"%PDF")),
        ],
        "Folder not found",
    );
    ok(
        "upload#Resource",
        &[
            ("driveId", text(&records)),
            ("path", text("party/OPEN1/content_1")),
            ("file", upload("more.pdf", b"%PDF")),
        ],
    );
    login("carol");
    refused(
        "create#Drive",
        &[("driveName", text("Mine")), ("driveKindEnumId", text("RdkRecords"))],
        "Only an administrator",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "image")]
#[test]
fn an_image_preview_is_a_thumbnail_made_once() {
    let (context, facade) = setup();
    let _entered = context.enter();
    install();
    users(&facade);
    login("alice");
    let personal = out_text(&ok("ensure#PersonalDrive", &[]), "driveId");
    let image = ::image::RgbImage::from_pixel(640, 320, ::image::Rgb([200, 30, 30]));
    let mut png = std::io::Cursor::new(Vec::new());
    ::image::DynamicImage::ImageRgb8(image)
        .write_to(&mut png, ::image::ImageFormat::Png)
        .unwrap();
    ok(
        "upload#Resource",
        &[
            ("driveId", text(&personal)),
            ("file", upload("photo.png", &png.into_inner())),
        ],
    );
    let preview = ok(
        "get#Preview",
        &[
            ("driveId", text(&personal)),
            ("path", text("photo.png")),
            ("size", Value::I64(128)),
        ],
    );
    assert_eq!(out_text(&preview, "kind"), "image");
    assert_eq!((number(preview.get("width")), number(preview.get("height"))), (128, 64));
    let location = out_text(&preview, "previewLocation");
    let made = context
        .factory()
        .resource()
        .get(&location)
        .unwrap()
        .read_bytes()
        .unwrap();
    assert!(made.starts_with(b"\x89PNG"));
    let again = ok(
        "get#Preview",
        &[
            ("driveId", text(&personal)),
            ("path", text("photo.png")),
            ("size", Value::I64(128)),
        ],
    );
    assert_eq!(out_text(&again, "previewLocation"), location, "the cached thumbnail");
    let _ = std::fs::remove_file(location.replace("runtime://", "./"));
    ok(
        "upload#Resource",
        &[
            ("driveId", text(&personal)),
            ("file", upload("notes.md", b"# Notes\nhello")),
        ],
    );
    let text_preview = ok(
        "get#Preview",
        &[("driveId", text(&personal)), ("path", text("notes.md"))],
    );
    assert_eq!(out_text(&text_preview, "kind"), "text");
    assert!(out_text(&text_preview, "text").contains("hello"));
    let recent: Vec<String> = maps(&ok("find#Recent", &[]), "resultList")
        .iter()
        .map(|row| field(row, "path"))
        .collect();
    assert_eq!(recent.first().map(String::as_str), Some("notes.md"));
}

#[test]
fn move_copy_rename_favorites_tags_zip_and_find() {
    let (context, facade) = setup();
    let _entered = context.enter();
    install();
    users(&facade);
    login("alice");
    let one = drive("folder", "One", &[]);
    let two = drive("gitDb", "Two", &[]);
    ok(
        "upload#Resource",
        &[
            ("driveId", text(&one)),
            ("path", text("docs")),
            ("fileList", files(&[("plan.md", "plan"), ("budget.xlsx", "xl")])),
        ],
    );
    ok(
        "set#Favorite",
        &[("driveId", text(&one)), ("path", text("docs/plan.md"))],
    );
    ok(
        "set#Tags",
        &[
            ("driveId", text(&one)),
            ("path", text("docs/plan.md")),
            ("tagNameList", Value::List(vec![text("Q3"), text("draft")])),
        ],
    );
    let listed = maps(
        &ok("list#Folder", &[("driveId", text(&one)), ("path", text("docs"))]),
        "fileList",
    );
    let plan = listed.iter().find(|row| field(row, "name") == "plan.md").unwrap();
    assert!(flag(plan, "favorite"));
    assert!(to_plain_string(plan.get("tags").unwrap()).contains("Q3"));
    assert_eq!(field(plan, "fileType"), "text");
    ok(
        "rename#Resource",
        &[
            ("driveId", text(&one)),
            ("path", text("docs")),
            ("newName", text("papers")),
        ],
    );
    let listed = maps(
        &ok("list#Folder", &[("driveId", text(&one)), ("path", text("papers"))]),
        "fileList",
    );
    let plan = listed.iter().find(|row| field(row, "name") == "plan.md").unwrap();
    assert!(flag(plan, "favorite"), "the favorite followed the rename");
    assert!(
        to_plain_string(plan.get("tags").unwrap()).contains("draft"),
        "the tags followed the rename"
    );
    ok("create#Folder", &[("driveId", text(&one)), ("name", text("archive"))]);
    let moved = ok(
        "move#Resource",
        &[
            ("driveId", text(&one)),
            ("path", text("papers/plan.md")),
            ("toPath", text("archive")),
        ],
    );
    assert_eq!(field(&maps(&moved, "movedList")[0], "newPath"), "archive/plan.md");
    let refused_move = ok(
        "move#Resource",
        &[
            ("driveId", text(&one)),
            ("path", text("archive")),
            ("toPath", text("archive")),
        ],
    );
    assert_eq!(field(&maps(&refused_move, "movedList")[0], "outcome"), "failed");
    let copied = ok(
        "copy#Resource",
        &[
            ("driveId", text(&one)),
            ("path", text("papers")),
            ("toDriveId", text(&two)),
        ],
    );
    assert_eq!(field(&maps(&copied, "copiedList")[0], "outcome"), "copied");
    assert_eq!(read(&format!("git://DRIVE_{two}/papers/budget.xlsx")), "xl");
    let across = ok(
        "move#Resource",
        &[
            ("driveId", text(&one)),
            ("path", text("archive/plan.md")),
            ("toDriveId", text(&two)),
            ("toPath", text("papers")),
        ],
    );
    assert_eq!(field(&maps(&across, "movedList")[0], "outcome"), "moved");
    assert_eq!(read(&format!("git://DRIVE_{two}/papers/plan.md")), "plan");
    assert_eq!(
        maps(&ok("list#Trash", &[("driveId", text(&one))]), "trashList").len(),
        1,
        "the source went to its trash"
    );
    let zip = ok("download#Zip", &[("driveId", text(&two)), ("path", text("papers"))]);
    assert_eq!(number(zip.get("fileCount")), 2);
    assert_eq!(out_text(&zip, "zipFilename"), "papers.zip");
    let zip_path = out_text(&zip, "zipLocation").replace("runtime://", "./");
    let archive = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
    let mut entries: Vec<String> = archive.file_names().map(str::to_string).collect();
    entries.sort();
    assert_eq!(entries, ["budget.xlsx", "plan.md"]);
    let _ = std::fs::remove_file(&zip_path);
    let found = ok("find#Resources", &[("text", text("budg"))]);
    let hits: Vec<(String, String)> = maps(&found, "resultList")
        .iter()
        .map(|row| (field(row, "driveId"), field(row, "path")))
        .collect();
    assert_eq!(hits.len(), 2, "{hits:?}");
    assert!(hits.contains(&(two.clone(), "papers/budget.xlsx".to_string())));
    assert_eq!(
        found.get("usedIndex"),
        Some(&Value::Bool(false)),
        "no search here: the walk"
    );
    let spreadsheets = ok(
        "find#Resources",
        &[("fileType", text("spreadsheet")), ("driveId", text(&one))],
    );
    assert_eq!(maps(&spreadsheets, "resultList").len(), 1);
    login("dave");
    assert!(
        maps(&ok("find#Resources", &[("text", text("budg"))]), "resultList").is_empty(),
        "a stranger finds nothing"
    );
    refused(
        "copy#Resource",
        &[
            ("driveId", text(&one)),
            ("path", text("papers")),
            ("toDriveId", text(&two)),
        ],
        "Folder not found",
    );
}

/// A search store that keeps what is indexed and deleted, for the drive file documents.
#[derive(Default)]
struct Recorder {
    indexed: Mutex<Vec<(String, crate::entity::document::Document)>>,
    deletes: Mutex<usize>,
}

struct RecordingBackend(Arc<Recorder>);

impl crate::search::SearchBackend for RecordingBackend {
    fn name(&self) -> &'static str {
        "recording"
    }
    fn ensure_index(&self, _mapping: &crate::search::DocumentMapping) -> Result<(), moqui_lib::error::Error> {
        Ok(())
    }
    fn index_exists(&self, _mapping: &crate::search::DocumentMapping) -> Result<bool, moqui_lib::error::Error> {
        Ok(true)
    }
    fn index_documents(
        &self,
        _mapping: &crate::search::DocumentMapping,
        documents: &[(String, crate::entity::document::Document)],
    ) -> Result<usize, moqui_lib::error::Error> {
        moqui_lib::sync::lock(&self.0.indexed).extend(documents.iter().cloned());
        Ok(documents.len())
    }
    fn delete_document(
        &self,
        _mapping: &crate::search::DocumentMapping,
        _id: &str,
    ) -> Result<bool, moqui_lib::error::Error> {
        Ok(true)
    }
    fn delete_matching(
        &self,
        _mapping: &crate::search::DocumentMapping,
        _filter: &crate::search::Query,
    ) -> Result<u64, moqui_lib::error::Error> {
        *moqui_lib::sync::lock(&self.0.deletes) += 1;
        Ok(1)
    }
    fn get_document(
        &self,
        _mapping: &crate::search::DocumentMapping,
        _id: &str,
    ) -> Result<Option<crate::entity::document::Document>, moqui_lib::error::Error> {
        Ok(None)
    }
    fn search(
        &self,
        _mappings: &[Arc<crate::search::DocumentMapping>],
        _request: &crate::search::SearchRequest,
    ) -> Result<crate::search::SearchPage, moqui_lib::error::Error> {
        Ok(crate::search::SearchPage::default())
    }
    fn aggregate(
        &self,
        _mapping: &crate::search::DocumentMapping,
        _filter: &crate::search::Query,
        _aggs: &IndexMap<String, crate::search::Agg>,
    ) -> Result<IndexMap<String, Value>, moqui_lib::error::Error> {
        Ok(IndexMap::new())
    }
    fn drop_index(&self, _mapping: &crate::search::DocumentMapping) -> Result<(), moqui_lib::error::Error> {
        Ok(())
    }
}

/// The seed data of `moqui.resource.ResourceDrive` in ResourceEntities.xml (the enumerations and the MoquiDriveFile
/// document), loaded as written.
fn seed_drive_entities(facade: &EntityFacade) {
    let path = crate::context::framework_dir().join("entity/ResourceEntities.xml");
    let root = moqui_lib::node::MNode::parse_file(&path).unwrap();
    let entity = root
        .children_named("entity")
        .into_iter()
        .find(|node| node.attribute("entity-name").as_deref() == Some("ResourceDrive"))
        .unwrap();
    let seed = entity.first_named("seed-data").unwrap();
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    for node in seed.children() {
        let mut value = facade.make_value(&node.name()).unwrap();
        for (name, attribute) in node.attributes() {
            value.set(&name, Value::string(attribute));
        }
        value.create(facade).unwrap();
    }
}

#[test]
fn drive_files_are_search_documents_kept_on_write_rename_and_trash() {
    let (context, facade) = setup();
    let _entered = context.enter();
    install();
    users(&facade);
    seed_drive_entities(&facade);
    let recorder = Arc::new(Recorder::default());
    crate::search::install(
        context.factory(),
        crate::search::SearchFacade::new(Arc::new(RecordingBackend(Arc::clone(&recorder))), facade.clone()),
    );
    login("alice");
    let team = drive("folder", "Team", &[]);
    ok(
        "upload#Resource",
        &[
            ("driveId", text(&team)),
            ("path", text("docs")),
            ("file", upload("Budget 2026.xlsx", b"xl")),
        ],
    );
    {
        let indexed = moqui_lib::sync::lock(&recorder.indexed);
        let (id, body) = indexed.last().unwrap();
        assert_eq!(id, &format!("{team}:docs/Budget 2026.xlsx"));
        assert_eq!(
            body.get("name").map(to_plain_string).as_deref(),
            Some("Budget 2026.xlsx")
        );
        assert_eq!(
            body.get("fileType").map(to_plain_string).as_deref(),
            Some("spreadsheet")
        );
        assert_eq!(
            body.get("modifiedByUserId").map(to_plain_string).as_deref(),
            Some("alice")
        );
        assert_eq!(body.get("folder").map(to_plain_string).as_deref(), Some("docs"));
    }
    ok(
        "set#Tags",
        &[
            ("driveId", text(&team)),
            ("path", text("docs/Budget 2026.xlsx")),
            ("tagNameList", Value::List(vec![text("finance")])),
        ],
    );
    assert_eq!(
        moqui_lib::sync::lock(&recorder.indexed)
            .last()
            .unwrap()
            .1
            .get("tags")
            .map(to_plain_string)
            .as_deref(),
        Some("finance")
    );
    let deletes_before = *moqui_lib::sync::lock(&recorder.deletes);
    ok(
        "rename#Resource",
        &[
            ("driveId", text(&team)),
            ("path", text("docs")),
            ("newName", text("money")),
        ],
    );
    assert_eq!(
        *moqui_lib::sync::lock(&recorder.deletes),
        deletes_before + 1,
        "the old path's documents go"
    );
    let ids: Vec<String> = moqui_lib::sync::lock(&recorder.indexed)
        .iter()
        .map(|(id, _)| id.clone())
        .collect();
    assert!(
        ids.contains(&format!("{team}:money")) && ids.contains(&format!("{team}:money/Budget 2026.xlsx")),
        "{ids:?}"
    );
    ok("trash#Resource", &[("driveId", text(&team)), ("path", text("money"))]);
    assert_eq!(
        *moqui_lib::sync::lock(&recorder.deletes),
        deletes_before + 2,
        "trash removes the documents"
    );
    let found = ok("find#Resources", &[("text", text("budget"))]);
    assert_eq!(
        found.get("usedIndex"),
        Some(&Value::Bool(true)),
        "with search the document is queried"
    );

    // a mirror's documents follow its head after a fetch: the whole drive the first time, then the paths that changed
    let mirror = drive("gitDb", "Mirror like", &[]);
    let repository = format!("DRIVE_{mirror}");
    let resources = context.factory().resource();
    resources
        .write_bytes(&format!("git://{repository}/one.txt"), b"1")
        .unwrap();
    resources
        .write_bytes(&format!("git://{repository}/sub/two.txt"), b"2")
        .unwrap();
    let indexed_ids = || -> Vec<String> {
        moqui_lib::sync::lock(&recorder.indexed)
            .iter()
            .map(|(id, _)| id.clone())
            .collect()
    };
    let before = indexed_ids().len();
    ok("index#MirrorDrives", &[("repositoryId", text(&repository))]);
    let first: Vec<String> = indexed_ids()[before..].to_vec();
    assert_eq!(
        first,
        [
            format!("{mirror}:one.txt"),
            format!("{mirror}:sub"),
            format!("{mirror}:sub/two.txt")
        ]
    );
    resources
        .write_bytes(&format!("git://{repository}/sub/two.txt"), b"22")
        .unwrap();
    let before = indexed_ids().len();
    ok(
        "index#MirrorDrives",
        &[("repositoryIdList", Value::List(vec![text(&repository)]))],
    );
    assert_eq!(
        indexed_ids()[before..].to_vec(),
        [format!("{mirror}:sub/two.txt")],
        "only what changed"
    );
    let before = indexed_ids().len();
    ok("index#MirrorDrives", &[("repositoryId", text(&repository))]);
    assert_eq!(indexed_ids().len(), before, "nothing new since");
}

/// A PDF engine that previews any PDF as the same small image, with three pages and the size of the PDF as its text.
struct FakePdf;

impl crate::screen::PdfEngine for FakePdf {
    fn render(&self, _html_print_document: &str) -> Result<Vec<u8>, crate::context::Error> {
        Err(crate::context::Error::new("the fake engine only previews"))
    }

    fn preview(
        &self,
        request: &crate::screen::PdfPreviewRequest<'_>,
    ) -> Result<crate::screen::PdfPreview, crate::context::Error> {
        if !request.pdf.starts_with(b"%PDF") {
            return Err(crate::context::Error::new("not a PDF"));
        }
        assert!(!request.library.main.is_empty() && !request.library.worker.is_empty());
        Ok(crate::screen::PdfPreview {
            png: if request.image {
                b"\x89PNG\r\n\x1a\nfake".to_vec()
            } else {
                Vec::new()
            },
            width: 30,
            height: 40,
            pages: 3,
            text: format!("fake pdf text {}", request.pdf.len()),
        })
    }
}

/// The file under `./tmp/preview` (runtime://tmp in a test) whose text holds `marker`.
fn cached_preview(marker: &str) -> Option<std::path::PathBuf> {
    std::fs::read_dir("tmp/preview")
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| std::fs::read_to_string(path).is_ok_and(|text| text.contains(marker)))
}

#[test]
fn pdf_and_spreadsheet_previews_are_cached_by_version_and_their_text_is_indexed() {
    let _pdf = crate::screen::pdf_test_lock();
    crate::screen::clear_pdf_engine();
    // the async extract#DriveFileText runs at run_pending
    let context = ExecutionContextFactory::test_context_in(
        "",
        crate::service::ServicePools::inline(),
        crate::context::framework_dir(),
    );
    let facade = crate::testing::open();
    facade.assume_db_tables();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    install();
    users(&facade);
    seed_drive_entities(&facade);
    let recorder = Arc::new(Recorder::default());
    crate::search::install(
        context.factory(),
        crate::search::SearchFacade::new(Arc::new(RecordingBackend(Arc::clone(&recorder))), facade.clone()),
    );
    login("alice");
    let team = drive("folder", "Previews", &[]);
    // a folder of its own each run: the previews and text kept under ./tmp by an earlier run have other keys
    let folder = format!("run{}", crate::user::now_millis());
    let preview = |path: &str, extra: &[(&str, Value)]| {
        let mut parameters = vec![("driveId", text(&team)), ("path", text(&format!("{folder}/{path}")))];
        parameters.extend(extra.iter().cloned());
        ok("get#Preview", &parameters)
    };

    // a spreadsheet: the first rows of the first sheet, then the same from the cache
    let marker = format!("Sheet{}", std::process::id());
    let mut book = rust_xlsxwriter::Workbook::new();
    let sheet = book.add_worksheet().set_name(&marker).unwrap();
    sheet.write_string(0, 0, "item").unwrap();
    sheet.write_string(0, 1, "qty").unwrap();
    sheet.write_string(1, 0, "Widget").unwrap();
    sheet.write_number(1, 1, 4.0).unwrap();
    book.add_worksheet()
        .set_name("Other")
        .unwrap()
        .write_string(0, 0, "elsewhere")
        .unwrap();
    ok(
        "upload#Resource",
        &[
            ("driveId", text(&team)),
            ("path", text(&folder)),
            ("file", upload("Budget.xlsx", &book.save_to_buffer().unwrap())),
        ],
    );
    let sheet = preview("Budget.xlsx", &[]);
    assert_eq!(out_text(&sheet, "kind"), "spreadsheet");
    assert_eq!(out_text(&sheet, "sheetName"), marker);
    assert_eq!(
        sheet.get("sheetNames"),
        Some(&Value::List(vec![text(&marker), text("Other")]))
    );
    assert_eq!(
        sheet.get("rows"),
        Some(&Value::List(vec![
            Value::List(vec![text("item"), text("qty")]),
            Value::List(vec![text("Widget"), Value::I32(4)]),
        ]))
    );
    let cached = cached_preview(&marker).expect("the cached sheet preview");
    let kept = std::fs::read_to_string(&cached).unwrap();
    std::fs::write(&cached, kept.replace("Widget", "Gadget")).unwrap();
    let again = preview("Budget.xlsx", &[]);
    assert!(
        format!("{:?}", again.get("rows")).contains("Gadget"),
        "the second preview reads the cache"
    );
    let _ = std::fs::remove_file(&cached);
    let other = preview("Budget.xlsx", &[("sheetName", text("Other"))]);
    assert_eq!(
        other.get("rows"),
        Some(&Value::List(vec![Value::List(vec![text("elsewhere")])]))
    );

    // a PDF with no engine: kind pdf, no image, no error
    ok(
        "upload#Resource",
        &[
            ("driveId", text(&team)),
            ("path", text(&folder)),
            ("file", upload("Report.pdf", b"%PDF-1.4 first")),
        ],
    );
    let bare = preview("Report.pdf", &[("size", Value::I64(400))]);
    assert_eq!(out_text(&bare, "kind"), "pdf");
    assert!(bare.get("previewLocation").is_none_or(Value::is_null), "{bare:?}");

    // with an engine: page 1 as an image and the page count; a new version gets a new image
    crate::screen::install_pdf_engine(Arc::new(FakePdf));
    let drawn = preview("Report.pdf", &[("size", Value::I64(400))]);
    assert_eq!(out_text(&drawn, "kind"), "pdf");
    assert_eq!(out_text(&drawn, "contentType"), "image/png");
    assert_eq!(number(drawn.get("pages")), 3);
    assert_eq!((number(drawn.get("width")), number(drawn.get("height"))), (30, 40));
    let first = out_text(&drawn, "previewLocation");
    assert!(read(&first).contains("fake"));
    assert_eq!(
        out_text(&preview("Report.pdf", &[("size", Value::I64(400))]), "previewLocation"),
        first,
        "the cached page"
    );
    ok(
        "upload#Resource",
        &[
            ("driveId", text(&team)),
            ("path", text(&folder)),
            ("file", upload("Report.pdf", b"%PDF-1.4 second version")),
        ],
    );
    let newer = preview("Report.pdf", &[("size", Value::I64(400))]);
    assert_ne!(out_text(&newer, "previewLocation"), first, "a new version, a new key");
    assert_eq!(number(newer.get("pages")), 3);

    // extracted text: index#DriveFiles queues what is not cached, extract#DriveFileText fills it and indexes again
    ok(
        "upload#Resource",
        &[
            ("driveId", text(&team)),
            ("path", text(&folder)),
            ("file", upload("Notes.pdf", b"%PDF-1.4 notes body")),
        ],
    );
    let text_of = |path: &str| -> Option<String> {
        moqui_lib::sync::lock(&recorder.indexed)
            .iter()
            .rev()
            .find(|(id, _)| id == &format!("{team}:{folder}/{path}"))
            .and_then(|(_, body)| {
                body.get("extractedText")
                    .filter(|value| !value.is_null())
                    .map(to_plain_string)
            })
    };
    assert_eq!(text_of("Notes.pdf"), None, "not extracted yet");
    assert!(context.factory().run_pending() > 0);
    assert_eq!(text_of("Notes.pdf").as_deref(), Some("fake pdf text 19"));
    assert!(
        text_of("Budget.xlsx").is_some_and(|text| text.contains("Widget\t4")),
        "{:?}",
        text_of("Budget.xlsx")
    );
    // indexed again from the cache, with nothing queued
    ok(
        "rename#Resource",
        &[
            ("driveId", text(&team)),
            ("path", text(&format!("{folder}/Notes.pdf"))),
            ("newName", text("Kept.pdf")),
        ],
    );
    assert_eq!(context.factory().run_pending(), 1, "a new path, a new key");
    assert_eq!(text_of("Kept.pdf").as_deref(), Some("fake pdf text 19"));
    ok(
        "set#Tags",
        &[
            ("driveId", text(&team)),
            ("path", text(&format!("{folder}/Kept.pdf"))),
            ("tagNameList", Value::List(vec![text("t")])),
        ],
    );
    assert_eq!(context.factory().run_pending(), 0, "the text comes from the cache");
    assert_eq!(text_of("Kept.pdf").as_deref(), Some("fake pdf text 19"));
    crate::screen::clear_pdf_engine();
}

/// What `send` wrote to the response: status, headers and body; the response is cleared for the next one.
fn sent(send: impl FnOnce()) -> (u16, Vec<(String, String)>, Vec<u8>) {
    crate::web::clear_output();
    send();
    let (status, headers, body, _) = crate::web::take_output();
    crate::web::clear_output();
    (status, headers, body)
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> &'a str {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map_or("", |(_, value)| value.as_str())
}

#[test]
fn a_past_version_and_a_zip_are_sent_by_their_names_privately_and_a_link_sends_its_file_itself() {
    let (context, facade) = setup();
    let _entered = context.enter();
    install();
    users(&facade);
    let _web = crate::web::install(BTreeMap::new());
    login("alice");
    let team = drive("folder", "Team", &[]);
    for body in ["one\n", "two\n"] {
        ok(
            "upload#Resource",
            &[
                ("driveId", text(&team)),
                ("path", text("docs")),
                ("file", upload("notes.txt", body.as_bytes())),
            ],
        );
    }
    let versions = maps(
        &ok(
            "get#Versions",
            &[("driveId", text(&team)), ("path", text("docs/notes.txt"))],
        ),
        "versionList",
    );
    let first = field(versions.last().unwrap(), "versionName");
    let info = ok(
        "get#ResourceInfo",
        &[
            ("driveId", text(&team)),
            ("path", text("docs/notes.txt")),
            ("versionName", text(&first)),
        ],
    );
    let location = match info.get("resource") {
        Some(Value::Map(resource)) => field(resource, "location"),
        other => panic!("no resource: {other:?}"),
    };
    assert!(location.ends_with(&format!("#{first}")), "{location}");

    // the version, by reference and in a response, named as asked and kept by no shared cache
    let script = |source: &str| crate::script::run_script(source).unwrap();
    script(&format!(
        "ec.context.sharedMap.streamed = ec.resource.getLocationReference('{location}').openStream().getText()"
    ));
    assert_eq!(context.shared_map().get("streamed"), Some(&text("one\n")));
    let (status, headers, body) = sent(|| {
        script(&format!(
            "ec.web.sendResourceResponse('{location}', false, 'notes v1.txt')"
        ))
    });
    assert_eq!((status, body.as_slice()), (200, b"one\n".as_slice()));
    let disposition = header(&headers, "Content-Disposition");
    assert!(
        disposition.starts_with("attachment; filename=\"notes_v1.txt\"") && disposition.contains("notes%20v1.txt"),
        "{disposition}"
    );
    assert_eq!(header(&headers, "Cache-Control"), "private, no-store");
    assert!(
        header(&headers, "Content-Type").starts_with("text/plain"),
        "{headers:?}"
    );
    let missing = location.replace(&format!("#{first}"), "#99");
    let (status, _, _) = sent(|| script(&format!("ec.web.sendResourceResponse('{missing}', false)")));
    assert_eq!(status, 404, "a version that is not there");
    let current = location.split('#').next().unwrap().to_string();
    let (_, headers, body) = sent(|| script(&format!("ec.web.sendResourceResponse('{current}', true)")));
    assert_eq!(body, b"two\n");
    assert_eq!(header(&headers, "Content-Disposition"), "inline");
    assert_eq!(
        header(&headers, "Cache-Control"),
        "private, no-store",
        "a drive's file shown inline"
    );

    // a zip made under a random name goes out under its own
    let zip = ok("download#Zip", &[("driveId", text(&team)), ("path", text("docs"))]);
    let (zip_location, zip_name) = (out_text(&zip, "zipLocation"), out_text(&zip, "zipFilename"));
    assert_eq!(zip_name, "docs.zip");
    let (_, headers, body) = sent(|| {
        script(&format!(
            "ec.web.sendResourceResponse('{zip_location}', false, '{zip_name}')"
        ))
    });
    assert!(body.starts_with(b"PK"));
    assert!(
        header(&headers, "Content-Disposition").contains("filename=\"docs.zip\""),
        "{headers:?}"
    );
    assert_eq!(header(&headers, "Content-Type"), "application/zip");

    // a public link's file: the service sends it itself, after the token check, to a visitor with no user
    let link = ok(
        "share#Resource",
        &[("driveId", text(&team)), ("path", text("docs")), ("isLink", text("Y"))],
    );
    let token = out_text(&link, "linkToken");
    logout();
    let (status, headers, body) = sent(|| {
        let out = ok(
            "download#PublicShare",
            &[
                ("token", text(&token)),
                ("path", text("notes.txt")),
                ("sendResponse", Value::Bool(true)),
                ("inline", Value::Bool(true)),
            ],
        );
        assert_eq!(out_text(&out, "fileName"), "notes.txt");
    });
    assert_eq!((status, body.as_slice()), (200, b"two\n".as_slice()));
    assert!(
        header(&headers, "Content-Disposition").starts_with("attachment; filename=\"notes.txt\""),
        "a text file is never shown in the page: {headers:?}"
    );
    assert_eq!(header(&headers, "Cache-Control"), "private, no-store");
    let (_, headers, body) = sent(|| {
        ok(
            "download#PublicShare",
            &[("token", text(&token)), ("sendResponse", Value::Bool(true))],
        );
    });
    assert!(body.starts_with(b"PK"), "the folder as a zip");
    assert!(
        header(&headers, "Content-Disposition").contains("filename=\"docs.zip\""),
        "{headers:?}"
    );
    let (status, _, body) = sent(|| {
        refused(
            "download#PublicShare",
            &[
                ("token", text("NoSuchTokenAtAll0000")),
                ("sendResponse", Value::Bool(true)),
            ],
            "expired or was not found",
        );
    });
    assert!(status == 200 && body.is_empty(), "nothing is sent for a bad token");
}

#[test]
fn a_stream_passed_to_a_service_stays_its_bytes_and_anything_else_is_never_written_as_text() {
    let (context, facade) = setup();
    let _entered = context.enter();
    install();
    users(&facade);
    login("alice");
    let team = drive("folder", "Team", &[]);
    ok(
        "upload#Resource",
        &[
            ("driveId", text(&team)),
            ("file", upload("source.bin", &[0, 1, 2, 250, 251])),
        ],
    );
    let root = format!("dbresource://drive/shared/{team}");
    crate::script::run_script(&format!(
        r#"const out = ec.service.sync().name('org.moqui.impl.FileServices.upload#Resource').parameters({{driveId: '{team}',
            fileList: [ec.resource.getLocationReference('{root}/source.bin').openStream(), {{}}],
            relativePathList: ['copy.bin', 'object.bin']}}).call()
        ec.context.sharedMap.outcomes = out.uploadedList.map((row) => row.outcome + ':' + (row.error || ''))"#
    ))
    .unwrap();
    assert_eq!(
        context.shared_map().get("outcomes"),
        Some(&Value::List(vec![
            text("created:"),
            text("failed:not a file or a stream")
        ]))
    );
    let copied = context.factory().resource().get(&format!("{root}/copy.bin")).unwrap();
    assert_eq!(copied.read_bytes().unwrap(), [0, 1, 2, 250, 251]);
    let exists = |location: String| context.factory().resource().stat(&location).unwrap().is_some();
    assert!(!exists(format!("{root}/object.bin")));
    // a stream kept in the context and read back is still the stream
    crate::script::run_script(&format!(
        "ec.context.sharedMap.kept = ec.resource.getLocationReference('{root}/source.bin').openStream()"
    ))
    .unwrap();
    crate::script::run_script(&format!(
        "ec.resource.putContent('{root}/kept.bin', ec.context.sharedMap.kept, 'from the context')"
    ))
    .unwrap();
    assert_eq!(
        context
            .factory()
            .resource()
            .get(&format!("{root}/kept.bin"))
            .unwrap()
            .read_bytes()
            .unwrap(),
        [0, 1, 2, 250, 251]
    );
    for source in [
        format!("ec.resource.putContent('{root}/bad.bin', {{}}, null)"),
        format!("ec.resource.makeChangeSet('{root}').write('bad.bin', [1, 2]).commit(null)"),
    ] {
        let err = crate::script::run_script(&source).unwrap_err();
        assert!(
            err.message().contains("text, bytes, an uploaded file or a stream"),
            "{err}"
        );
    }
    assert!(!exists(format!("{root}/bad.bin")));
}

#[test]
fn the_drive_temporary_files_older_than_a_day_are_removed() {
    let (context, _facade) = setup();
    let _entered = context.enter();
    install();
    let folder = format!("drive-cleaning-{}", std::process::id());
    let tmp = context.factory().runtime_dir().join("tmp").join(&folder);
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("a1b2")).unwrap();
    std::fs::write(tmp.join("old.zip"), b"old").unwrap();
    std::fs::write(tmp.join("a1b2/named copy.pdf"), b"old").unwrap();
    std::fs::write(tmp.join("new.zip"), b"new").unwrap();
    let two_days_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86_400);
    for old in ["old.zip", "a1b2/named copy.pdf"] {
        std::fs::File::options()
            .write(true)
            .open(tmp.join(old))
            .unwrap()
            .set_modified(two_days_ago)
            .unwrap();
    }
    let out = {
        let _authz = context.disable_authz();
        ok("clean#DriveTempFiles", &[])
    };
    assert!(number(out.get("removedCount")) >= 2);
    assert!(!tmp.join("old.zip").exists());
    assert!(!tmp.join("a1b2").exists(), "the folder its last file left");
    assert!(tmp.join("new.zip").exists(), "a file of today stays");
    let _ = std::fs::remove_dir_all(&tmp);
}
/// An external user (the permission EXTERNAL_USER, an app's client contacts) reaches no records drive, whatever its
/// membership and the record visibility hook say: not listed among the user's drives (the WebDAV and SFTP home), no
/// level at its top or in a record folder the hook shows, no list or file. An internal member keeps the record folders.
#[test]
fn an_external_user_reaches_no_records_drive() {
    let (context, facade) = setup();
    let _entered = context.enter();
    let dir = std::env::temp_dir().join(format!("moqui-files-external-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("service/test")).unwrap();
    std::fs::write(dir.join("service/test/DriveStub.xml"), RECORD_HOOK).unwrap();
    let framework = crate::context::framework_dir().join("service/org/moqui");
    let failures = crate::service::lint(&[
        framework.join("impl/FileServices.xml"),
        framework.join("impl/GitServices.xml"),
        framework.join("impl/ChatServices.xml"),
        dir.join("service/test/DriveStub.xml"),
    ])
    .unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    users(&facade);
    user(&facade, "outsider");
    row(
        &facade,
        "moqui.security.UserPermission",
        &[("userPermissionId", text("EXTERNAL_USER"))],
    );
    row(
        &facade,
        "moqui.security.UserGroup",
        &[("userGroupId", text("FX_OUTSIDE"))],
    );
    row(
        &facade,
        "moqui.security.UserGroupMember",
        &[
            ("userGroupId", text("FX_OUTSIDE")),
            ("userId", text("outsider")),
            ("fromDate", Value::I64(0)),
        ],
    );
    row(
        &facade,
        "moqui.security.UserGroupPermission",
        &[
            ("userGroupId", text("FX_OUTSIDE")),
            ("userPermissionId", text("EXTERNAL_USER")),
            ("fromDate", Value::I64(0)),
        ],
    );
    row(
        &facade,
        "moqui.screen.ScreenThemeResource",
        &[
            ("screenThemeId", text("DEFAULT_QUASAR")),
            ("sequenceNum", Value::I64(990)),
            ("resourceTypeEnumId", text("STRT_RECORD_VISIBLE_SERVICE")),
            ("resourceValue", text("test.DriveStub.check#Visible")),
        ],
    );
    let records = {
        let _authz = context.disable_authz();
        login("alice");
        out_text(
            &ok(
                "create#Drive",
                &[
                    ("driveName", text("Records")),
                    ("driveKindEnumId", text("RdkRecords")),
                    ("rootLocation", text("dbresource://mantle/content")),
                    (
                        "memberList",
                        Value::List(vec![
                            Value::Map(IndexMap::from([
                                ("userGroupId".to_string(), text("ALL_USERS")),
                                ("permissionEnumId".to_string(), text("RdpEdit")),
                            ])),
                            Value::Map(IndexMap::from([
                                ("userId".to_string(), text("outsider")),
                                ("permissionEnumId".to_string(), text("RdpManage")),
                            ])),
                        ]),
                    ),
                ],
            ),
            "driveId",
        )
    };
    row(
        &facade,
        "moqui.resource.ResourceDriveRecordFolder",
        &[
            ("driveId", text(&records)),
            ("folderPath", text("party")),
            ("recordEntityName", text("mantle.party.Party")),
            ("idField", text("partyId")),
            ("label", text("Parties")),
            ("sequenceNum", Value::I64(1)),
        ],
    );
    context
        .factory()
        .resource()
        .write_bytes(
            "dbresource://mantle/content/party/OPEN1/content_1/shared.pdf",
            b"%PDF shared",
        )
        .unwrap();
    let drive_ids = || -> Vec<String> {
        maps(&ok("get#UserDrives", &[]), "driveList")
            .iter()
            .map(|drive| field(drive, "driveId"))
            .collect()
    };

    // an internal member: the drive is listed and the record folder the hook shows is reached
    login("carol");
    assert!(drive_ids().contains(&records));
    assert!(level(&records, Some("party/OPEN1/content_1/shared.pdf")) >= 1);
    assert_eq!(
        names(&ok(
            "list#Folder",
            &[("driveId", text(&records)), ("path", text("party"))]
        )),
        ["OPEN1"]
    );

    // the external user, a manager by membership: nothing
    login("outsider");
    assert!(!drive_ids().contains(&records));
    assert_eq!(level(&records, None), 0);
    assert_eq!(level(&records, Some("")), 0);
    assert_eq!(level(&records, Some("party/OPEN1/content_1/shared.pdf")), 0);
    let visible = crate::service::sync(
        "org.moqui.impl.ChatServices.check#RecordVisible",
        &BTreeMap::from([
            ("recordEntityName".to_string(), text("mantle.party.Party")),
            ("recordId".to_string(), text("OPEN1")),
        ]),
    )
    .unwrap();
    assert_eq!(visible.get("recordVisible"), Some(&Value::Bool(false)));
    for (name, parameters) in [
        (
            "list#Folder",
            vec![("driveId", text(&records)), ("path", text("party"))],
        ),
        (
            "get#ResourceInfo",
            vec![
                ("driveId", text(&records)),
                ("path", text("party/OPEN1/content_1/shared.pdf")),
            ],
        ),
    ] {
        let _ = call(name, &parameters);
        assert!(!errors().is_empty(), "{name} answered the external user");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
