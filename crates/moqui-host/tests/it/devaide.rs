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

//! The dev-aide's services on a dev instance (plans/design-dev-aide.md P2): a site component and an upstream app, each its
//! own scratch git repository with a bare remote, and git isolated from the desktop (no global or system config, no
//! askpass, no credential helper). Saves reload and check, a save over a changed file is refused with the disk's text,
//! the upstream lock and the tiers hold, paths stay in their roots, writes are audited, commits name the user as author
//! with the task trailer, and Publish pushes the site's changes, to a publish branch when one is set, and not at all on a
//! shared server whose network git is off.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "devtools"))]

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;
use moqui_lib::object::{to_plain_string, Value};

use crate::common::start_with_files_and_links;

const SITE_COMPONENT: &str = r#"<component name="my-site"/>"#;
const SITE_SERVICES: &str = r#"<services>
    <service verb="get" noun="Thing">
        <out-parameters><parameter name="result"/></out-parameters>
        <actions><set field="result" value="first"/></actions>
    </service>
</services>
"#;
const UPSTREAM_COMPONENT: &str = r#"<component name="upstream-app"/>"#;
const UPSTREAM_SERVICES: &str = "<services>\n    <service verb=\"run\" noun=\"Up\"/>\n</services>\n";

/// git with the desktop left out: no global or system config, no prompt, no askpass.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The process's git, for the server and this test alike, kept from the desktop's config and credentials; the committer is
/// the server's identity.
fn isolate_git(home: &Path) {
    for (name, value) in [
        ("HOME", home.display().to_string()),
        ("GIT_CONFIG_GLOBAL", "/dev/null".to_string()),
        ("GIT_CONFIG_NOSYSTEM", "1".to_string()),
        ("GIT_TERMINAL_PROMPT", "0".to_string()),
        ("GIT_ASKPASS", String::new()),
        ("GIT_COMMITTER_NAME", "Dev Server".to_string()),
        ("GIT_COMMITTER_EMAIL", "server@example.com".to_string()),
        ("GIT_AUTHOR_NAME", "Setup".to_string()),
        ("GIT_AUTHOR_EMAIL", "setup@example.com".to_string()),
    ] {
        std::env::set_var(name, value);
    }
}

/// A repository with one commit on main, pushed to a bare remote beside it.
fn repository(dir: &Path, remote: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Start"]);
    git(
        remote.parent().unwrap(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    git(dir, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(dir, &["push", "-q", "-u", "origin", "main"]);
}

fn call(
    factory: &Arc<ExecutionContextFactory>,
    user: &str,
    name: &str,
    parameters: &[(&str, &str)],
) -> Result<BTreeMap<String, Value>, String> {
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user(user).unwrap();
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), Value::string(*value)))
        .collect();
    let result = moqui_core::service::sync(name, &parameters).map_err(|err| err.to_string());
    let errors = context.message().errors().to_vec();
    match result {
        Ok(out) if errors.is_empty() => Ok(out),
        Ok(_) => Err(errors.join("; ")),
        Err(err) => Err(format!("{err} {}", errors.join("; "))),
    }
}

/// A service of the test's own component, which no artifact group grants.
fn run(factory: &Arc<ExecutionContextFactory>, name: &str) -> BTreeMap<String, Value> {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    moqui_core::service::sync(name, &BTreeMap::new()).unwrap()
}

fn text(out: &BTreeMap<String, Value>, key: &str) -> String {
    out.get(key).map(to_plain_string).unwrap_or_default()
}

const WS: &str = "org.moqui.devaide.WorkspaceServices.";
const VCS: &str = "org.moqui.devaide.VcsServices.";

#[test]
fn the_workspace_saves_checks_commits_and_publishes() {
    let users = r#"
    <moqui.security.UserAccount userId="DEV_EXT" username="ext.user" userFullName="Ext User" emailAddress="ext@example.com"/>
    <moqui.security.UserGroupMember userGroupId="DEV_AIDE_EXTENDERS" userId="DEV_EXT" fromDate="0"/>
</entity-facade-xml>"#;
    let security = include_str!("../../../../runtime/dev-component/dev-aide/data/DevAideSecurityData.xml")
        .replace("</entity-facade-xml>", users);
    let dev_aide = std::fs::canonicalize(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/dev-component/dev-aide"
    ))
    .unwrap();
    let files = [
        ("component/my-site/component.xml", SITE_COMPONENT),
        ("component/my-site/service/mysite/Things.xml", SITE_SERVICES),
        ("component/upstream-app/component.xml", UPSTREAM_COMPONENT),
        ("component/upstream-app/service/upapp/Up.xml", UPSTREAM_SERVICES),
    ];
    let gate = start_with_files_and_links(
        "dev-aide",
        Some(&security),
        &[
            ("instance_purpose", "dev"),
            ("dev_aide_site_component", "my-site"),
            ("dev_aide_shared", "false"),
        ],
        &files,
        &[(dev_aide.to_str().unwrap(), "dev-component/dev-aide")],
    );
    let factory = Arc::clone(gate.server.factory());
    let site = gate.runtime.join("component/my-site");
    let upstream = gate.runtime.join("component/upstream-app");
    isolate_git(&gate.runtime);
    repository(&site, &gate.runtime.join("site-remote.git"));
    repository(&upstream, &gate.runtime.join("upstream-remote.git"));

    // the roots: the site component's repository and the upstream app's, by their directory names
    let roots = call(&factory, "john.doe", &format!("{WS}get#Roots"), &[]).unwrap();
    let listed = moqui_lib::json::to_json(&roots["roots"]);
    let root_of = |id: &str| {
        listed
            .as_array()
            .unwrap()
            .iter()
            .find(|root| root["id"] == id)
            .cloned()
            .unwrap()
    };
    assert_eq!(root_of("my-site")["upstream"], false, "{listed} {roots:?}");
    assert_eq!(root_of("my-site")["kind"], "git");
    assert_eq!(root_of("upstream-app")["upstream"], true);
    assert_eq!(text(&roots, "tier"), "full");

    // a save of a service file: written, reloaded, checked; the new service runs at once
    let path = "service/mysite/Things.xml";
    let file = call(
        &factory,
        "john.doe",
        &format!("{WS}get#File"),
        &[("rootId", "my-site"), ("path", path)],
    )
    .unwrap();
    let base = text(&file, "hash");
    let changed = SITE_SERVICES.replace("value=\"first\"", "value=\"second\"").replace(
        "</services>",
        "    <service verb=\"get\" noun=\"Other\"><out-parameters><parameter name=\"result\"/></out-parameters>\n        <actions><set field=\"result\" value=\"other\"/></actions></service>\n</services>",
    );
    let saved = call(
        &factory,
        "john.doe",
        &format!("{WS}save#File"),
        &[
            ("rootId", "my-site"),
            ("path", path),
            ("text", &changed),
            ("baseHash", &base),
        ],
    )
    .unwrap();
    assert_eq!(text(&saved, "conflict"), "false");
    let report = moqui_lib::json::to_json(&saved["reload"]);
    assert!(
        report["files"][0]["summary"]
            .as_str()
            .unwrap()
            .starts_with("service file reloaded"),
        "{report}"
    );
    assert_eq!(text(&saved, "status"), "M");
    let thing = run(&factory, "mysite.Things.get#Thing");
    assert_eq!(text(&thing, "result"), "second");
    let other = run(&factory, "mysite.Things.get#Other");
    assert_eq!(text(&other, "result"), "other");

    // a save from the old hash: the file changed since, so it is refused with the disk's text
    let stale = call(
        &factory,
        "john.doe",
        &format!("{WS}save#File"),
        &[
            ("rootId", "my-site"),
            ("path", path),
            ("text", "<services/>"),
            ("baseHash", &base),
        ],
    )
    .unwrap();
    assert_eq!(text(&stale, "conflict"), "true");
    assert_eq!(text(&stale, "diskText"), changed);
    assert_eq!(std::fs::read_to_string(site.join(path)).unwrap(), changed);

    // the upstream lock: refused, naming the site component to extend in
    let up = call(
        &factory,
        "john.doe",
        &format!("{WS}get#File"),
        &[("rootId", "upstream-app"), ("path", "service/upapp/Up.xml")],
    )
    .unwrap();
    assert_eq!(text(&up, "readOnly"), "true");
    let refused = call(
        &factory,
        "john.doe",
        &format!("{WS}save#File"),
        &[
            ("rootId", "upstream-app"),
            ("path", "service/upapp/Up.xml"),
            ("text", UPSTREAM_SERVICES),
            ("baseHash", &text(&up, "hash")),
        ],
    )
    .unwrap_err();
    assert!(
        refused.contains("upstream-app is upstream: extend it in my-site"),
        "{refused}"
    );

    // the extend tier: writes the site component, not upstream, and cannot turn the lock off
    let ext_file = call(
        &factory,
        "ext.user",
        &format!("{WS}get#File"),
        &[("rootId", "my-site"), ("path", path)],
    )
    .unwrap();
    call(
        &factory,
        "ext.user",
        &format!("{WS}save#File"),
        &[
            ("rootId", "my-site"),
            ("path", path),
            ("text", &changed.replace("other", "other2")),
            ("baseHash", &text(&ext_file, "hash")),
        ],
    )
    .unwrap();
    let ext_refused = call(
        &factory,
        "ext.user",
        &format!("{WS}set#UpstreamEdits"),
        &[("enabled", "true")],
    )
    .unwrap_err();
    assert!(ext_refused.contains("full tier"), "{ext_refused}");

    // the full tier turns the lock off: the upstream save goes through, with the upstream-edit warning
    call(
        &factory,
        "john.doe",
        &format!("{WS}set#UpstreamEdits"),
        &[("enabled", "true")],
    )
    .unwrap();
    let forked = call(
        &factory,
        "john.doe",
        &format!("{WS}save#File"),
        &[
            ("rootId", "upstream-app"),
            ("path", "service/upapp/Up.xml"),
            ("text", &UPSTREAM_SERVICES.replace("Up\"", "Up2\"")),
            ("baseHash", &text(&up, "hash")),
        ],
    )
    .unwrap();
    assert!(moqui_lib::json::to_json(&forked["problems"])
        .to_string()
        .contains("upstream.edit"));
    call(
        &factory,
        "john.doe",
        &format!("{WS}set#UpstreamEdits"),
        &[("enabled", "false")],
    )
    .unwrap();

    // paths stay in their root
    for bad in ["../../etc/passwd", ".git/config", "/etc/passwd"] {
        let err = call(
            &factory,
            "john.doe",
            &format!("{WS}get#File"),
            &[("rootId", "my-site"), ("path", bad)],
        )
        .unwrap_err();
        assert!(!err.is_empty(), "{bad}");
    }

    // a CRLF file keeps its line endings through a save
    std::fs::write(site.join("data.txt"), "one\r\ntwo\r\n").unwrap();
    let crlf = call(
        &factory,
        "john.doe",
        &format!("{WS}get#File"),
        &[("rootId", "my-site"), ("path", "data.txt")],
    )
    .unwrap();
    assert_eq!(text(&crlf, "lineEnding"), "CRLF");
    call(
        &factory,
        "john.doe",
        &format!("{WS}save#File"),
        &[
            ("rootId", "my-site"),
            ("path", "data.txt"),
            ("text", "one\ntwo\nthree\n"),
            ("baseHash", &text(&crlf, "hash")),
        ],
    )
    .unwrap();
    assert_eq!(
        std::fs::read(site.join("data.txt")).unwrap(),
        b"one\r\ntwo\r\nthree\r\n"
    );

    // every write is audited
    let audits = {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        factory
            .entity()
            .unwrap()
            .find("moqui.dev.DevAideAudit")
            .unwrap()
            .list()
            .unwrap()
            .len()
    };
    assert!(audits >= 4, "{audits} audit rows");

    // commit: the user as author, the server as committer, the task trailer
    call(
        &factory,
        "john.doe",
        &format!("{VCS}stage#Files"),
        &[("rootId", "my-site"), ("paths", path)],
    )
    .unwrap();
    let status = call(
        &factory,
        "john.doe",
        &format!("{VCS}get#Status"),
        &[("rootId", "my-site")],
    )
    .unwrap();
    assert_eq!(text(&status, "authorName"), "John Doe");
    let committed = call(
        &factory,
        "john.doe",
        &format!("{VCS}commit#Changes"),
        &[("rootId", "my-site"), ("summary", "Add a service"), ("task", "PROJ-12")],
    )
    .unwrap();
    let id = text(&committed, "commitId");
    assert_eq!(
        git(&site, &["log", "-1", "--format=%an|%cn|%s"]).trim(),
        "John Doe|Dev Server|Add a service"
    );
    assert!(git(&site, &["log", "-1", "--format=%B"]).contains("Task: PROJ-12"));
    assert_eq!(git(&site, &["rev-parse", "HEAD"]).trim(), id);

    // Publish: the rest of the site's changes committed and pushed to the remote
    let published = call(
        &factory,
        "john.doe",
        &format!("{VCS}publish#Site"),
        &[("rootId", "my-site")],
    )
    .unwrap();
    assert_eq!(text(&published, "published"), "true", "{published:?}");
    let remote = gate.runtime.join("site-remote.git");
    assert_eq!(
        git(&remote, &["rev-parse", "main"]).trim(),
        git(&site, &["rev-parse", "HEAD"]).trim()
    );
    assert!(git(&site, &["log", "-1", "--format=%s"]).starts_with("Customize: data.txt"));

    // with a publish branch an admin set for the user, Publish pushes there and says which branch to merge
    {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        moqui_core::user::set_preference("DEV_AIDE_PUBLISH_BRANCH", Some("publish/{user}")).unwrap();
    }
    std::fs::write(site.join("data.txt"), "four\r\n").unwrap();
    let branched = call(
        &factory,
        "john.doe",
        &format!("{VCS}publish#Site"),
        &[("rootId", "my-site")],
    )
    .unwrap();
    assert_eq!(text(&branched, "pushedBranch"), "publish/john.doe");
    assert!(
        text(&branched, "message").contains("merge publish/john.doe into main"),
        "{branched:?}"
    );
    assert_eq!(
        git(&remote, &["rev-parse", "publish/john.doe"]).trim(),
        git(&site, &["rev-parse", "HEAD"]).trim()
    );

    // a shared server with its network git off: the push is refused, nothing goes out
    moqui_lib::env::set_property("dev_aide_shared", "true");
    let shared = call(
        &factory,
        "john.doe",
        &format!("{VCS}push#Remote"),
        &[("rootId", "my-site")],
    )
    .unwrap_err();
    moqui_lib::env::set_property("dev_aide_shared", "false");
    assert!(shared.contains("off on this shared dev server"), "{shared}");
}

const UPSTREAM_SCREEN: &str = r#"<screen>
    <widgets>
        <container-box name="Box"><box-body>
            <form-single name="Facts"><field name="upId"><default-field><display/></default-field></field></form-single>
        </box-body></container-box>
    </widgets>
</screen>
"#;
const UPSTREAM_ENTITIES: &str = r#"<entities>
    <entity entity-name="UpThing" package="upapp"><field name="upId" type="id" is-pk="true"/></entity>
</entities>
"#;
const SITE_EXTEND_SCREEN: &str = r#"<screen-extend>
    <form-single name="Facts"><field name="extra" after="upId"><default-field><display/></default-field></field></form-single>
    <widgets-extend name="Box" where="append"><label text="Site"/></widgets-extend>
</screen-extend>
"#;
const SITE_ENTITIES: &str = r#"<entities>
    <extend-entity entity-name="UpThing" package="upapp"><field name="extra" type="text-short"/></extend-entity>
</entities>
"#;
const SITE_CALLS: &str = r#"<services>
    <service verb="get" noun="Thing">
        <out-parameters><parameter name="result"/></out-parameters>
        <actions><set field="result" value="thing"/></actions>
    </service>
    <service verb="get" noun="Outer">
        <out-parameters><parameter name="result"/></out-parameters>
        <actions><service-call name="mysite.Things.get#Thing" out-map="inner"/><set field="result" from="inner.result"/></actions>
    </service>
</services>
"#;

/// The artifact index on a dev instance (plans/design-dev-aide.md P3): built into its rows, kept per file by each reload,
/// an unchanged file not read again (as after a restart, which reads the ids back from the rows), the site's
/// extensions with their state, a renamed anchor in an upstream screen showing its extension broken after the reload,
/// and a call observed on the artifact stack joining the references.
#[test]
fn the_index_follows_reloads_and_extensions() {
    let security = include_str!("../../../../runtime/dev-component/dev-aide/data/DevAideSecurityData.xml");
    let dev_aide = std::fs::canonicalize(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/dev-component/dev-aide"
    ))
    .unwrap();
    let files = [
        ("component/my-site/component.xml", SITE_COMPONENT),
        ("component/my-site/service/mysite/Things.xml", SITE_CALLS),
        ("component/my-site/entity/SiteEntities.xml", SITE_ENTITIES),
        (
            "component/my-site/screen-extend/upstream-app/screen/UpApp/Detail.xml",
            SITE_EXTEND_SCREEN,
        ),
        ("component/upstream-app/component.xml", UPSTREAM_COMPONENT),
        ("component/upstream-app/entity/UpEntities.xml", UPSTREAM_ENTITIES),
        ("component/upstream-app/screen/UpApp/Detail.xml", UPSTREAM_SCREEN),
    ];
    let gate = start_with_files_and_links(
        "dev-aide-index",
        Some(security),
        &[("instance_purpose", "dev"), ("dev_aide_site_component", "my-site")],
        &files,
        &[(dev_aide.to_str().unwrap(), "dev-component/dev-aide")],
    );
    let factory = Arc::clone(gate.server.factory());
    const DT: &str = "org.moqui.impl.DevToolsServices.";
    let json = |out: &BTreeMap<String, Value>, key: &str| moqui_lib::json::to_json(&out[key]);

    // the build: every component file, timed
    let built = call(
        &factory,
        "john.doe",
        &format!("{DT}rebuild#ArtifactIndex"),
        &[("full", "true")],
    )
    .unwrap();
    let report = json(&built, "report");
    eprintln!("artifact index build: {report}");
    assert!(report["read"].as_u64().unwrap() > 10, "{report}");

    // found by name, with its file and line; the site's extensions all resolve
    let found = call(
        &factory,
        "john.doe",
        &format!("{DT}find#Artifacts"),
        &[("text", "UpThing")],
    )
    .unwrap();
    let found = json(&found, "artifacts");
    assert_eq!(found[0]["id"], "entity:upapp.UpThing", "{found}");
    assert!(found[0]["path"].as_str().unwrap().ends_with("UpEntities.xml"));
    let extensions = call(
        &factory,
        "john.doe",
        &format!("{DT}get#ArtifactExtensions"),
        &[("components", "my-site")],
    )
    .unwrap();
    let listed = json(&extensions, "extensions");
    assert_eq!(text(&extensions, "brokenCount"), "0", "{listed}");
    assert!(listed.as_array().unwrap().len() >= 4, "{listed}");
    let users = call(
        &factory,
        "john.doe",
        &format!("{DT}get#ArtifactRefs"),
        &[("artifactId", "entity:upapp.UpThing")],
    )
    .unwrap();
    let users = json(&users, "refs");
    assert!(
        users
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["refType"] == "extend" && r["other"]["component"] == "my-site"),
        "{users}"
    );

    // a restart reads nothing again: the rows and their hashes are kept
    let again = call(&factory, "john.doe", &format!("{DT}rebuild#ArtifactIndex"), &[]).unwrap();
    let again = json(&again, "report");
    assert_eq!(again["read"], 0, "{again}");

    // the upstream screen renames its form: the reload replaces that file's rows, and the site's extension of the old
    // name is broken
    let detail = gate.runtime.join("component/upstream-app/screen/UpApp/Detail.xml");
    std::fs::write(&detail, UPSTREAM_SCREEN.replace("name=\"Facts\"", "name=\"Facts2\"")).unwrap();
    let started = std::time::Instant::now();
    moqui_core::devtools::reload_paths(&factory, std::slice::from_ref(&detail));
    eprintln!(
        "artifact index: reload with its per-file update in {} ms",
        started.elapsed().as_millis()
    );
    let broken = call(
        &factory,
        "john.doe",
        &format!("{DT}get#ArtifactExtensions"),
        &[("components", "my-site")],
    )
    .unwrap();
    let listed = json(&broken, "extensions");
    assert_eq!(text(&broken, "brokenCount"), "1", "{listed}");
    assert_eq!(listed[0]["target"], "Facts");
    assert_eq!(listed[0]["refType"], "anchor");
    // and back: the anchor resolves again
    std::fs::write(&detail, UPSTREAM_SCREEN).unwrap();
    moqui_core::devtools::reload_paths(&factory, &[detail]);
    let fixed = call(
        &factory,
        "john.doe",
        &format!("{DT}get#ArtifactExtensions"),
        &[("components", "my-site")],
    )
    .unwrap();
    assert_eq!(text(&fixed, "brokenCount"), "0");

    // a call observed on the artifact stack: get#Outer calls get#Thing, counted and written
    let outer = run(&factory, "mysite.Things.get#Outer");
    assert_eq!(text(&outer, "result"), "thing");
    moqui_core::devtools::index::calls::flush(&factory).unwrap();
    let callers = call(
        &factory,
        "john.doe",
        &format!("{DT}get#ArtifactRefs"),
        &[("artifactId", "service:mysite.Things.get#Thing")],
    )
    .unwrap();
    let callers = json(&callers, "refs");
    let from_outer = callers
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["from"] == "service:mysite.Things.get#Outer")
        .unwrap_or_else(|| panic!("{callers}"));
    assert!(from_outer["calls"].as_i64().unwrap() >= 1, "{callers}");
}

/// The index's scale (plans/design-dev-aide.md 7.1, question 11): this repository's components (base-component and
/// component) and a synthetic corpus ten times their size, each built into the rows, with a per-file update and a find
/// timed. Run once by hand and the numbers recorded in the design:
/// `cargo test -p moqui-host --features dev --test it devaide::the_index_scales -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, run by hand"]
fn the_index_scales() {
    let security = include_str!("../../../../runtime/dev-component/dev-aide/data/DevAideSecurityData.xml");
    let dev_aide = std::fs::canonicalize(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/dev-component/dev-aide"
    ))
    .unwrap();
    let gate = start_with_files_and_links(
        "dev-aide-scale",
        Some(security),
        &[("instance_purpose", "dev")],
        &[],
        &[(dev_aide.to_str().unwrap(), "dev-component/dev-aide")],
    );
    let factory = Arc::clone(gate.server.factory());
    let repository_runtime = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../runtime"));
    let mut corpus = Vec::new();
    for parent in ["base-component", "component"] {
        let Ok(entries) = std::fs::read_dir(repository_runtime.join(parent)) else {
            continue;
        };
        for entry in entries
            .flatten()
            .filter(|entry| entry.path().join("component.xml").is_file())
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            corpus.extend(moqui_core::devtools::index::directory_files(&name, &entry.path()));
        }
    }
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    use moqui_core::devtools::index;
    let measure = |label: &str, files: Vec<index::Located>| {
        index::clear(&factory).unwrap();
        let count = files.len();
        let some = files
            .iter()
            .find(|file| file.location.ends_with("OrderEntities.xml"))
            .unwrap_or(&files[count / 2])
            .clone();
        let report = index::build_files(&factory, files, false).unwrap();
        let started = std::time::Instant::now();
        let update = index::update_located(&factory, &[some]).unwrap();
        let update_millis = started.elapsed().as_millis();
        let started = std::time::Instant::now();
        let found = index::find(&factory, "OrdHdr", &[], &[], 20).unwrap();
        let find_millis = started.elapsed().as_millis();
        eprintln!(
            "{label}: {count} files, {} artifacts, {} references; build {} ms (read {} ms, write {} ms); per-file update {update_millis} ms ({} artifacts); find {find_millis} ms ({} found)",
            report.artifacts, report.refs, report.millis, report.read_millis, report.write_millis, update.artifacts, found.len()
        );
    };
    measure("corpus", corpus.clone());
    let synthetic = index::synthetic(&corpus, &gate.runtime.join("synthetic"), 10).unwrap();
    measure("synthetic x10", synthetic);
}

/// F14 (plans/design-dev-aide.md 13.3, 13.5): a screen renders with proposed text standing at a location for the renders
/// in the overlay, a new `screen-extend` file merged into its base screen; outside the overlay the screen is as saved,
/// and the cached definition is not the overlaid one.
#[test]
fn a_screen_renders_with_proposed_text_in_an_overlay() {
    let screen = r#"<screen require-authentication="false">
    <widgets>
        <form-single name="Facts"><field name="upId"><default-field title="Up Id"><display/></default-field></field></form-single>
    </widgets>
</screen>
"#;
    let files = [
        ("component/my-site/component.xml", SITE_COMPONENT),
        ("component/upstream-app/component.xml", UPSTREAM_COMPONENT),
        ("component/upstream-app/screen/UpApp/Detail.xml", screen),
    ];
    let gate = start_with_files_and_links("dev-aide-overlay", None, &[("instance_purpose", "dev")], &files, &[]);
    let factory = Arc::clone(gate.server.factory());
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let render = || {
        moqui_core::screen::ScreenTest::new(&factory)
            .root_screen("component://upstream-app/screen/UpApp/Detail.xml")
            .render("", BTreeMap::new())
            .unwrap()
    };
    let before = render();
    let text = |render: &moqui_core::screen::TestRender| render.text();
    assert!(!text(&before).contains("Extra Note"), "{}", text(&before));
    let mut overlay = moqui_core::screen::overlay::Overlay::new();
    overlay.insert_location(
        &factory,
        "component://my-site/screen-extend/upstream-app/screen/UpApp/Detail.xml",
        "<screen-extend><form-single name=\"Facts\"><field name=\"extraNote\" after=\"upId\"><default-field title=\"Extra Note\"><display/></default-field></field></form-single></screen-extend>",
    );
    let previewed = moqui_core::screen::overlay::with(overlay, render);
    assert!(text(&previewed).contains("Extra Note"), "{}", text(&previewed));
    // nothing was written, and the next render is as saved
    assert!(!gate.runtime.join("component/my-site/screen-extend").exists());
    assert!(!text(&render()).contains("Extra Note"));
}

const ENTITY: &str = "org.moqui.devaide.EntityServices.";

/// The entity modeler's save (plans/design-dev-aide.md P5, F5): the entity view of an upstream entity names the site's
/// entity file as where its edits go; a save of the site's `extend-entity` relinks the model at once and adds the
/// column, no restart; a file that does not parse keeps the model there was, with "restart to apply" and the problem;
/// a site's seed data file loads.
#[test]
fn an_entity_save_relinks_and_adds_its_column() {
    let upstream_entities = r#"<entities>
    <entity entity-name="Widget" package="upapp">
        <field name="widgetId" type="id" is-pk="true"/>
        <field name="widgetName" type="text-medium"/>
    </entity>
</entities>
"#;
    let site_entities = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<entities>\n    <!-- the site's additions -->\n</entities>\n";
    let dev_aide = std::fs::canonicalize(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/dev-component/dev-aide"
    ))
    .unwrap();
    let files = [
        ("component/my-site/component.xml", SITE_COMPONENT),
        ("component/my-site/entity/MySiteEntities.xml", site_entities),
        ("component/upstream-app/component.xml", UPSTREAM_COMPONENT),
        ("component/upstream-app/entity/UpEntities.xml", upstream_entities),
    ];
    let gate = start_with_files_and_links(
        "dev-aide-entity",
        Some(include_str!("../../../../runtime/dev-component/dev-aide/data/DevAideSecurityData.xml")),
        &[("instance_purpose", "dev"), ("dev_aide_site_component", "my-site")],
        &files,
        &[(dev_aide.to_str().unwrap(), "dev-component/dev-aide")],
    );
    let factory = Arc::clone(gate.server.factory());
    let facade = factory.entity().unwrap();
    let columns = |facade: &moqui_core::entity::EntityFacade| -> Vec<String> {
        facade
            .table_columns("upapp.Widget")
            .unwrap()
            .into_iter()
            .map(|(name, _)| name.to_ascii_lowercase())
            .collect()
    };
    assert!(!columns(&facade).contains(&"due_date".to_string()));

    // the view of the upstream entity: its edits go to the site's entity file
    let view = call(&factory, "john.doe", &format!("{ENTITY}get#EntityView"), &[("artifactId", "entity:upapp.Widget")]).unwrap();
    let view = moqui_lib::json::to_json(&view["view"]);
    assert_eq!(view["upstream"], true, "{view}");
    assert_eq!(view["target"]["rootId"], "my-site", "{view}");
    assert_eq!(view["target"]["path"], "entity/MySiteEntities.xml");
    assert_eq!(view["target"]["exists"], true);
    assert_eq!(view["seedFile"]["path"], "data/MySiteSeedData.xml");
    assert!(view["fields"].as_array().unwrap().iter().any(|field| field["name"] == "widgetName" && field["component"] == "upstream-app"), "{view}");

    // the save of the extension: relinked, the column added, no restart
    let path = "entity/MySiteEntities.xml";
    let base = text(&call(&factory, "john.doe", &format!("{WS}get#File"), &[("rootId", "my-site"), ("path", path)]).unwrap(), "hash");
    let extended = site_entities.replace(
        "</entities>",
        "    <extend-entity entity-name=\"Widget\" package=\"upapp\">\n        <field name=\"dueDate\" type=\"date\"/>\n    </extend-entity>\n</entities>",
    );
    let saved = call(
        &factory,
        "john.doe",
        &format!("{WS}save#File"),
        &[("rootId", "my-site"), ("path", path), ("text", &extended), ("baseHash", &base)],
    )
    .unwrap();
    let report = moqui_lib::json::to_json(&saved["reload"]);
    let entity_file = &report["files"][0];
    assert_eq!(saved["restart"], Value::Bool(false), "{report}");
    assert!(entity_file["summary"].as_str().unwrap().contains("relinked"), "{report}");
    assert!(entity_file["ddl"].as_array().unwrap().iter().any(|sql| sql.as_str().unwrap().contains("due_date")), "{report}");
    assert!(columns(&facade).contains(&"due_date".to_string()));
    {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let mut widget = facade.make_value("upapp.Widget").unwrap();
        widget.set("widgetId", Value::string("W1"));
        widget.set("dueDate", Value::string("2026-11-01"));
        widget.create(&facade).unwrap();
        let found = facade.find("upapp.Widget").unwrap().eq("widgetId", Value::string("W1")).one().unwrap().unwrap();
        assert!(!matches!(found.fields()["dueDate"], Value::Null), "the new column holds the date");
    }

    // a file that does not parse keeps the model there was
    let base = text(&saved, "hash");
    let broken = call(
        &factory,
        "john.doe",
        &format!("{WS}save#File"),
        &[("rootId", "my-site"), ("path", path), ("text", "<entities><extend-entity entity-name="), ("baseHash", &base)],
    )
    .unwrap();
    assert_eq!(broken["restart"], Value::Bool(true));
    assert!(moqui_lib::json::to_json(&broken["problems"]).to_string().contains("reload.entity"));
    assert!(facade
        .with_entity_table("upapp.Widget")
        .is_some());
    let fields = moqui_core::entity::info::definition_info(&facade, "upapp.Widget").unwrap();
    assert!(moqui_lib::json::to_json(&Value::Map(fields)).to_string().contains("dueDate"));

    // a site's seed data loads
    let seed = "<entity-facade-xml type=\"seed\">\n    <moqui.basic.EnumerationType enumTypeId=\"WidgetPriority\" description=\"Priority\"/>\n    <moqui.basic.Enumeration enumId=\"WpHigh\" enumTypeId=\"WidgetPriority\" description=\"High\"/>\n</entity-facade-xml>\n";
    call(
        &factory,
        "john.doe",
        &format!("{WS}save#File"),
        &[("rootId", "my-site"), ("path", "data/MySiteSeedData.xml"), ("text", seed), ("baseHash", "")],
    )
    .unwrap();
    let loaded = call(
        &factory,
        "john.doe",
        &format!("{ENTITY}load#SeedFile"),
        &[("rootId", "my-site"), ("path", "data/MySiteSeedData.xml")],
    )
    .unwrap();
    assert_eq!(text(&loaded, "records"), "2");
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    assert!(facade
        .find("moqui.basic.Enumeration")
        .unwrap()
        .eq("enumId", Value::string("WpHigh"))
        .one()
        .unwrap()
        .is_some());
}
