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

//! The Dev AIDE with a group server (plans/design-dev-aide.md 21.10, P11), the instance its own group server over
//! loopback: the site component's working tree has the instance's hosted `my-site` repository as its origin. Connect
//! signs in as an installed app does (F17: `/sso/appLogin` with no identity provider, this server's own login, the
//! confirmation, the code at `/sso/appToken` for a login key), then sets the addons repository, the task source and
//! git's credential; the working tree's remote resolves to its hosted repository; a pushed branch opens a merge request
//! through the framework's REST (F9); a user who may only view proposes through `refs/for/main`; a merge request is
//! checked out as a branch; Publish to a publish branch opens one; Remove takes back what Connect set. A Smithy server's
//! side of the same is its REST contract only (the dev-aide crate's `hosting::tests`): mr-pass and mr-mantle do not
//! load here.

#![cfg(all(
    unix,
    feature = "embedded-postgres",
    feature = "axum",
    feature = "devtools",
    feature = "git"
))]

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use base64::Engine;
use moqui_core::context::ExecutionContextFactory;
use moqui_lib::object::{to_plain_string, Value};
use sha2::Digest;

use crate::common::{exchange, http_request, merge_cookies, start_with_files_and_links};

const SITE_COMPONENT: &str = r#"<component name="my-site"/>"#;
const SITE_SERVICES: &str = "<services>\n    <service verb=\"get\" noun=\"Thing\"/>\n</services>\n";
const HS: &str = "org.moqui.devaide.HostingServices.";
const VCS: &str = "org.moqui.devaide.VcsServices.";

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// git kept from the desktop's config and credentials, for the server and this test alike.
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

fn json(out: &BTreeMap<String, Value>, key: &str) -> serde_json::Value {
    out.get(key).map(moqui_lib::json::to_json).unwrap_or_default()
}

fn text(out: &BTreeMap<String, Value>, key: &str) -> String {
    out.get(key).map(to_plain_string).unwrap_or_default()
}

/// A login key of `user`, as the server's Git access page makes one.
fn login_key(factory: &Arc<ExecutionContextFactory>, user: &str) -> String {
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user(user).unwrap();
    let key = moqui_core::user::get_login_key(Some(2.0)).unwrap();
    moqui_core::user::logout().unwrap();
    key
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> &'a str {
    headers
        .iter()
        .find(|(have, _)| have.eq_ignore_ascii_case(name))
        .map_or("", |(_, value)| value.as_str())
}

/// The browser's side of an installed app's sign-in at this server's own login (F17), as john.doe: the code the
/// confirmation hands to `redirect`.
fn browser_sign_in(address: SocketAddr, challenge: &str, redirect: &str) -> String {
    let query = format!(
        "codeChallenge={challenge}&codeChallengeMethod=S256&state=st-1&redirectUri={}&deviceName=Dev%20AIDE",
        redirect.replace(':', "%3A").replace('/', "%2F")
    );
    let (status, headers, body) = exchange(
        address,
        &http_request("GET", &format!("/sso/appLogin?{query}"), &[], ""),
    );
    assert_eq!(status, 302, "{body}");
    assert!(header(&headers, "location").contains("/Login?returnTo="), "{headers:?}");
    let cookie = merge_cookies("", &headers);
    let (status, headers, body) = exchange(
        address,
        &http_request(
            "POST",
            "/Login/login",
            &[
                ("Cookie", &cookie),
                ("Content-Type", "application/x-www-form-urlencoded"),
            ],
            "username=john.doe&password=moqui",
        ),
    );
    assert_eq!(status, 302, "{body}");
    let cookie = merge_cookies(&cookie, &headers);
    let (status, headers, page) = exchange(
        address,
        &http_request("GET", "/sso/appCode", &[("Cookie", &cookie)], ""),
    );
    assert_eq!(status, 200, "{page}");
    assert!(page.contains("Sign in to Dev AIDE as john.doe"), "{page}");
    let cookie = merge_cookies(&cookie, &headers);
    let marker = "name=\"moquiSessionToken\" value=\"";
    let start = page.find(marker).expect("the confirmation has the session token") + marker.len();
    let token = &page[start..start + page[start..].find('"').unwrap()];
    let (status, headers, body) = exchange(
        address,
        &http_request(
            "POST",
            "/sso/appCodeConfirm",
            &[
                ("Cookie", &cookie),
                ("Content-Type", "application/x-www-form-urlencoded"),
            ],
            &format!("allow=true&moquiSessionToken={token}"),
        ),
    );
    assert_eq!(status, 302, "{body}");
    let location = header(&headers, "location");
    assert!(
        location.starts_with(redirect) && location.contains("state=st-1"),
        "{location}"
    );
    let code = location.split("code=").nth(1).unwrap().split('&').next().unwrap();
    code.to_string()
}

#[test]
fn a_dev_instance_connects_to_its_group_server_and_opens_merge_requests() {
    if !crate::common::own_process() {
        return;
    }
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("skipped: git is not installed");
        return;
    }
    let data = include_str!("../../../../runtime/components/dev/dev-aide/data/DevAideSecurityData.xml").replace(
        "</entity-facade-xml>",
        r#"
    <moqui.resource.GitRepository repositoryId="my-site" repositoryKind="db" description="The site component, hosted"/>
    <moqui.security.UserAccount userId="DEV_VIEWER" username="dev.viewer" userFullName="Dev Viewer" emailAddress="viewer@example.com"/>
    <moqui.security.UserGroupMember userGroupId="DEV_AIDE_EXTENDERS" userId="DEV_VIEWER" fromDate="0"/>
    <moqui.security.UserGroup userGroupId="MY_SITE_VIEWERS" description="May view my-site"/>
    <moqui.security.UserGroupMember userGroupId="MY_SITE_VIEWERS" userId="DEV_VIEWER" fromDate="0"/>
    <moqui.security.ArtifactGroup artifactGroupId="GIT_MY_SITE" description="my-site"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="GIT_MY_SITE" artifactName="my-site" artifactTypeEnumId="AT_GIT_REPOSITORY"/>
    <moqui.security.ArtifactAuthz artifactAuthzId="GIT_MY_SITE_VIEW" userGroupId="MY_SITE_VIEWERS" artifactGroupId="GIT_MY_SITE"
        authzTypeEnumId="AUTHZT_ALLOW" authzActionEnumId="AUTHZA_VIEW"/>
</entity-facade-xml>"#,
    );
    let dev_aide = std::fs::canonicalize(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/components/dev/dev-aide"
    ))
    .unwrap();
    let files = [
        ("components/addon/my-site/component.xml", SITE_COMPONENT),
        ("components/addon/my-site/service/mysite/Things.xml", SITE_SERVICES),
    ];
    let gate = start_with_files_and_links(
        "dev-aide-group",
        Some(&data),
        &[
            ("instance_purpose", "dev"),
            ("dev_aide_site_component", "my-site"),
            ("dev_aide_shared", "false"),
        ],
        &files,
        &[(dev_aide.to_str().unwrap(), "components/dev/dev-aide")],
    );
    let factory = Arc::clone(gate.server.factory());
    let address = gate.server.address();
    let base = format!("http://127.0.0.1:{}", address.port());
    let site = gate.runtime.join("components/addon/my-site");
    isolate_git(&gate.runtime);

    // the working tree, whose origin is the instance's own hosted repository
    let john_key = login_key(&factory, "john.doe");
    git(&site, &["init", "-q", "-b", "main"]);
    git(&site, &["add", "-A"]);
    git(&site, &["commit", "-q", "-m", "Start"]);
    let with_key = format!(
        "http://john.doe:{john_key}@127.0.0.1:{}/git/my-site.git",
        address.port()
    );
    git(&site, &["push", "-q", &with_key, "main"]);
    git(&site, &["remote", "add", "origin", &format!("{base}/git/my-site.git")]);
    git(&site, &["fetch", "-q", &with_key, "main:refs/remotes/origin/main"]);
    git(&site, &["branch", "-q", "--set-upstream-to=origin/main", "main"]);

    // Connect: the browser's sign-in at the server's own login, then the code taken for a key (F17)
    let verifier = "dev-aide-verifier-0123456789-0123456789-0123456789".to_string();
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()));
    let redirect = format!("{base}/qapps/dev/Settings/serverSignedIn");
    let code = browser_sign_in(address, &challenge, &redirect);
    {
        // what start#Connect keeps, which needs the page's request for this instance's own address
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let facade = factory.entity().unwrap();
        let mut row = facade.make_value("moqui.dev.DevAideServer").unwrap();
        row.set("serverId", Value::string("GROUP"));
        row.set("userId", Value::string("EX_JOHN_DOE"));
        row.set("label", Value::string("Team"));
        row.set("baseUrl", Value::string(&base));
        row.set("pendingState", Value::string("st-1"));
        row.set("pendingVerifier", Value::string(&verifier));
        row.set("pendingRedirect", Value::string(&redirect));
        row.set("pendingDate", moqui_core::user::now_value());
        row.create(&facade).unwrap();
    }
    let finished = call(
        &factory,
        "john.doe",
        &format!("{HS}finish#Connect"),
        &[("state", "st-1"), ("code", &code)],
    )
    .unwrap();
    assert_eq!(text(&finished, "statusText"), "connected", "{finished:?}");
    let servers = call(&factory, "john.doe", &format!("{HS}get#Servers"), &[]).unwrap();
    let server = json(&servers, "servers")[0].clone();
    assert_eq!(server["username"], "john.doe", "{server}");
    assert_eq!(server["hasToken"], true);
    assert!(
        server["offer"]["git"]["httpsUrlTemplate"]
            .as_str()
            .unwrap()
            .ends_with("/git/${name}.git"),
        "{server}"
    );

    // what Connect sets, as confirmed: the addons repository, the task source, git's credential
    let applied = call(
        &factory,
        "john.doe",
        &format!("{HS}apply#ServerSetup"),
        &[
            ("serverId", "GROUP"),
            ("addons", "true"),
            ("defaultRepository", "true"),
            ("taskSource", "true"),
            ("gitCredential", "stored"),
        ],
    )
    .unwrap();
    assert_eq!(json(&applied, "done").as_array().unwrap().len(), 3, "{applied:?}");
    let myaddons = std::fs::read_to_string(gate.runtime.join("myaddons.xml")).unwrap();
    assert!(myaddons.contains("<addons default-repository=\"team\">"), "{myaddons}");
    assert!(
        myaddons.contains(&format!("url=\"{base}/git/${{name}}.git\"")),
        "{myaddons}"
    );
    let sources = call(
        &factory,
        "john.doe",
        "org.moqui.devaide.TaskServices.get#TaskSources",
        &[],
    )
    .unwrap();
    assert!(
        json(&sources, "sources")
            .as_array()
            .unwrap()
            .iter()
            .any(|source| source["label"] == "Team"),
        "{sources:?}"
    );

    // the working tree's hosted repository, with john.doe's rights
    let hosted = call(
        &factory,
        "john.doe",
        &format!("{HS}resolve#RootHosting"),
        &[("rootId", "my-site")],
    )
    .unwrap();
    let repository = json(&hosted, "hosted")["repository"].clone();
    assert_eq!(repository["repositoryId"], "my-site", "{hosted:?}");
    assert_eq!(repository["access"]["canUpdate"], true);
    assert_eq!(repository["defaultBranch"], "main");

    // Push with a merge request, through git's stored credential and the framework's REST
    git(&site, &["switch", "-q", "-c", "due-date"]);
    std::fs::write(
        site.join("service/mysite/Things.xml"),
        SITE_SERVICES.replace("Thing", "DueDate"),
    )
    .unwrap();
    git(&site, &["commit", "-q", "-am", "Add a due date"]);
    let pushed = call(
        &factory,
        "john.doe",
        &format!("{VCS}push#Remote"),
        &[("rootId", "my-site"), ("openMergeRequest", "true")],
    )
    .unwrap();
    assert_eq!(json(&pushed, "mergeRequest")["mergeRequestNum"], 1, "{pushed:?}");
    let mine = call(
        &factory,
        "john.doe",
        &format!("{HS}get#MergeRequests"),
        &[("rootId", "my-site"), ("filter", "mine")],
    )
    .unwrap();
    let listed = json(&mine, "mergeRequests");
    assert_eq!(listed[0]["title"], "Add a due date", "{listed}");
    assert_eq!(listed[0]["sourceRef"], "due-date");

    // a user who may only view proposes through refs/for/main (push to review), with their own connection
    let viewer_key = login_key(&factory, "dev.viewer");
    let stored = call(
        &factory,
        "dev.viewer",
        &format!("{HS}store#ServerToken"),
        &[("baseUrl", &base), ("label", "Team"), ("token", &viewer_key)],
    )
    .unwrap();
    let viewer_server = text(&stored, "serverId");
    assert_eq!(text(&stored, "statusText"), "connected");
    call(
        &factory,
        "dev.viewer",
        &format!("{HS}apply#ServerSetup"),
        &[("serverId", &viewer_server), ("gitCredential", "stored")],
    )
    .unwrap();
    let seen = call(
        &factory,
        "dev.viewer",
        &format!("{HS}resolve#RootHosting"),
        &[("rootId", "my-site")],
    )
    .unwrap();
    assert_eq!(
        json(&seen, "hosted")["repository"]["access"]["canCreate"],
        false,
        "{seen:?}"
    );
    git(&site, &["switch", "-q", "main"]);
    git(&site, &["switch", "-q", "-c", "viewer-fix"]);
    std::fs::write(site.join("README.md"), "A fix\n").unwrap();
    git(&site, &["add", "README.md"]);
    git(&site, &["commit", "-q", "-m", "A viewer's fix"]);
    let proposed_head = git(&site, &["rev-parse", "HEAD"]);
    let proposed = call(
        &factory,
        "dev.viewer",
        &format!("{HS}propose#Changes"),
        &[
            ("rootId", "my-site"),
            ("topic", "viewer-fix"),
            ("title", "A viewer's fix"),
        ],
    )
    .unwrap();
    assert_eq!(text(&proposed, "target"), "main", "{proposed:?}");
    let open = call(
        &factory,
        "john.doe",
        &format!("{HS}get#MergeRequests"),
        &[("rootId", "my-site")],
    )
    .unwrap();
    let proposal = json(&open, "mergeRequests")
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["title"] == "A viewer's fix")
        .cloned()
        .unwrap_or_else(|| panic!("{open:?}"));
    assert_eq!(proposal["pushedForReview"], true);

    // the proposal checked out as a branch of its own
    git(&site, &["switch", "-q", "main"]);
    let number = proposal["mergeRequestNum"].to_string();
    let checked = call(
        &factory,
        "john.doe",
        &format!("{HS}checkout#MergeRequest"),
        &[("rootId", "my-site"), ("mergeRequestNum", &number)],
    )
    .unwrap();
    let branch = text(&checked, "branch");
    assert_eq!(branch, format!("mr/{number}"));
    assert_eq!(git(&site, &["rev-parse", &branch]), proposed_head);

    // Publish to a publish branch opens the merge request into the current branch
    git(&site, &["switch", "-q", "main"]);
    call(
        &factory,
        "john.doe",
        "org.moqui.devaide.WorkspaceServices.store#Root",
        &[
            ("location", site.to_str().unwrap()),
            ("publishBranch", "publish/{user}"),
        ],
    )
    .unwrap();
    std::fs::write(
        site.join("service/mysite/Things.xml"),
        SITE_SERVICES.replace("Thing", "Published"),
    )
    .unwrap();
    let published = call(
        &factory,
        "john.doe",
        &format!("{VCS}publish#Site"),
        &[("rootId", "my-site"), ("summary", "Publish a change")],
    )
    .unwrap();
    assert!(
        text(&published, "message").contains("opened merge request"),
        "{published:?}"
    );
    assert_eq!(json(&published, "mergeRequest")["target"], "main");

    // Remove takes back what Connect set
    let removed = call(
        &factory,
        "john.doe",
        &format!("{HS}remove#Server"),
        &[("serverId", "GROUP")],
    )
    .unwrap();
    let removed = json(&removed, "removed");
    assert!(
        removed.as_array().unwrap().iter().any(|part| part == "the task source"),
        "{removed}"
    );
    let myaddons = std::fs::read_to_string(gate.runtime.join("myaddons.xml")).unwrap();
    assert!(!myaddons.contains("repository name=\"team\""), "{myaddons}");
}
