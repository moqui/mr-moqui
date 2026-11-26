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
/// The group server's application, as Mantle would be for threads and Smithy for the agreement: a review-thread service
/// over a table of its own, and an agreement check that knows one signed address (plans/design-dev-aide.md 21.10).
const STUB_COMPONENT: &str = r#"<component name="review-stub"/>"#;
const STUB_CONF: &str = r#"<moqui-conf><git-server review-thread-service="reviewstub.Stub.handle#Thread"
    agreement-check-service="reviewstub.Stub.check#Signed"/></moqui-conf>"#;
const STUB_ENTITIES: &str = r#"<entities>
    <entity entity-name="ReviewNote" package="reviewstub">
        <field name="noteId" type="id" is-pk="true"/>
        <field name="threadId" type="id"/>
        <field name="mergeRequestId" type="id"/>
        <field name="path" type="text-medium"/>
        <field name="newLine" type="number-integer"/>
        <field name="oldLine" type="number-integer"/>
        <field name="authorName" type="text-medium"/>
        <field name="body" type="text-long"/>
        <field name="resolved" type="text-indicator"/>
    </entity>
</entities>"#;
const STUB_SERVICES: &str = r#"<services>
    <service verb="handle" noun="Thread">
        <implements service="org.moqui.git.GitServices.handle#GitReviewThread"/>
        <actions><script><![CDATA[
            const E = 'reviewstub.ReviewNote'
            if (operation === 'find') {
                let find = ec.entity.find(E).condition('mergeRequestId', mergeRequestId).disableAuthz()
                if (path) find = find.condition('path', path)
                const byThread = {}
                threadList = []
                for (const note of Array.from(find.orderBy('noteId').list())) {
                    let thread = byThread[note.threadId]
                    if (!thread) {
                        thread = byThread[note.threadId] = {threadId: note.threadId, path: note.path, newLine: note.newLine,
                            oldLine: note.oldLine, resolved: false, commentList: []}
                        threadList.push(thread)
                    }
                    if (note.resolved === 'Y') thread.resolved = true
                    thread.commentList.push({commentId: note.noteId, authorName: note.authorName, body: note.body, bodyFormat: 'markdown'})
                }
            } else if (operation === 'comment') {
                // a reply only to a thread of this merge request (handle#GitReviewThread's rule)
                if (threadId && ec.entity.find(E).condition('threadId', threadId).condition('mergeRequestId', mergeRequestId)
                        .disableAuthz().count() == 0) { ec.message.addError('No thread ' + threadId); return }
                const note = ec.entity.makeValue(E).setAll({mergeRequestId, path, newLine, oldLine, body,
                    authorName: ec.user.username}).setSequencedIdPrimary()
                note.threadId = threadId || note.noteId
                note.create()
                threadId = note.threadId
            } else if (operation === 'resolve') {
                const notes = () => ec.entity.find(E).condition('threadId', threadId).condition('mergeRequestId', mergeRequestId).disableAuthz()
                if (notes().count() == 0) { ec.message.addError('No thread ' + threadId); return }
                notes().updateAll({resolved: resolved ? 'Y' : 'N'})
            }
        ]]></script></actions>
    </service>
    <service verb="get" noun="MyTasks">
        <in-parameters><parameter name="pageSize"/></in-parameters>
        <out-parameters><parameter name="taskList" type="List"/></out-parameters>
        <actions><script>taskList = [{workEffortId: 'STUB_T1', workEffortName: 'A stub task', statusId: 'WeInProgress'}]</script></actions>
    </service>
    <service verb="check" noun="Signed">
        <implements service="org.moqui.git.GitServices.check#GitAuthorSigned"/>
        <actions><script><![CDATA[
            known = true
            required = true
            signed = authorEmail === 'signed@example.com'
        ]]></script></actions>
    </service>
</services>"#;
/// Mantle's REST as the task panel reads it (question 18): `my/tasks` takes a key for tasks alone, `my/noticeCounts` none.
const STUB_REST: &str = r#"<resource name="mantle">
    <resource name="my">
        <resource name="tasks" key-scope="tasks"><method type="get"><service name="reviewstub.Stub.get#MyTasks"/></method></resource>
        <resource name="noticeCounts"><method type="get"><service name="reviewstub.Stub.get#MyTasks"/></method></resource>
    </resource>
</resource>"#;
const AUTHORS: &str = "# Authors\n\n## Copyright Waiver\n\nText.\n\n- Written in 2026 by Ann A - ann\n\n## Grant of Patent License\n\nText.\n\n- Written in 2026 by Ann A - ann\n";
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
/// confirmation hands to `redirect`, and the browser's cookie, signed in.
fn browser_sign_in(address: SocketAddr, challenge: &str, redirect: &str) -> (String, String) {
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
    assert!(
        page.contains("calls itself \u{201c}Dev AIDE\u{201d}") && page.contains("as john.doe?"),
        "{page}"
    );
    let cookie = merge_cookies(&cookie, &headers);
    let token = form_field(&page, "moquiSessionToken");
    let pending = form_field(&page, "pendingId");
    let (status, headers, body) = exchange(
        address,
        &http_request(
            "POST",
            "/sso/appCodeConfirm",
            &[
                ("Cookie", &cookie),
                ("Content-Type", "application/x-www-form-urlencoded"),
            ],
            &format!("allow=true&moquiSessionToken={token}&pendingId={pending}"),
        ),
    );
    assert_eq!(status, 302, "{body}");
    let location = header(&headers, "location");
    assert!(
        location.starts_with(redirect) && location.contains("state=st-1"),
        "{location}"
    );
    let code = location.split("code=").nth(1).unwrap().split('&').next().unwrap();
    (code.to_string(), merge_cookies(&cookie, &headers))
}

/// A hidden field of a page's form: the session token, the pending sign-in it was shown for.
fn form_field(page: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\" value=\"");
    let start = page
        .find(&marker)
        .unwrap_or_else(|| panic!("the page has {name}: {page}"))
        + marker.len();
    page[start..start + page[start..].find('"').unwrap()].to_string()
}

/// A browser signed in already starts another app sign-in at this server's own login (F17, item 7 of the P11 review):
/// it stays signed in, the confirmation names the port the code goes to, is never framed, and asks for the password;
/// a `localhost` redirect is refused. Its confirmation answers only the sign-in it was shown for, and an account with no
/// password here signs out and in again from it, the app sign-in kept.
fn signed_in_browser_confirms_who_it_is(address: SocketAddr, cookie: &str, challenge: &str, redirect: &str) {
    let start = |redirect: &str| {
        let query = format!(
            "codeChallenge={challenge}&codeChallengeMethod=S256&state=st-2&redirectUri={}&deviceName=Other%20app",
            redirect.replace(':', "%3A").replace('/', "%2F")
        );
        exchange(
            address,
            &http_request("GET", &format!("/sso/appLogin?{query}"), &[("Cookie", cookie)], ""),
        )
    };
    let (status, _, body) = start(&redirect.replace("127.0.0.1", "localhost"));
    assert_eq!(status, 400, "a localhost redirect: {body}");
    let (status, headers, body) = start(redirect);
    assert_eq!(status, 302, "{body}");
    assert_eq!(
        header(&headers, "location"),
        "/sso/appCode",
        "not signed out: {headers:?}"
    );
    let cookie = merge_cookies(cookie, &headers);
    let code_page = |cookie: &str| exchange(address, &http_request("GET", "/sso/appCode", &[("Cookie", cookie)], ""));
    let (status, headers, page) = code_page(&cookie);
    assert_eq!(status, 200, "{page}");
    assert_eq!(header(&headers, "x-frame-options"), "DENY");
    assert!(header(&headers, "content-security-policy").contains("frame-ancestors 'none'"));
    let port = redirect.split(':').nth(2).unwrap().split('/').next().unwrap();
    assert!(
        page.contains(&format!("port {port}")) && page.contains("calls itself"),
        "{page}"
    );
    assert!(page.contains("type=\"password\""), "{page}");
    let cookie = merge_cookies(&cookie, &headers);
    let token = form_field(&page, "moquiSessionToken");
    let first = form_field(&page, "pendingId");
    let post = |cookie: &str, body: &str| {
        exchange(
            address,
            &http_request(
                "POST",
                "/sso/appCodeConfirm",
                &[
                    ("Cookie", cookie),
                    ("Content-Type", "application/x-www-form-urlencoded"),
                ],
                body,
            ),
        )
    };
    // another start in its place: the page shown first confirms nothing
    let (status, _, _) = start(redirect);
    assert_eq!(status, 302);
    let (_, _, page) = code_page(&cookie);
    let second = form_field(&page, "pendingId");
    assert_ne!(first, second);
    let (status, _, page) = post(
        &cookie,
        &format!("allow=true&password=moqui&moquiSessionToken={token}&pendingId={first}"),
    );
    assert_eq!(status, 400, "a replaced sign-in: {page}");
    assert!(page.contains("The sign-in changed"), "{page}");
    // the one shown now: a wrong password asks again, the right one gives the code
    let (status, _, _) = start(redirect);
    assert_eq!(status, 302);
    let (_, _, page) = code_page(&cookie);
    let pending = form_field(&page, "pendingId");
    let confirm = |password: &str| {
        post(
            &cookie,
            &format!("allow=true&password={password}&moquiSessionToken={token}&pendingId={pending}"),
        )
    };
    let (status, _, page) = confirm("not-it");
    assert_eq!(status, 200, "a wrong password asks again: {page}");
    let (status, headers, body) = confirm("moqui");
    assert_eq!(status, 302, "{body}");
    assert!(header(&headers, "location").contains("code="), "{headers:?}");
    // no password here: sign out and in again from the page, the sign-in kept
    let (status, _, _) = start(redirect);
    assert_eq!(status, 302);
    let (_, _, page) = code_page(&cookie);
    let pending = form_field(&page, "pendingId");
    let token = form_field(&page, "moquiSessionToken");
    let (status, headers, body) = post(
        &cookie,
        &format!("signOut=true&moquiSessionToken={token}&pendingId={pending}"),
    );
    assert_eq!(status, 302, "{body}");
    assert!(
        header(&headers, "location").starts_with("/Login?returnTo="),
        "{headers:?}"
    );
    let cookie = merge_cookies(&cookie, &headers);
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
    let (status, _, page) = code_page(&cookie);
    assert_eq!(status, 200, "{page}");
    assert!(
        page.contains("Sign in to the app") && !page.contains("type=\"password\""),
        "{page}"
    );
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
    <moqui.security.ArtifactGroup artifactGroupId="STUB_MY" description="The stub's my REST"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="STUB_MY" artifactName="/mantle/my" artifactTypeEnumId="AT_REST_PATH"
        inheritAuthz="Y"/>
    <moqui.security.ArtifactAuthz artifactAuthzId="STUB_MY_ALL" userGroupId="ALL_USERS" artifactGroupId="STUB_MY"
        authzTypeEnumId="AUTHZT_ALLOW" authzActionEnumId="AUTHZA_ALL"/>
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
        ("components/addon/my-site/AUTHORS.md", AUTHORS),
        ("components/addon/review-stub/component.xml", STUB_COMPONENT),
        ("components/addon/review-stub/MoquiConf.xml", STUB_CONF),
        ("components/addon/review-stub/entity/ReviewStub.xml", STUB_ENTITIES),
        (
            "components/addon/review-stub/service/reviewstub/Stub.xml",
            STUB_SERVICES,
        ),
        ("components/addon/review-stub/service/mantle.rest.xml", STUB_REST),
    ];
    let gate = start_with_files_and_links(
        "dev-aide-group",
        Some(&data),
        &[
            ("moqui_conf__instance__purpose", "dev"),
            ("moqui_conf__dev_aide__site_component", "my-site"),
            ("moqui_conf__dev_aide__shared", "false"),
            ("moqui_conf__llm_facade__gateway__enabled", "true"),
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
    let (code, browser) = browser_sign_in(address, &challenge, &redirect);
    signed_in_browser_confirms_who_it_is(address, &browser, &challenge, &redirect);
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
    // git's credential is the server's account (item 6)
    let (credential_name, app_key) = {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let facade = factory.entity().unwrap();
        let row = facade
            .find("moqui.dev.DevAideCredential")
            .unwrap()
            .eq("serverId", Value::string("GROUP"))
            .one()
            .unwrap()
            .unwrap();
        (
            row.get("username").map(to_plain_string).unwrap_or_default(),
            row.get("token").map(to_plain_string).unwrap_or_default(),
        )
    };
    assert_eq!(credential_name, "john.doe");
    // question 18: Connect keeps a key for each service, never the account's: the token is git's alone, the tasks and
    // gateway keys are their own, and the sign-in's account key is ended (no application here keeps it)
    let (tasks_key, signed_in_gateway, app_kept) = {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let facade = factory.entity().unwrap();
        let row = facade
            .find("moqui.dev.DevAideServer")
            .unwrap()
            .eq("serverId", Value::string("GROUP"))
            .one()
            .unwrap()
            .unwrap();
        let key = |name: &str| row.get(name).map(to_plain_string).unwrap_or_default();
        (key("tasksKey"), key("gatewayKey"), key("appKey"))
    };
    assert!(app_kept.is_empty(), "no application asks for the installed app's key");
    {
        let context = factory.new_context();
        let _entered = context.enter();
        assert!(
            moqui_core::user::login_user_key(&app_key).is_err(),
            "the token is no account key"
        );
        moqui_core::user::login_user_key_for(&app_key, "git").unwrap();
        moqui_core::user::logout().unwrap();
        moqui_core::user::login_user_key_for(&tasks_key, "tasks").unwrap();
        moqui_core::user::logout().unwrap();
        assert!(moqui_core::user::login_user_key_for(&tasks_key, "git").is_err());
        moqui_core::user::login_user_key_for(&signed_in_gateway, "gateway").unwrap();
        moqui_core::user::logout().unwrap();
        // the sign-in's own key (the installed app's, the account's) is ended on the server
        let _authz = context.disable_authz();
        let facade = factory.entity().unwrap();
        let signed_in = facade
            .find("moqui.security.UserLoginKey")
            .unwrap()
            .eq("userId", Value::string("EX_JOHN_DOE"))
            .eq("deviceName", Value::string("Dev AIDE"))
            .list()
            .unwrap();
        assert!(!signed_in.is_empty());
        let now = moqui_core::user::now_millis();
        assert!(
            signed_in.iter().all(|row| row
                .get("thruDate")
                .and_then(moqui_core::user::value_millis)
                .is_some_and(|thru| thru <= now)),
            "the account's key is ended"
        );
    }
    // over HTTP, each key where a REST resource declares its scope, and nowhere else
    let status = |key: &str, path: &str| exchange(address, &http_request("GET", path, &[("api_key", key)], "")).0;
    let (git_path, tasks_path) = ("/rest/s1/moqui/git/my-site/mergeRequests", "/rest/s1/mantle/my/tasks");
    assert_eq!(status(&app_key, git_path), 200, "the git key on the git REST");
    assert_eq!(status(&tasks_key, tasks_path), 200, "the tasks key on my/tasks");
    assert_eq!(status(&app_key, tasks_path), 401, "the git key on my/tasks");
    assert_eq!(status(&tasks_key, git_path), 401, "the tasks key on the git REST");
    assert_eq!(
        status(&tasks_key, "/rest/s1/mantle/my/noticeCounts"),
        401,
        "the tasks key elsewhere"
    );
    assert_eq!(
        status(&signed_in_gateway, tasks_path),
        401,
        "the gateway key on my/tasks"
    );
    assert!(
        server["offer"]["git"]["httpsUrlTemplate"]
            .as_str()
            .unwrap()
            .ends_with("/git/${name}.git"),
        "{server}"
    );

    // what Connect sets, as confirmed: the addons repository, the task source, the gateway's lines with a key for the
    // gateway alone (the developer's own lines kept), git's credential
    let env_file = gate.runtime.join(".env");
    let env_before = std::fs::read_to_string(&env_file).unwrap_or_default();
    std::fs::write(
        &env_file,
        format!(
            "{env_before}\nmoqui_conf__llm_facade__endpoint__DEFAULT__api_key=sk-own\nmoqui_conf__llm_facade__model__DEFAULT__name=mine\n"
        ),
    )
    .unwrap();
    let applied = call(
        &factory,
        "john.doe",
        &format!("{HS}apply#ServerSetup"),
        &[
            ("serverId", "GROUP"),
            ("addons", "true"),
            ("defaultRepository", "true"),
            ("taskSource", "true"),
            ("gateway", "true"),
            ("gitCredential", "stored"),
        ],
    )
    .unwrap();
    assert_eq!(json(&applied, "done").as_array().unwrap().len(), 4, "{applied:?}");
    let env_lines = moqui_lib::env::parse_dotenv(&std::fs::read_to_string(&env_file).unwrap());
    let env = |name: &str| {
        env_lines
            .iter()
            .rev()
            .find(|(have, _)| have == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    let gateway_key = env("moqui_conf__llm_facade__endpoint__DEFAULT__api_key");
    assert!(
        gateway_key.len() == 40 && gateway_key != app_key,
        "item 4: {gateway_key}"
    );
    assert_eq!(gateway_key, signed_in_gateway, "the gateway's key is the sign-in's");
    assert_eq!(
        env("moqui_conf__llm_facade__endpoint__DEFAULT__url"),
        format!("{base}/llm")
    );
    {
        let context = factory.new_context();
        let _entered = context.enter();
        assert!(
            moqui_core::user::login_user_key(&gateway_key).is_err(),
            "the gateway's key opens nothing else"
        );
        moqui_core::user::login_user_key_for(&gateway_key, "gateway").unwrap();
        moqui_core::user::logout().unwrap();
    }
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
    // the task source reads the user's tasks with the key for tasks alone
    let tasks = call(&factory, "john.doe", "org.moqui.devaide.TaskServices.get#Tasks", &[]).unwrap();
    assert!(json(&tasks, "tasks").to_string().contains("A stub task"), "{tasks:?}");

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
    // the shared root keeps no one's rights or agreement (item 12)
    let hosted_json = {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let facade = factory.entity().unwrap();
        let row = facade
            .find("moqui.dev.DevAideRoot")
            .unwrap()
            .eq("hostedServerId", Value::string("GROUP"))
            .one()
            .unwrap()
            .unwrap();
        row.get("hostedJson").map(to_plain_string).unwrap_or_default()
    };
    assert!(
        !hosted_json.contains("\"access\"") && !hosted_json.contains("\"agreement\""),
        "{hosted_json}"
    );
    // a user with no connection finds nothing, and leaves john.doe's resolution as it was
    let unseen = call(
        &factory,
        "dev.viewer",
        &format!("{HS}resolve#RootHosting"),
        &[("rootId", "my-site")],
    )
    .unwrap();
    assert!(json(&unseen, "hosted").is_null(), "{unseen:?}");
    let still = call(
        &factory,
        "john.doe",
        &format!("{HS}get#MergeRequests"),
        &[("rootId", "my-site")],
    );
    assert!(still.is_ok(), "{still:?}");

    // a Connect-made task source moved to another address takes a token of its own, not the server's (item 5)
    let source_id = json(&sources, "sources")
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["label"] == "Team")
        .and_then(|source| source["taskSourceId"].as_str().map(str::to_string))
        .unwrap();
    let moved = call(
        &factory,
        "john.doe",
        "org.moqui.devaide.TaskServices.store#TaskSource",
        &[
            ("taskSourceId", &source_id),
            ("label", "Team"),
            ("username", "john.doe"),
            ("baseUrl", "https://elsewhere.example.com"),
        ],
    );
    assert!(
        matches!(&moved, Err(err) if err.contains("token of its own")),
        "{moved:?}"
    );

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
    assert!(text(&checked, "warning").contains("someone else's code"), "{checked:?}");
    // never by force: a branch of the developer's is refused, and mr/<n> with a commit of its own stays (item 8)
    let onto_main = call(
        &factory,
        "john.doe",
        &format!("{HS}checkout#MergeRequest"),
        &[("rootId", "my-site"), ("mergeRequestNum", &number), ("branch", "main")],
    );
    assert!(
        matches!(&onto_main, Err(err) if err.contains("is a branch already")),
        "{onto_main:?}"
    );
    std::fs::write(site.join("LOCAL.md"), "mine\n").unwrap();
    git(&site, &["add", "LOCAL.md"]);
    git(&site, &["commit", "-q", "-m", "A local commit"]);
    let local_head = git(&site, &["rev-parse", "HEAD"]);
    let again = call(
        &factory,
        "john.doe",
        &format!("{HS}checkout#MergeRequest"),
        &[("rootId", "my-site"), ("mergeRequestNum", &number)],
    );
    assert!(matches!(&again, Err(err) if err.contains("of your own")), "{again:?}");
    assert_eq!(git(&site, &["rev-parse", &branch]), local_head);

    // P12 review: a thread made on the server shows on its line, in the gutter of the checked-out branch; a reply and
    // a resolve reach the server; a review is submitted
    // on the server, through its REST, as dev.viewer
    let threads_path = format!("/rest/s1/moqui/git/my-site/mergeRequests/{number}/threads");
    let (status, _, body) = exchange(
        address,
        &http_request(
            "POST",
            &threads_path,
            &[("api_key", &viewer_key), ("Content-Type", "application/json")],
            r#"{"body": "Is this the right file?", "path": "README.md", "newLine": 1}"#,
        ),
    );
    assert_eq!(status, 200, "{body}");
    let read = call(
        &factory,
        "john.doe",
        &format!("{HS}get#ReviewThreads"),
        &[("rootId", "my-site"), ("mergeRequestNum", &number)],
    )
    .unwrap();
    let thread = json(&read, "threads")[0].clone();
    assert_eq!(
        (thread["path"].as_str(), thread["newLine"].as_i64()),
        (Some("README.md"), Some(1)),
        "{read:?}"
    );
    assert_eq!(thread["comments"][0]["author"], "dev.viewer");
    let thread_id = thread["threadId"].as_str().unwrap().to_string();
    let gutter = call(
        &factory,
        "john.doe",
        &format!("{HS}get#FileThreads"),
        &[("rootId", "my-site"), ("path", "README.md")],
    )
    .unwrap();
    assert_eq!(json(&gutter, "threads").as_array().unwrap().len(), 1, "{gutter:?}");
    call(
        &factory,
        "john.doe",
        &format!("{HS}create#ReviewComment"),
        &[
            ("rootId", "my-site"),
            ("mergeRequestNum", &number),
            ("threadId", &thread_id),
            ("body", "Yes: it is the readme"),
        ],
    )
    .unwrap();
    call(
        &factory,
        "john.doe",
        &format!("{HS}resolve#ReviewThread"),
        &[
            ("rootId", "my-site"),
            ("mergeRequestNum", &number),
            ("threadId", &thread_id),
        ],
    )
    .unwrap();
    let (status, _, body) = exchange(
        address,
        &http_request("GET", &threads_path, &[("api_key", &viewer_key)], ""),
    );
    assert_eq!(status, 200, "{body}");
    let on_server: serde_json::Value = serde_json::from_str(&body).unwrap();
    let server_thread = on_server["threadList"][0].clone();
    assert_eq!(
        server_thread["commentList"].as_array().unwrap().len(),
        2,
        "{on_server:?}"
    );
    assert_eq!(server_thread["resolved"], true);
    let gutter = call(
        &factory,
        "john.doe",
        &format!("{HS}get#FileThreads"),
        &[("rootId", "my-site"), ("path", "README.md")],
    )
    .unwrap();
    assert!(json(&gutter, "threads").as_array().unwrap().is_empty(), "{gutter:?}");
    let changes = call(
        &factory,
        "john.doe",
        &format!("{HS}get#MergeRequestChanges"),
        &[("rootId", "my-site"), ("mergeRequestNum", &number)],
    )
    .unwrap();
    assert!(
        json(&changes, "files")
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["path"] == "README.md"),
        "{changes:?}"
    );
    let file = call(
        &factory,
        "john.doe",
        &format!("{HS}get#MergeRequestFile"),
        &[
            ("rootId", "my-site"),
            ("mergeRequestNum", &number),
            ("path", "README.md"),
            ("baseCommitId", &text(&changes, "baseCommitId")),
            ("headCommitId", &text(&changes, "headCommitId")),
        ],
    )
    .unwrap();
    assert_eq!(text(&file, "modified"), "A fix\n", "{file:?}");
    call(
        &factory,
        "john.doe",
        &format!("{HS}submit#Review"),
        &[
            ("rootId", "my-site"),
            ("mergeRequestNum", &number),
            ("decision", "comment"),
            ("body", "Read"),
        ],
    )
    .unwrap();

    // P12 checks: a failed status shows on its commit
    let (status, _, body) = exchange(
        address,
        &http_request(
            "POST",
            &format!("/rest/s1/moqui/git/my-site/commits/{proposed_head}/statuses"),
            &[("api_key", &john_key), ("Content-Type", "application/json")],
            // GitHub's names, as an outside CI posts them: `context` among them
            r#"{"state": "failure", "context": "ci/test", "description": "1 failed"}"#,
        ),
    );
    assert_eq!(status, 200, "{body}");
    let states = call(
        &factory,
        "john.doe",
        &format!("{HS}get#CommitStates"),
        &[("rootId", "my-site"), ("commitIds", &proposed_head)],
    )
    .unwrap();
    assert_eq!(json(&states, "states")[&proposed_head], "failure", "{states:?}");
    let checks = call(
        &factory,
        "john.doe",
        &format!("{HS}get#Checks"),
        &[("rootId", "my-site"), ("commitId", &proposed_head)],
    )
    .unwrap();
    assert_eq!(json(&checks, "statuses")[0]["state"], "failure", "{checks:?}");
    assert_eq!(json(&checks, "statuses")[0]["context"], "ci/test", "{checks:?}");

    // a reviewer's suggestion quotes code of the diff, which an agent may not have while its authors have not signed
    let (status, _, body) = exchange(
        address,
        &http_request(
            "POST",
            &threads_path,
            &[("api_key", &viewer_key), ("Content-Type", "application/json")],
            r#"{"body": "Try:\n```suggestion\nlet leaked = 1;\n```", "path": "README.md", "newLine": 1}"#,
        ),
    );
    assert_eq!(status, 200, "{body}");

    // P13 the hosting tool, as an agent calls it: its reads under the read fence write nothing and mark the server's
    // text as untrusted; a person's act is not offered; the diff of a merge request by an author who has not signed is
    // refused; a write's card shows the text and where it goes; the write is refused in a sim, as the Workspace's is
    {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let read = |args: serde_json::Value| dev_aide::agent::hosting::read(&args).map_err(|error| error.instruction);
        let fenced = moqui_core::entity::fence::enter("tool hosting");
        let listed = read(serde_json::json!({"action": "merge_requests", "root": "my-site"})).unwrap();
        assert!(listed.contains("<untrusted from=\"group-server\">"), "{listed}");
        assert!(listed.contains("A viewer's fix"), "{listed}");
        let one = read(serde_json::json!({"action": "merge_request", "root": "my-site", "number": number})).unwrap();
        assert!(one.contains("A viewer's fix"), "{one}");
        let threads = read(serde_json::json!({"action": "threads", "root": "my-site", "number": number})).unwrap();
        assert!(threads.contains("Is this the right file?"), "{threads}");
        assert!(threads.contains("without the code of its diff"), "{threads}");
        assert!(threads.contains("Try:") && !threads.contains("leaked"), "{threads}");
        let checks = read(serde_json::json!({"action": "checks", "root": "my-site", "commit": proposed_head})).unwrap();
        assert!(checks.contains("ci/test") && checks.contains("failure"), "{checks}");
        // the proposal's commits are by setup@example.com, whom the stub's agreement does not know as signed
        let diff = read(serde_json::json!({"action": "changes", "root": "my-site", "number": number})).unwrap_err();
        assert!(diff.contains("authors have not signed"), "{diff}");
        assert!(!diff.contains("README.md"), "{diff}");
        let approve = read(serde_json::json!({"action": "approve", "root": "my-site", "number": number})).unwrap_err();
        assert!(approve.contains("a person"), "{approve}");
        let merge = read(serde_json::json!({"action": "merge", "root": "my-site", "number": number})).unwrap_err();
        assert!(merge.contains("server's page"), "{merge}");
        let card = dev_aide::agent::hosting::card(&serde_json::json!({"action": "comment", "root": "my-site",
            "number": number, "body": "Fixed in the next commit", "path": "README.md", "line": 1}))
        .unwrap();
        assert!(card.summary.contains("README.md line 1"), "{}", card.summary);
        assert!(card.summary.contains(&base), "{}", card.summary);
        assert_eq!(card.changes[0].proposed, "Fixed in the next commit");
        assert!(fenced.violations().is_empty(), "{:?}", fenced.violations());
        drop(fenced);
        {
            let _sim = moqui_core::entity::fence::enter_sim("try_service of a test");
            assert!(
                dev_aide::agent::hosting::execute(&card.args).is_err(),
                "a comment from a sim"
            );
        }
        dev_aide::agent::hosting::execute(&card.args).unwrap();
    }
    let (status, _, body) = exchange(
        address,
        &http_request("GET", &threads_path, &[("api_key", &viewer_key)], ""),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body.matches("Fixed in the next commit").count(), 1, "{body}");

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

    // a push whose merge request cannot be opened (the branch is the target) still pushes, and says why (item 11)
    let pushed_main = call(
        &factory,
        "john.doe",
        &format!("{VCS}push#Remote"),
        &[("rootId", "my-site"), ("openMergeRequest", "true")],
    )
    .unwrap();
    assert!(
        text(&pushed_main, "mergeRequestError").contains("is the target"),
        "{pushed_main:?}"
    );

    // P12 agreement: an author who has not signed is warned; Sign commits AUTHORS.md alone
    git(&site, &["config", "user.email", "unsigned@example.com"]);
    git(&site, &["config", "user.name", "John Doe"]);
    let agreement = call(
        &factory,
        "john.doe",
        &format!("{HS}get#Agreement"),
        &[("rootId", "my-site")],
    )
    .unwrap();
    assert_eq!(
        (text(&agreement, "required"), text(&agreement, "signed")),
        ("true".to_string(), "false".to_string()),
        "{agreement:?}"
    );
    // an agent's try_service in the sim: Dev AIDE's own write services are refused before they run, and the fence
    // refuses what they would do (a commit, a file), which nothing would undo
    let head_before = git(&site, &["rev-parse", "HEAD"]);
    let authors_before = std::fs::read_to_string(site.join("AUTHORS.md")).unwrap();
    {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        assert!(dev_aide::agent::try_service::sim_refusal(&format!("{HS}sign#Agreement")).is_some());
        assert!(dev_aide::agent::try_service::sim_refusal(&format!("{HS}get#Agreement")).is_none());
        let _sim = moqui_core::entity::fence::enter_sim("try_service of a test");
        let params = |pairs: &[(&str, &str)]| -> BTreeMap<String, Value> {
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_string(), Value::string(*value)))
                .collect()
        };
        let signed = moqui_core::service::sync(
            &format!("{HS}sign#Agreement"),
            &params(&[("rootId", "my-site"), ("legalName", "Mallory"), ("years", "2026")]),
        );
        let saved = moqui_core::service::sync(
            "org.moqui.devaide.WorkspaceServices.save#File",
            &params(&[("rootId", "my-site"), ("path", "SIM.md"), ("text", "from the sim\n")]),
        );
        let errors = context.message().errors().join("; ");
        assert!(
            signed.is_err() || saved.is_err() || !errors.is_empty(),
            "the sim refused nothing"
        );
    }
    assert_eq!(
        git(&site, &["rev-parse", "HEAD"]),
        head_before,
        "no commit from the sim"
    );
    assert_eq!(
        std::fs::read_to_string(site.join("AUTHORS.md")).unwrap(),
        authors_before
    );
    assert!(!site.join("SIM.md").exists(), "no file from the sim");

    std::fs::write(site.join("STAGED.md"), "staged\n").unwrap();
    git(&site, &["add", "STAGED.md"]);
    let signed = call(
        &factory,
        "john.doe",
        &format!("{HS}sign#Agreement"),
        &[("rootId", "my-site"), ("legalName", "John Doe"), ("years", "2026")],
    )
    .unwrap();
    assert_eq!(text(&signed, "line"), "- Written in 2026 by John Doe - john.doe");
    assert_eq!(git(&site, &["show", "--name-only", "--format=", "HEAD"]), "AUTHORS.md");
    assert!(git(&site, &["diff", "--cached", "--name-only"]).contains("STAGED.md"));
    let authors = std::fs::read_to_string(site.join("AUTHORS.md")).unwrap();
    assert_eq!(authors.matches("John Doe - john.doe").count(), 2, "{authors}");
    git(&site, &["reset", "-q", "STAGED.md"]);

    // P13 worktrees: two agents' sessions work in two worktrees of my-site; each session's files are its own
    // worktree's; a worktree with uncommitted changes is removed only when they are discarded, and its branch stays
    const WT: &str = "org.moqui.devaide.WorktreeServices.";
    {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let facade = factory.entity().unwrap();
        for (id, user) in [
            ("CONV1", "EX_JOHN_DOE"),
            ("CONV2", "EX_JOHN_DOE"),
            ("CONVX", "DEV_VIEWER"),
        ] {
            let mut row = facade.make_value("moqui.agent.AgentConversation").unwrap();
            row.set("conversationId", Value::string(id));
            row.set("userId", Value::string(user));
            row.create(&facade).unwrap();
        }
    }
    // a session's worktree is for one of the caller's own conversations
    let theirs = call(
        &factory,
        "john.doe",
        &format!("{WT}create#Worktree"),
        &[("rootId", "my-site"), ("conversationId", "CONVX")],
    );
    assert!(matches!(&theirs, Err(err) if err.contains("of yours")), "{theirs:?}");
    let one = call(
        &factory,
        "john.doe",
        &format!("{WT}create#Worktree"),
        &[("rootId", "my-site"), ("conversationId", "CONV1")],
    )
    .unwrap();
    let two = call(
        &factory,
        "john.doe",
        &format!("{WT}create#Worktree"),
        &[("rootId", "my-site"), ("conversationId", "CONV2")],
    )
    .unwrap();
    let (one_id, two_id) = (text(&one, "rootId"), text(&two, "rootId"));
    let (one_dir, two_dir) = (text(&one, "location"), text(&two, "location"));
    assert!(!one_id.is_empty() && one_id != two_id, "{one:?} {two:?}");
    assert_eq!(text(&one, "branch"), "agent/conv1");
    {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let named = "my-site:service/mysite/Things.xml";
        let of_one = dev_aide::agent::session_file("CONV1", named).unwrap();
        let of_two = dev_aide::agent::session_file("CONV2", "component://my-site/service/mysite/Things.xml").unwrap();
        let of_none = dev_aide::agent::session_file("CONV3", named).unwrap();
        assert!(of_one.starts_with(&one_dir), "{of_one:?}");
        assert!(of_two.starts_with(&two_dir), "{of_two:?}");
        assert!(
            of_none.starts_with(std::fs::canonicalize(&site).unwrap()),
            "{of_none:?}"
        );
        // session one reaches neither session two's worktree nor, by its path, the main tree
        let into_two = dev_aide::agent::session_file("CONV1", &format!("{two_id}:service/mysite/Things.xml"));
        assert!(
            matches!(&into_two, Err(err) if err.contains("another session")),
            "{into_two:?}"
        );
        let by_path = format!(
            "{}/service/mysite/Things.xml",
            std::fs::canonicalize(&site).unwrap().display()
        );
        assert!(dev_aide::agent::session_file("CONV1", &by_path)
            .unwrap()
            .starts_with(&one_dir));
        // the hosting tool in a worktree session reads the main tree's hosted repository, under the read fence
        let fenced = moqui_core::entity::fence::enter("tool hosting");
        let listed =
            dev_aide::agent::hosting::read(&serde_json::json!({"action": "merge_requests", "root": one_id})).unwrap();
        assert!(listed.contains("A viewer's fix"), "{listed}");
        assert!(fenced.violations().is_empty(), "{:?}", fenced.violations());
    }
    {
        // another user's worktree is never their session's, whatever conversation it names
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("dev.viewer").unwrap();
        let theirs = dev_aide::agent::session_file("CONV1", "my-site:service/mysite/Things.xml").unwrap();
        assert!(!theirs.starts_with(&one_dir), "{theirs:?}");
    }
    let not_theirs = call(
        &factory,
        "dev.viewer",
        &format!("{WT}remove#Worktree"),
        &[("rootId", &two_id)],
    );
    assert!(
        matches!(&not_theirs, Err(err) if err.contains("another user's")),
        "{not_theirs:?}"
    );
    call(
        &factory,
        "john.doe",
        "org.moqui.devaide.WorkspaceServices.save#File",
        &[("rootId", &one_id), ("path", "NOTE.md"), ("text", "from session one\n")],
    )
    .unwrap();
    assert!(Path::new(&one_dir).join("NOTE.md").is_file());
    assert!(
        !Path::new(&two_dir).join("NOTE.md").exists(),
        "session two sees session one's edit"
    );
    assert!(!site.join("NOTE.md").exists(), "the main tree sees a session's edit");
    let listed = call(
        &factory,
        "john.doe",
        &format!("{WT}get#Worktrees"),
        &[("rootId", "my-site")],
    )
    .unwrap();
    assert_eq!(json(&listed, "worktrees").as_array().unwrap().len(), 2, "{listed:?}");
    assert!(text(&listed, "note").contains("not loaded"), "{listed:?}");
    let kept = call(
        &factory,
        "john.doe",
        &format!("{WT}remove#Worktree"),
        &[("rootId", &one_id)],
    );
    assert!(
        matches!(&kept, Err(err) if err.contains("uncommitted changes")),
        "{kept:?}"
    );
    assert!(Path::new(&one_dir).join("NOTE.md").is_file());
    call(
        &factory,
        "john.doe",
        &format!("{WT}remove#Worktree"),
        &[("rootId", &one_id), ("discardChanges", "true")],
    )
    .unwrap();
    call(
        &factory,
        "john.doe",
        &format!("{WT}remove#Worktree"),
        &[("rootId", &two_id)],
    )
    .unwrap();
    assert!(!Path::new(&one_dir).exists() && !Path::new(&two_dir).exists());
    assert_eq!(git(&site, &["branch", "--list", "agent/conv1"]), "agent/conv1");

    // 21.8: a conversation that ends ends its worktree once the write commits: archived, a clean one is removed (its
    // branch stays); deleted, one with uncommitted changes is kept and its user told once
    let one = call(
        &factory,
        "john.doe",
        &format!("{WT}create#Worktree"),
        &[("rootId", "my-site"), ("conversationId", "CONV1")],
    )
    .unwrap();
    let two = call(
        &factory,
        "john.doe",
        &format!("{WT}create#Worktree"),
        &[("rootId", "my-site"), ("conversationId", "CONV2")],
    )
    .unwrap();
    let (one_dir, two_id, two_dir) = (text(&one, "location"), text(&two, "rootId"), text(&two, "location"));
    call(
        &factory,
        "john.doe",
        "org.moqui.devaide.WorkspaceServices.save#File",
        &[("rootId", &two_id), ("path", "NOTE.md"), ("text", "kept\n")],
    )
    .unwrap();
    let end = |verb: &str, parameters: &[(&str, &str)]| {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let parameters: BTreeMap<String, Value> = parameters
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::string(*value)))
            .collect();
        moqui_core::service::sync(&format!("{verb}#moqui.agent.AgentConversation"), &parameters).unwrap();
        assert!(
            context.message().errors().is_empty(),
            "{:?}",
            context.message().errors()
        );
    };
    let waited = |done: &dyn Fn() -> bool| {
        let started = std::time::Instant::now();
        while !done() && started.elapsed() < std::time::Duration::from_secs(10) {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        done()
    };
    end("update", &[("conversationId", "CONV1"), ("statusId", "AgcArchived")]);
    assert!(
        waited(&|| !Path::new(&one_dir).exists()),
        "the archived session's clean worktree is removed"
    );
    assert_eq!(git(&site, &["branch", "--list", "agent/conv1"]), "agent/conv1");
    end("delete", &[("conversationId", "CONV2")]);
    let told = || {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        factory
            .entity()
            .unwrap()
            .find("moqui.security.user.NotificationMessage")
            .unwrap()
            .eq("topic", Value::string("DevAideWorktreeKept"))
            .count()
            .unwrap()
    };
    assert!(
        waited(&|| told() == 1),
        "the deleted session's user is told its worktree is kept"
    );
    assert!(
        Path::new(&two_dir).join("NOTE.md").is_file(),
        "the worktree with changes is kept"
    );
    // told once: the conversation's end heard again changes nothing
    call(
        &factory,
        "john.doe",
        &format!("{WT}end#ConversationWorktrees"),
        &[("conversationId", "CONV2")],
    )
    .unwrap();
    assert_eq!(told(), 1);
    // the kept worktree goes as any other, its changes discarded
    call(
        &factory,
        "john.doe",
        &format!("{WT}remove#Worktree"),
        &[("rootId", &two_id), ("discardChanges", "true")],
    )
    .unwrap();

    // P13 Run locally: the jobs of the working tree's ci.xml, a matrix job with its combinations
    std::fs::write(
        site.join("ci.xml"),
        r#"<ci><pipeline name="check" on="push"><job name="db" image="moqui-build">
            <matrix><axis name="db" values="postgres mysql"/></matrix><secret name="DEPLOY_KEY"/>
            <step run="./moq test --db ${matrix.db}"/></job></pipeline></ci>"#,
    )
    .unwrap();
    let jobs = call(
        &factory,
        "john.doe",
        "org.moqui.devaide.LocalRunServices.get#LocalJobs",
        &[("rootId", "my-site")],
    )
    .unwrap();
    let job = json(&jobs, "jobs")[0].clone();
    assert_eq!(job["matrixKeys"], serde_json::json!(["postgres", "mysql"]), "{jobs:?}");
    assert_eq!(job["secrets"], serde_json::json!(["DEPLOY_KEY"]), "{jobs:?}");
    assert!(text(&jobs, "note").contains("not in the job's image"), "{jobs:?}");
    let mysql = job["combinations"][1].clone();
    assert!(mysql["script"].as_str().unwrap().contains("--db mysql"), "{mysql}");
    let digest = mysql["digest"].as_str().unwrap().to_string();
    let run = |digest: &str| -> Vec<(&'static str, String)> {
        vec![
            ("rootId", "my-site".into()),
            ("pipeline", "check".into()),
            ("job", "db".into()),
            ("matrixKey", "mysql".into()),
            ("digest", digest.to_string()),
        ]
    };
    // a script other than the one shown is refused
    let stale = call(
        &factory,
        "john.doe",
        "org.moqui.devaide.LocalRunServices.start#LocalRun",
        &[
            ("rootId", "my-site"),
            ("pipeline", "check"),
            ("job", "db"),
            ("matrixKey", "mysql"),
            ("digest", "0000"),
        ],
    );
    assert!(matches!(&stale, Err(err) if err.contains("changed since")), "{stale:?}");
    // never from an agent's fence or a sim
    {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let parameters: BTreeMap<String, Value> = run(&digest)
            .into_iter()
            .map(|(key, value)| (key.to_string(), Value::string(value)))
            .collect();
        for sim in [false, true] {
            let _fence = if sim {
                moqui_core::entity::fence::enter_sim("try_service of a test")
            } else {
                moqui_core::entity::fence::enter("tool try_service")
            };
            let started = moqui_core::service::sync("org.moqui.devaide.LocalRunServices.start#LocalRun", &parameters);
            let errors = context.message().errors().join("; ");
            context.message().clear_errors();
            assert!(
                started.is_err() || !errors.is_empty(),
                "a local run from a fence (sim {sim})"
            );
        }
    }
    // never from a header login
    let body = format!("rootId=my-site&pipeline=check&job=db&matrixKey=mysql&digest={digest}");
    let (_, _, answer) = exchange(
        address,
        &http_request(
            "POST",
            "/apps/dev/Workspace/startLocalRun",
            &[
                ("api_key", &john_key),
                ("Content-Type", "application/x-www-form-urlencoded"),
            ],
            &body,
        ),
    );
    assert!(answer.contains("logged in by a header"), "{answer}");
    assert!(
        std::fs::read_dir(gate.runtime.join("tmp/dev-aide-run"))
            .map(|dir| dir.count())
            .unwrap_or(0)
            == 0,
        "no script was written"
    );
    std::fs::remove_file(site.join("ci.xml")).unwrap();

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
    // the developer's own .env lines are back, the gateway's URL is gone, and both keys are ended (items 3, 4)
    let env_lines = moqui_lib::env::parse_dotenv(&std::fs::read_to_string(&env_file).unwrap());
    let env = |name: &str| {
        env_lines
            .iter()
            .rev()
            .find(|(have, _)| have == name)
            .map(|(_, value)| value.clone())
    };
    assert_eq!(
        env("moqui_conf__llm_facade__endpoint__DEFAULT__api_key").as_deref(),
        Some("sk-own")
    );
    assert_eq!(
        env("moqui_conf__llm_facade__model__DEFAULT__name").as_deref(),
        Some("mine")
    );
    assert_eq!(env("moqui_conf__llm_facade__endpoint__DEFAULT__url"), None);
    let context = factory.new_context();
    let _entered = context.enter();
    assert!(moqui_core::user::login_user_key_for(&gateway_key, "gateway").is_err());
    assert!(
        moqui_core::user::login_user_key_for(&app_key, "git").is_err(),
        "the sign-in's git key is ended"
    );
    assert!(
        moqui_core::user::login_user_key_for(&tasks_key, "tasks").is_err(),
        "the tasks key is ended"
    );
}
