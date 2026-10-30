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

//! The framework services that were Groovy and are JavaScript now (`WikiServices`, the git update cleanup of
//! `InstanceServices`) on `git://` repositories. Their other tests are moqui-core's `service/script_ports_tests.rs`.

use std::collections::BTreeMap;

use moqui_lib::object::{to_plain_string, Value};

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_core::service::ServicePools;

fn setup() -> (EntityFacade, ExecutionContext) {
    let facade = moqui_core::testing::open();
    facade.assume_db_tables();
    let context =
        ExecutionContextFactory::test_context_in("", ServicePools::bounded(1, 4), moqui_core::context::framework_dir());
    context.factory().set_entity(facade.clone());
    (facade, context)
}

fn install(files: &[&str]) {
    let dir = moqui_core::context::framework_dir().join("service/org/moqui");
    let paths: Vec<_> = files.iter().map(|file| dir.join(file)).collect();
    let failures = moqui_core::service::lint(&paths).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
}

fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, Value> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), Value::string(*value)))
        .collect()
}

fn text(map: &BTreeMap<String, Value>, name: &str) -> String {
    map.get(name).map(to_plain_string).unwrap_or_default()
}

fn errors(context: &ExecutionContext) -> String {
    context.message().errors().join("\n")
}

fn seed(facade: &EntityFacade, entity: &str, pairs: &[(&str, &str)]) {
    let mut row = facade.make_value(entity).unwrap();
    for (name, value) in pairs {
        row.set(*name, Value::string(*value));
    }
    row.create(facade).unwrap();
}

/// A wiki space whose pages are `git://` files in a database repository: the wiki services use only the resource
/// API, so page versions are commits; then the GitServices export it, and refuse a fetch of a non-mirror.
#[test]
fn a_wiki_space_on_a_git_repository_keeps_commits_as_versions() {
    let (facade, context) = setup();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    crate::install(context.factory()).unwrap();
    install(&["EntityServices.xml", "impl/WikiServices.xml", "impl/GitServices.xml"]);
    seed(
        &facade,
        "moqui.resource.GitRepository",
        &[("repositoryId", "wiki"), ("repositoryKind", "db")],
    );
    let sync = |name: &str, pairs: &[(&str, &str)]| {
        let out = moqui_core::service::sync(name, &params(pairs)).unwrap_or_else(|err| panic!("{name}: {err}"));
        assert_eq!(errors(&context), "", "{name}");
        out
    };
    sync(
        "org.moqui.impl.WikiServices.create#WikiSpace",
        &[
            ("wikiSpaceId", "GTS"),
            ("description", "Git space"),
            ("rootPageDirectory", "git://wiki"),
        ],
    );
    for text in ["one", "two"] {
        sync(
            "org.moqui.impl.WikiServices.update#WikiPage",
            &[("wikiSpaceId", "GTS"), ("pageName", "Page1"), ("pageText", text)],
        );
    }
    let location = "git://wiki/GTS/Page1.md";
    let history = context.factory().resource().version_history(location).unwrap();
    assert_eq!(history.len(), 2);
    let info = sync(
        "org.moqui.impl.WikiServices.get#WikiPageInfo",
        &[("wikiSpaceId", "GTS"), ("pagePath", "Page1")],
    );
    assert_eq!(text(&info, "currentVersionName"), history[0].version_name);
    assert_eq!(text(&info, "pageLocation"), location);
    let first = sync(
        "org.moqui.impl.WikiServices.get#PublishedWikiPageText",
        &[
            ("wikiSpaceId", "GTS"),
            ("pagePath", "Page1"),
            ("versionName", &history[1].version_name),
        ],
    );
    assert_eq!(text(&first, "pageText"), "one");

    let target = std::env::temp_dir().join(format!("moqui-wiki-git-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&target);
    let exported = sync(
        "org.moqui.impl.GitServices.export#GitRepository",
        &[("repositoryId", "wiki"), ("targetPath", &target.to_string_lossy())],
    );
    assert_eq!(text(&exported, "exportPath"), target.to_string_lossy());
    assert!(target.join("HEAD").is_file());
    let _ = std::fs::remove_dir_all(&target);

    let out = moqui_core::service::sync(
        "org.moqui.impl.GitServices.fetch#GitRepository",
        &params(&[("repositoryId", "wiki")]),
    )
    .expect("a business failure is a message, not an Err");
    assert!(!out.contains_key("headCommitId"));
    assert!(errors(&context).contains("not a remote mirror"), "{}", errors(&context));
}

/// A wiki space rooted in a subdirectory of a code repository (`git://code/docs`, IronStone Projects' documentation
/// beside the code): its root page and pages are files under `docs/`, each save a commit beside the code's, which stays
/// as it was; history, child pages, a rename, the page list and a delete work as at a repository's root, and a branch
/// rule protecting the default branch does not refuse the saves (`git://` writes pass the rules and are logged).
#[test]
fn a_wiki_space_in_a_code_repository_subdirectory_keeps_its_pages_beside_the_code() {
    let (facade, context) = setup();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    crate::install(context.factory()).unwrap();
    install(&["EntityServices.xml", "impl/WikiServices.xml", "impl/GitServices.xml"]);
    seed(
        &facade,
        "moqui.resource.GitRepository",
        &[("repositoryId", "code"), ("repositoryKind", "db")],
    );
    let resources = context.factory().resource();
    resources
        .write_bytes("git://code/src/main.rs", b"fn main() {}\n")
        .unwrap();
    resources.write_bytes("git://code/README.md", b"# Code\n").unwrap();
    seed(
        &facade,
        "moqui.resource.GitBranchRule",
        &[
            ("branchRuleId", "CODE_MAIN"),
            ("repositoryId", "code"),
            ("refPattern", "main"),
            ("allowForcePush", "N"),
            ("allowDelete", "N"),
            ("requireMergeRequest", "Y"),
        ],
    );
    let sync = |name: &str, pairs: &[(&str, &str)]| {
        let out = moqui_core::service::sync(name, &params(pairs)).unwrap_or_else(|err| panic!("{name}: {err}"));
        assert_eq!(errors(&context), "", "{name}");
        out
    };
    let made = sync(
        "org.moqui.impl.WikiServices.create#WikiSpace",
        &[
            ("wikiSpaceId", "CODEDOCS"),
            ("description", "Code documentation"),
            ("rootPageDirectory", "git://code/docs"),
        ],
    );
    assert_eq!(text(&made, "rootPageLocation"), "git://code/docs/CODEDOCS.md");
    assert!(resources.get("git://code/docs/CODEDOCS.md").unwrap().exists());
    let access = sync(
        "org.moqui.impl.WikiServices.get#WikiPageAccess",
        &[("wikiSpaceId", "CODEDOCS")],
    );
    assert_eq!(text(&access, "storage"), "git");
    assert_eq!(text(&access, "repositoryId"), "code");
    assert_eq!(text(&access, "canUpdate"), "true");

    for body in ["# Getting started\n\nOne.\n", "# Getting started\n\nTwo.\n"] {
        sync(
            "org.moqui.impl.WikiServices.update#WikiPage",
            &[
                ("wikiSpaceId", "CODEDOCS"),
                ("pageName", "GettingStarted"),
                ("pageText", body),
                ("versionNote", "Getting started"),
            ],
        );
    }
    // a child page under its parent page (a directory without its page file is not listed)
    sync(
        "org.moqui.impl.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", "CODEDOCS"),
            ("pageName", "Releases"),
            ("pageText", "# Releases\n"),
        ],
    );
    sync(
        "org.moqui.impl.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", "CODEDOCS"),
            ("pageName", "1.0"),
            ("parentPath", "Releases"),
            ("pageText", "# 1.0\n"),
        ],
    );
    let page = "git://code/docs/CODEDOCS/GettingStarted.md";
    assert_eq!(resources.location_text(page).unwrap(), "# Getting started\n\nTwo.\n");
    assert!(resources
        .get("git://code/docs/CODEDOCS/Releases/1.0.md")
        .unwrap()
        .exists());
    let history = resources.version_history(page).unwrap();
    assert_eq!(history.len(), 2);
    let info = sync(
        "org.moqui.impl.WikiServices.get#WikiPageInfo",
        &[("wikiSpaceId", "CODEDOCS"), ("pagePath", "GettingStarted")],
    );
    assert_eq!(text(&info, "pageLocation"), page);
    assert_eq!(text(&info, "currentVersionName"), history[0].version_name);
    // the code beside the pages is as it was
    assert_eq!(
        resources.location_text("git://code/src/main.rs").unwrap(),
        "fn main() {}\n"
    );
    assert_eq!(resources.location_text("git://code/README.md").unwrap(), "# Code\n");

    let pages = sync(
        "org.moqui.impl.WikiServices.get#WikiSpacePages",
        &[("wikiSpaceId", "CODEDOCS")],
    );
    let Some(Value::List(flat)) = pages.get("allChildFileFlatList") else {
        panic!("no pages: {pages:?}");
    };
    let mut paths: Vec<String> = flat
        .iter()
        .filter_map(|item| match item {
            Value::Map(map) => map.get("path").map(to_plain_string),
            _ => None,
        })
        .collect();
    paths.sort();
    assert_eq!(paths, ["", "GettingStarted", "Releases", "Releases/1.0"]);

    let renamed = sync(
        "org.moqui.impl.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", "CODEDOCS"),
            ("pagePath", "GettingStarted"),
            ("pageName", "Start"),
            ("pageText", "# Start\n"),
        ],
    );
    assert_eq!(text(&renamed, "pagePath"), "Start");
    assert!(resources.get("git://code/docs/CODEDOCS/Start.md").unwrap().exists());
    assert!(!resources.get(page).unwrap().exists());
    sync(
        "org.moqui.impl.WikiServices.delete#WikiPage",
        &[("wikiSpaceId", "CODEDOCS"), ("pagePath", "Start")],
    );
    assert!(!resources.get("git://code/docs/CODEDOCS/Start.md").unwrap().exists());
    assert_eq!(
        resources.location_text("git://code/src/main.rs").unwrap(),
        "fn main() {}\n"
    );
}

/// `clean#GitUpdates` removes done and none rows older than the window, and their refs, and leaves the rest.
/// One row whose delete fails is logged and skipped.
#[test]
fn old_done_git_updates_are_removed_and_one_failure_does_not_stop_the_rest() {
    let facade = moqui_core::testing::open_with(
        &moqui_core::testing::framework_dir(),
        moqui_core::testing::Options {
            entity_eca: true,
            data_feed: false,
            tape: None,
        },
    );
    facade.assume_db_tables();
    let context =
        ExecutionContextFactory::test_context_in("", ServicePools::bounded(1, 4), moqui_core::context::framework_dir());
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    install(&["impl/GitServices.xml"]);
    moqui_core::entity::load_feed::add_eca_rules(
        &facade,
        r#"<eecas><eeca id="FailGitUpdate" entity="moqui.resource.GitUpdate" on-delete="true" run-before="true">
            <condition><expression>updateId == 'FAIL'</expression></condition>
            <actions><script>throw new Error("forced")</script></actions>
        </eeca></eecas>"#,
    )
    .unwrap();
    let ago = |days: i64| {
        let millis = jiff::Timestamp::now().as_millisecond() - days * 86_400_000;
        moqui_lib::object::basic_convert(&Value::I64(millis), "java.sql.Timestamp").value
    };
    let row = |id: &str, status: &str, when: Value| {
        let mut update = facade.make_value("moqui.resource.GitUpdate").unwrap();
        update.set("updateId", Value::string(id));
        update.set("ecaStatus", Value::string(status));
        update.set("updateDate", when);
        update.set("source", Value::string("write"));
        update.create(&facade).unwrap();
    };
    let git_ref = |id: &str| {
        let mut reference = facade.make_value("moqui.resource.GitUpdateRef").unwrap();
        reference.set("updateId", Value::string(id));
        reference.set("refName", Value::string("refs/heads/main"));
        reference.create(&facade).unwrap();
    };
    row("OLD_DONE", "done", ago(200));
    git_ref("OLD_DONE");
    row("OLD_NONE", "none", ago(200));
    row("OLD_PENDING", "pending", ago(200));
    row("OLD_FAILED", "failed", ago(200));
    row("OLD_RUNNING", "running", ago(200));
    row("OLD_RECORDING", "recording", ago(200));
    row("NEW_DONE", "done", ago(1));
    git_ref("NEW_DONE");
    row("FAIL", "done", ago(200));
    let out = moqui_core::service::sync("org.moqui.impl.GitServices.clean#GitUpdates", &params(&[])).unwrap();
    assert_eq!(errors(&context), "", "{out:?}");
    assert_eq!(text(&out, "recordsRemoved"), "2", "{out:?}");
    let present = |id: &str| {
        facade
            .find("moqui.resource.GitUpdate")
            .unwrap()
            .eq("updateId", Value::string(id))
            .one()
            .unwrap()
            .is_some()
    };
    let refs = |id: &str| {
        facade
            .find("moqui.resource.GitUpdateRef")
            .unwrap()
            .eq("updateId", Value::string(id))
            .list()
            .unwrap()
            .len()
    };
    assert!(!present("OLD_DONE") && refs("OLD_DONE") == 0);
    assert!(!present("OLD_NONE"));
    for kept in [
        "OLD_PENDING",
        "OLD_FAILED",
        "OLD_RUNNING",
        "OLD_RECORDING",
        "NEW_DONE",
        "FAIL",
    ] {
        assert!(present(kept), "{kept}");
    }
    assert_eq!(refs("NEW_DONE"), 1);
}
