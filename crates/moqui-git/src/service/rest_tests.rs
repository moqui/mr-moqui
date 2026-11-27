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

//! The framework's REST for hosting (`tools/service/moqui.rest.xml`), on the in-memory database with the framework's
//! services: merge requests by their numbers through review and merge, the rights of a viewer, the repository behind a
//! URL, the offer with no credentials, and the application's checks: a repository it keeps is refused a write with its
//! REST named, read as before, and its agreement answered; another repository is written as without it.

use std::collections::BTreeMap;

use moqui_lib::object::{to_plain_string, Value};

use super::{kept_message, path_error, url_paths, Keeper, MergeRequestRef, MAX_PAGE_SIZE};
use crate::server::hosting_tests::{entry, grant_entities, ok, s, text_of};
use crate::server::tests::{create, define, fixture_in, fixture_with, user, Fixture, READER, WRITER};

/// A user with no right on any repository.
const OUTSIDER: &str = "git.outsider";
use moqui_core::context::ExecutionContext;

/// The two REST paths' grants, as ToolsSecurityData.xml has them, and the REST file.
fn rest_paths(fixture: &Fixture) {
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        for (group, path) in [("MOQUI_API_GIT", "/moqui/git"), ("MOQUI_API_HOSTING", "/moqui/hosting")] {
            create(
                &fixture.entity,
                "moqui.security.ArtifactGroup",
                &[("artifactGroupId", group)],
            );
            create(
                &fixture.entity,
                "moqui.security.ArtifactGroupMember",
                &[
                    ("artifactGroupId", group),
                    ("artifactName", path),
                    ("artifactTypeEnumId", "AT_REST_PATH"),
                    ("nameIsPattern", "N"),
                    ("inheritAuthz", "Y"),
                ],
            );
            create(
                &fixture.entity,
                "moqui.security.ArtifactAuthz",
                &[
                    ("artifactAuthzId", &format!("{group}_USERS")),
                    ("userGroupId", "ALL_USERS"),
                    ("artifactGroupId", group),
                    ("authzTypeEnumId", "AUTHZT_ALLOW"),
                    ("authzActionEnumId", "AUTHZA_ALL"),
                ],
            );
        }
    }
    let rest_file = moqui_core::context::framework_dir()
        .parent()
        .unwrap()
        .join("tools/service/moqui.rest.xml");
    moqui_core::service::install_rest(fixture.context.factory(), &[rest_file]);
}

/// A REST call as `user` (none: no credentials): its answer and the errors it left.
fn rest(
    fixture: &Fixture,
    user: Option<&str>,
    method: &str,
    path: &str,
    parameters: &[(&str, Value)],
) -> (Option<BTreeMap<String, Value>>, String) {
    let _entered = fixture.context.enter();
    if let Some(user) = user {
        moqui_core::user::internal_login_user(user).unwrap();
    }
    let segments: Vec<&str> = path.split('/').filter(|segment| !segment.is_empty()).collect();
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    let answer = moqui_core::service::run(&segments, method, &parameters);
    let mut errors = ExecutionContext::with_current(|ec| {
        let text = ec.message().errors_text().join("\n");
        ec.message().clear();
        text
    })
    .unwrap_or_default();
    let _ = moqui_core::user::logout();
    match answer {
        Ok(Value::Map(entries)) => (Some(entries.into_iter().collect()), errors),
        Ok(other) => (Some(BTreeMap::from([("value".to_string(), other)])), errors),
        Err(err) => {
            errors.push_str(&format!("\n{err}"));
            (None, errors)
        }
    }
}

fn field(out: &Option<BTreeMap<String, Value>>, key: &str) -> Value {
    out.as_ref()
        .and_then(|out| out.get(key))
        .cloned()
        .unwrap_or(Value::Null)
}

/// Two commits made by the services: main with a file, and branch `topic` from it with one more.
fn branches(fixture: &Fixture, id: &str) {
    ok(
        fixture,
        WRITER,
        "org.moqui.git.GitServices.commit#GitFiles",
        &[
            ("repositoryId", s(id)),
            ("message", s("First")),
            (
                "fileMap",
                Value::Map([("a.txt".to_string(), s("a\n"))].into_iter().collect()),
            ),
        ],
    );
    ok(
        fixture,
        WRITER,
        "org.moqui.git.GitServices.commit#GitFiles",
        &[
            ("repositoryId", s(id)),
            ("message", s("Topic work")),
            ("branch", s("main")),
            ("newBranch", s("topic")),
            (
                "fileMap",
                Value::Map([("t.txt".to_string(), s("topic\n"))].into_iter().collect()),
            ),
        ],
    );
}

#[test]
fn merge_requests_go_through_rest_by_their_numbers() {
    let fixture = fixture_in("rest-review", moqui_core::context::framework_dir());
    grant_entities(&fixture);
    rest_paths(&fixture);
    define(&fixture, "web", "db");
    branches(&fixture, "web");

    // the repository with the caller's rights
    let (writer_view, errors) = rest(&fixture, Some(WRITER), "get", "moqui/git/web", &[]);
    assert!(errors.is_empty(), "{errors}");
    let repository = field(&writer_view, "repository");
    assert_eq!(text_of(entry(&repository, "defaultBranch")), "main");
    assert_eq!(text_of(entry(entry(&repository, "access"), "canUpdate")), "true");
    let (reader_view, _) = rest(&fixture, Some(READER), "get", "moqui/git/web", &[]);
    assert_eq!(
        text_of(entry(entry(&field(&reader_view, "repository"), "access"), "canUpdate")),
        "false"
    );

    // opened, read, renamed, listed as the author's, reviewed, closed and opened again, diffed, merged
    let (opened, errors) = rest(
        &fixture,
        Some(WRITER),
        "post",
        "moqui/git/web/mergeRequests",
        &[("sourceRef", s("topic")), ("title", s("Topic"))],
    );
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(to_plain_string(&field(&opened, "mergeRequestNum")), "1");
    let (read, errors) = rest(&fixture, Some(READER), "get", "moqui/git/web/mergeRequests/1", &[]);
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(text_of(entry(&field(&read, "mergeRequest"), "title")), "Topic");
    let (_, errors) = rest(
        &fixture,
        Some(WRITER),
        "patch",
        "moqui/git/web/mergeRequests/1",
        &[("title", s("Topic, renamed"))],
    );
    assert!(errors.is_empty(), "{errors}");
    let (mine, errors) = rest(
        &fixture,
        Some(WRITER),
        "get",
        "moqui/hosting/mergeRequests",
        &[("filter", s("mine"))],
    );
    assert!(errors.is_empty(), "{errors}");
    let titles: Vec<String> = match field(&mine, "mergeRequestList") {
        Value::List(items) => items.iter().map(|item| text_of(entry(item, "title"))).collect(),
        other => panic!("{other:?}"),
    };
    assert_eq!(titles, ["Topic, renamed"]);
    let (theirs, _) = rest(
        &fixture,
        Some(READER),
        "get",
        "moqui/hosting/mergeRequests",
        &[("filter", s("mine"))],
    );
    assert!(matches!(field(&theirs, "mergeRequestList"), Value::List(items) if items.is_empty()));
    let (_, errors) = rest(
        &fixture,
        Some(READER),
        "post",
        "moqui/git/web/mergeRequests/1/reviews",
        &[("decision", s("comment")), ("note", s("Looks right"))],
    );
    assert!(errors.is_empty(), "{errors}");
    for step in ["close", "reopen"] {
        let (_, errors) = rest(
            &fixture,
            Some(WRITER),
            "post",
            &format!("moqui/git/web/mergeRequests/1/{step}"),
            &[],
        );
        assert!(errors.is_empty(), "{step}: {errors}");
    }
    let (diff, errors) = rest(&fixture, Some(READER), "get", "moqui/git/web/mergeRequests/1/diff", &[]);
    assert!(errors.is_empty(), "{errors}");
    let paths: Vec<String> = match field(&diff, "fileList") {
        Value::List(items) => items.iter().map(|item| text_of(entry(item, "path"))).collect(),
        other => panic!("{other:?}"),
    };
    assert_eq!(paths, ["t.txt"]);
    let (merged, errors) = rest(
        &fixture,
        Some(WRITER),
        "post",
        "moqui/git/web/mergeRequests/1/merge",
        &[],
    );
    assert!(errors.is_empty(), "{errors}");
    assert!(!to_plain_string(&field(&merged, "commitId")).is_empty());

    // a viewer opens none from a branch, a number that is not there is not found, and nobody is asked to log in
    let (_, errors) = rest(
        &fixture,
        Some(READER),
        "post",
        "moqui/git/web/mergeRequests",
        &[("sourceRef", s("topic")), ("title", s("Again"))],
    );
    assert!(errors.contains("may not push"), "{errors}");
    let (_, errors) = rest(&fixture, Some(WRITER), "get", "moqui/git/web/mergeRequests/9", &[]);
    assert!(errors.contains("No merge request !9 in web"), "{errors}");
    let (none, _) = rest(&fixture, None, "get", "moqui/git/web/mergeRequests/1", &[]);
    assert!(none.is_none());
}

const APPLICATION: &str = r#"<services>
    <service verb="check" noun="Write">
        <implements service="org.moqui.git.GitServices.check#GitRestWrite"/>
        <actions><script><![CDATA[
            const kept = repositoryId === 'kept'
            application = kept ? 'smithy' : null
            restUrl = kept ? '/rest/s1/pass/v1/sites/S/smithy/repositories/kept' : null
            // a repository whose check answers nothing: refused, as only an explicit true allows
            if (repositoryId !== 'silent') allowed = !kept || operation === 'read'
            if (allowed === false) message = 'kept by Smithy: ' + operation
        ]]></script></actions>
    </service>
    <service verb="check" noun="Signed">
        <implements service="org.moqui.git.GitServices.check#GitAuthorSigned"/>
        <actions><script><![CDATA[
            known = repositoryId === 'kept'
            required = known
            signed = false
            signUrl = known ? '/kept/-/blob/main/AUTHORS.md' : null
        ]]></script></actions>
    </service>
    <service verb="handle" noun="Thread">
        <implements service="org.moqui.git.GitServices.handle#GitReviewThread"/>
        <actions><script><![CDATA[
            if (operation === 'find') threadList = [{threadId: 'T1', path: path || 'a.txt', newLine: 1, resolved: false,
                commentList: [{commentId: 'C1', authorName: ec.user.username, body: 'On ' + mergeRequestId}]}]
            // a thread is of one merge request: its id carries it, and another's is not there (the interface's rule)
            const ofThis = (id) => String(id).startsWith('T-' + mergeRequestId + '-')
            if (operation === 'comment') { if (body === 'fail') { ec.message.addError('No merge request ' + mergeRequestId); return }
                if (threadId && !ofThis(threadId)) { ec.message.addError('No thread ' + threadId); return }
                threadId = threadId || ('T-' + mergeRequestId + '-' + (path || 'mr') + '-' + (newLine || 0)) }
            if (operation === 'resolve') { if (!ofThis(threadId)) { ec.message.addError('No thread ' + threadId); return }
                threadId = threadId + (resolved ? '-resolved' : '-open') }
        ]]></script></actions>
    </service>
    <service verb="add" noun="Offer">
        <implements service="org.moqui.git.GitServices.add#GitHostingOffer"/>
        <actions><script><![CDATA[
            offer.application = {name: 'smithy', restBase: '/rest/s1/pass/v1/sites/S/smithy'}
            offer.forks = true
        ]]></script></actions>
    </service>
</services>"#;

#[test]
fn an_application_keeps_its_repositories_writes_to_its_own_rest() {
    let fixture = fixture_with(
        "rest-kept",
        moqui_core::context::framework_dir(),
        r#"<git-server rest-write-check-service="test.GitRestApp.check#Write"
            agreement-check-service="test.GitRestApp.check#Signed" offer-service="test.GitRestApp.add#Offer"/>"#,
    );
    grant_entities(&fixture);
    rest_paths(&fixture);
    {
        let _entered = fixture.context.enter();
        std::fs::create_dir_all(fixture.dir.join("service/test")).unwrap();
        let stub = fixture.dir.join("service/test/GitRestApp.xml");
        std::fs::write(&stub, APPLICATION).unwrap();
        let git = moqui_core::context::framework_dir().join("service/org/moqui/git");
        let failures = moqui_core::service::lint(&[
            git.join("GitServices.xml"),
            git.join("GitReviewServices.xml"),
            git.join("GitCiServices.xml"),
            git.join("GitRestServices.xml"),
            stub,
        ])
        .unwrap();
        assert!(failures.is_empty(), "{failures:?}");
    }
    for id in ["kept", "free"] {
        define(&fixture, id, "db");
        branches(&fixture, id);
    }
    let open = |id: &str| {
        rest(
            &fixture,
            Some(WRITER),
            "post",
            &format!("moqui/git/{id}/mergeRequests"),
            &[("sourceRef", s("topic")), ("title", s("Topic"))],
        )
    };
    let (opened, errors) = open("free");
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(to_plain_string(&field(&opened, "mergeRequestNum")), "1");
    let (refused, errors) = open("kept");
    assert!(
        refused.as_ref().is_none_or(|out| out.get("mergeRequestNum").is_none()),
        "{refused:?}"
    );
    assert!(
        errors.contains("GitKeptByApplication: kept by Smithy: mergeRequest.create"),
        "{errors}"
    );
    assert!(
        errors.contains("(use /rest/s1/pass/v1/sites/S/smithy/repositories/kept)"),
        "{errors}"
    );
    let (_, errors) = rest(
        &fixture,
        Some(WRITER),
        "post",
        "moqui/git/kept/refs",
        &[("refName", s("other")), ("revision", s("main"))],
    );
    assert!(errors.contains("GitKeptByApplication"), "{errors}");

    // read as before, with who keeps it and where its REST is; the agreement is the application's answer
    let (read, errors) = rest(&fixture, Some(READER), "get", "moqui/git/kept", &[]);
    assert!(errors.is_empty(), "{errors}");
    let kept_by = entry(&field(&read, "repository"), "keptBy").clone();
    assert_eq!(text_of(entry(&kept_by, "application")), "smithy");
    let (signed, _) = rest(
        &fixture,
        Some(READER),
        "get",
        "moqui/hosting/agreement",
        &[("repositoryId", s("kept"))],
    );
    assert_eq!(to_plain_string(&field(&signed, "known")), "true");
    assert_eq!(to_plain_string(&field(&signed, "signed")), "false");
    assert_eq!(
        to_plain_string(&field(&signed, "signUrl")),
        "/kept/-/blob/main/AUTHORS.md"
    );
    let (unknown, _) = rest(&fixture, Some(READER), "get", "moqui/git/free/agreement", &[]);
    assert_eq!(to_plain_string(&field(&unknown, "known")), "false");
    // SEC-GIT-21: another author's answer is a reviewer's, not any viewer's
    let (_, errors) = rest(
        &fixture,
        Some(READER),
        "get",
        "moqui/hosting/agreement",
        &[("repositoryId", s("kept")), ("authorEmail", s("someone@example.com"))],
    );
    assert!(errors.contains("update right"), "{errors}");
    let (theirs, errors) = rest(
        &fixture,
        Some(WRITER),
        "get",
        "moqui/hosting/agreement",
        &[("repositoryId", s("kept")), ("authorEmail", s("someone@example.com"))],
    );
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(to_plain_string(&field(&theirs, "known")), "true");

    // the repository behind a clone URL, an SSH URL and a page URL
    for url in [
        "https://team.example.com/git/kept.git",
        "ssh://git.reader@team.example.com:2222/kept.git",
        "https://team.example.com/kept/-/tree/main/a.txt",
    ] {
        let (found, errors) = rest(
            &fixture,
            Some(READER),
            "get",
            "moqui/hosting/repository",
            &[("url", s(url))],
        );
        assert!(errors.is_empty(), "{url}: {errors}");
        assert_eq!(
            text_of(entry(&field(&found, "repository"), "repositoryId")),
            "kept",
            "{url}"
        );
    }
    let (missing, _) = rest(
        &fixture,
        Some(READER),
        "get",
        "moqui/hosting/repository",
        &[("url", s("https://team.example.com/git/nothing.git"))],
    );
    assert_eq!(to_plain_string(&field(&missing, "found")), "false");

    // what the server offers, to a caller with no credentials too, with what the application adds
    let (offer, errors) = rest(&fixture, None, "get", "moqui/hosting/offer", &[]);
    assert!(errors.is_empty(), "{errors}");
    let offer = field(&offer, "offer");
    assert_eq!(text_of(entry(entry(&offer, "user"), "signedIn")), "false");
    assert_eq!(text_of(entry(entry(&offer, "application"), "name")), "smithy");
    assert!(text_of(entry(entry(&offer, "git"), "httpsUrlTemplate")).ends_with("/git/${name}.git"));
    let (signed_in, _) = rest(&fixture, Some(READER), "get", "moqui/hosting/offer", &[]);
    assert_eq!(
        text_of(entry(entry(&field(&signed_in, "offer"), "user"), "username")),
        READER
    );
}

#[test]
fn urls_name_repository_paths() {
    assert_eq!(
        url_paths("https://h.example.com/git/moqui/mr-moqui.git"),
        ["moqui/mr-moqui", "mr-moqui"]
    );
    assert_eq!(url_paths("https://h.example.com/git/web.git/info/refs"), ["web"]);
    assert_eq!(
        url_paths("ssh://u@h:2222/moqui/mr-moqui.git"),
        ["moqui/mr-moqui", "mr-moqui"]
    );
    assert_eq!(url_paths("u@h:moqui/mr-moqui.git"), ["moqui/mr-moqui", "mr-moqui"]);
    assert_eq!(
        url_paths("https://h/apps/moqui/mr-moqui/-/tree/main/x.txt?a=1"),
        ["apps/moqui/mr-moqui", "moqui/mr-moqui", "mr-moqui"]
    );
    assert!(url_paths("not a url").is_empty());
    assert!(url_paths("/local/dir").is_empty());
}

#[test]
fn a_refusal_names_the_applications_rest() {
    let keeper = Keeper {
        allowed: false,
        application: "smithy".into(),
        rest_url: "/rest/s1/pass/x".into(),
        ..Keeper::default()
    };
    assert_eq!(
        kept_message("r1", &keeper),
        "GitKeptByApplication: repository r1 is kept by smithy, which takes this change through its own REST \
         (use /rest/s1/pass/x)"
    );
}

/// A private repository and its keeper are not revealed to a caller who may not view it; a hidden merge request is not
/// there for anyone but its author, in the same words as a number that is not there; a keeper that leaves `allowed`
/// empty refuses; one page of merge requests is capped.
#[test]
fn what_a_caller_may_not_see_answers_as_missing() {
    let fixture = fixture_with(
        "rest-unseen",
        moqui_core::context::framework_dir(),
        r#"<git-server rest-write-check-service="test.GitRestApp.check#Write"/>"#,
    );
    grant_entities(&fixture);
    rest_paths(&fixture);
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        user(&fixture.entity, OUTSIDER);
        std::fs::create_dir_all(fixture.dir.join("service/test")).unwrap();
        let stub = fixture.dir.join("service/test/GitRestApp.xml");
        std::fs::write(&stub, APPLICATION).unwrap();
        let git = moqui_core::context::framework_dir().join("service/org/moqui/git");
        let failures = moqui_core::service::lint(&[
            git.join("GitServices.xml"),
            git.join("GitReviewServices.xml"),
            git.join("GitCiServices.xml"),
            git.join("GitRestServices.xml"),
            stub,
        ])
        .unwrap();
        assert!(failures.is_empty(), "{failures:?}");
    }
    for id in ["kept", "silent", "web"] {
        define(&fixture, id, "db");
        branches(&fixture, id);
    }

    // 1: an outsider's write on a kept repository: not found, with no keeper named
    let (_, errors) = rest(
        &fixture,
        Some(OUTSIDER),
        "post",
        "moqui/git/kept/refs",
        &[("refName", s("other")), ("revision", s("main"))],
    );
    assert!(errors.contains("Repository kept not found"), "{errors}");
    assert!(
        !errors.contains("GitKeptByApplication") && !errors.contains("Smithy"),
        "{errors}"
    );

    // 13: a check that leaves allowed empty refuses
    let (_, errors) = rest(
        &fixture,
        Some(WRITER),
        "post",
        "moqui/git/silent/refs",
        &[("refName", s("other")), ("revision", s("main"))],
    );
    assert!(errors.contains("GitKeptByApplication"), "{errors}");

    // 2: a hidden merge request is its author's alone; to anyone else it is a number that is not there
    let (opened, errors) = rest(
        &fixture,
        Some(WRITER),
        "post",
        "moqui/git/web/mergeRequests",
        &[("sourceRef", s("topic")), ("title", s("Held"))],
    );
    assert!(errors.is_empty(), "{errors}");
    let id = to_plain_string(&field(&opened, "mergeRequestId"));
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let parameters: BTreeMap<String, Value> = [
            ("mergeRequestId".to_string(), s(&id)),
            ("hidden".to_string(), Value::Bool(true)),
        ]
        .into();
        moqui_core::service::sync("org.moqui.git.GitReviewServices.set#MergeRequestHidden", &parameters).unwrap();
    }
    let (_, hidden) = rest(&fixture, Some(READER), "get", "moqui/git/web/mergeRequests/1", &[]);
    let (_, missing) = rest(&fixture, Some(READER), "get", "moqui/git/web/mergeRequests/7", &[]);
    assert_eq!(hidden.replace("!1", "!N"), missing.replace("!7", "!N"));
    let (_, diff) = rest(&fixture, Some(READER), "get", "moqui/git/web/mergeRequests/1/diff", &[]);
    assert!(
        diff.contains("No merge request !1 in web") && !diff.contains(&id),
        "{diff}"
    );
    // its author passes the path's gate, and the framework's services decide: a change of its title is taken, and its
    // read (the application's to show) answers in the same words as a number that is not there
    let (_, errors) = rest(
        &fixture,
        Some(WRITER),
        "patch",
        "moqui/git/web/mergeRequests/1",
        &[("title", s("Held, renamed"))],
    );
    assert!(errors.is_empty(), "{errors}");
    let (_, own) = rest(&fixture, Some(WRITER), "get", "moqui/git/web/mergeRequests/1", &[]);
    assert_eq!(own.trim(), "No merge request !1 in web", "{own}");

    // 13: a page of merge requests is capped
    let (_, errors) = rest(
        &fixture,
        Some(WRITER),
        "get",
        "moqui/hosting/mergeRequests",
        &[("pageSize", s("100000")), ("filter", s("all"))],
    );
    assert!(errors.is_empty(), "{errors}");
}

#[test]
fn a_services_error_names_the_number_not_the_id() {
    let merge_request = MergeRequestRef {
        repository_id: "web".into(),
        merge_request_id: "100042".into(),
        num: "3".into(),
    };
    assert_eq!(
        path_error("No merge request 100042", &merge_request),
        "No merge request !3 in web"
    );
    assert_eq!(path_error("may not push", &merge_request), "may not push");
    assert_eq!(MAX_PAGE_SIZE, 100);
}

/// SEC-GIT-18: a page of merge requests is at most `git-server` `merge-requests-max-page`, and a call that names no
/// repository looks only in the first `merge-requests-max-repositories` the caller may view, by id; a call that names
/// repositories looks in those.
#[test]
fn a_call_that_names_no_repository_looks_in_so_many() {
    let fixture = fixture_with(
        "rest-bounded",
        moqui_core::context::framework_dir(),
        r#"<git-server merge-requests-max-page="1" merge-requests-max-repositories="1"/>"#,
    );
    grant_entities(&fixture);
    rest_paths(&fixture);
    for (id, title) in [("alpha", "Alpha"), ("beta", "Beta")] {
        define(&fixture, id, "db");
        branches(&fixture, id);
        let (_, errors) = rest(
            &fixture,
            Some(WRITER),
            "post",
            &format!("moqui/git/{id}/mergeRequests"),
            &[("sourceRef", s("topic")), ("title", s(title))],
        );
        assert!(errors.is_empty(), "{errors}");
    }
    let found = |parameters: &[(&str, Value)]| {
        let (answer, errors) = rest(&fixture, Some(READER), "get", "moqui/hosting/mergeRequests", parameters);
        assert!(errors.is_empty(), "{errors}");
        let titles: Vec<String> = match field(&answer, "mergeRequestList") {
            Value::List(items) => items.iter().map(|item| text_of(entry(item, "title"))).collect(),
            other => panic!("{other:?}"),
        };
        (titles, to_plain_string(&field(&answer, "mergeRequestListCount")))
    };
    // no repository named: the first one the reader may view, alpha
    assert_eq!(
        found(&[("filter", s("all")), ("pageSize", s("50"))]),
        (vec!["Alpha".to_string()], "1".to_string())
    );
    // named, past the walk's bound; a page of one
    assert_eq!(found(&[("repositoryId", s("beta"))]), (vec!["Beta".to_string()], "1".to_string()));
    let both = Value::List(vec![s("alpha"), s("beta")]);
    let (titles, count) = found(&[("repositoryIdList", both), ("pageSize", s("50"))]);
    assert_eq!((titles.len(), count.as_str()), (1, "2"));
}

/// The review threads go to the application's service, as the caller, behind the same gates as the merge request:
/// viewers read, a kept repository's writes are the application's, a hidden merge request is not there, and a server
/// with no service has none.
#[test]
fn review_threads_are_the_applications_behind_the_merge_requests_gates() {
    let fixture = fixture_with(
        "rest-threads",
        moqui_core::context::framework_dir(),
        r#"<git-server rest-write-check-service="test.GitRestApp.check#Write"
            review-thread-service="test.GitRestApp.handle#Thread"/>"#,
    );
    grant_entities(&fixture);
    rest_paths(&fixture);
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        user(&fixture.entity, OUTSIDER);
        std::fs::create_dir_all(fixture.dir.join("service/test")).unwrap();
        let stub = fixture.dir.join("service/test/GitRestApp.xml");
        std::fs::write(&stub, APPLICATION).unwrap();
        let git = moqui_core::context::framework_dir().join("service/org/moqui/git");
        let failures = moqui_core::service::lint(&[
            git.join("GitServices.xml"),
            git.join("GitReviewServices.xml"),
            git.join("GitCiServices.xml"),
            git.join("GitRestServices.xml"),
            stub,
        ])
        .unwrap();
        assert!(failures.is_empty(), "{failures:?}");
    }
    for id in ["web", "kept"] {
        define(&fixture, id, "db");
        branches(&fixture, id);
    }
    let (_, errors) = rest(
        &fixture,
        Some(WRITER),
        "post",
        "moqui/git/web/mergeRequests",
        &[("sourceRef", s("topic")), ("title", s("Topic"))],
    );
    assert!(errors.is_empty(), "{errors}");
    let (read, errors) = rest(
        &fixture,
        Some(READER),
        "get",
        "moqui/git/web/mergeRequests/1/threads",
        &[("path", s("t.txt"))],
    );
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(to_plain_string(&field(&read, "threadsAvailable")), "true");
    let thread = match field(&read, "threadList") {
        Value::List(items) => items[0].clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(text_of(entry(&thread, "path")), "t.txt");
    let (made, errors) = rest(
        &fixture,
        Some(READER),
        "post",
        "moqui/git/web/mergeRequests/1/threads",
        &[("body", s("Why?")), ("path", s("t.txt")), ("newLine", Value::I64(1))],
    );
    assert!(errors.is_empty(), "{errors}");
    let thread_id = to_plain_string(&field(&made, "threadId"));
    assert!(
        thread_id.starts_with("T-") && thread_id.ends_with("-t.txt-1"),
        "{thread_id}"
    );
    let (resolved, errors) = rest(
        &fixture,
        Some(WRITER),
        "post",
        &format!("moqui/git/web/mergeRequests/1/threads/{thread_id}/resolve"),
        &[],
    );
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(
        to_plain_string(&field(&resolved, "threadId")),
        format!("{thread_id}-resolved")
    );
    // a thread of merge request 1 is not reached through merge request 2's path, to resolve or to reply
    ok(
        &fixture,
        WRITER,
        "org.moqui.git.GitServices.commit#GitFiles",
        &[
            ("repositoryId", s("web")),
            ("message", s("Other work")),
            ("branch", s("main")),
            ("newBranch", s("other")),
            (
                "fileMap",
                Value::Map([("o.txt".to_string(), s("other\n"))].into_iter().collect()),
            ),
        ],
    );
    let (second, errors) = rest(
        &fixture,
        Some(WRITER),
        "post",
        "moqui/git/web/mergeRequests",
        &[("sourceRef", s("other")), ("title", s("Second"))],
    );
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(to_plain_string(&field(&second, "mergeRequestNum")), "2");
    {
        let (_, errors) = rest(
            &fixture,
            Some(WRITER),
            "post",
            &format!("moqui/git/web/mergeRequests/2/threads/{thread_id}/resolve"),
            &[],
        );
        assert!(errors.contains("No thread"), "{errors}");
        let (_, errors) = rest(
            &fixture,
            Some(READER),
            "post",
            "moqui/git/web/mergeRequests/2/threads",
            &[("body", s("Sneaky")), ("threadId", s(&thread_id))],
        );
        assert!(errors.contains("No thread"), "{errors}");
    }
    // the service's own words about the merge request are the path's
    let (_, errors) = rest(
        &fixture,
        Some(READER),
        "post",
        "moqui/git/web/mergeRequests/1/threads",
        &[("body", s("fail"))],
    );
    assert_eq!(errors.trim(), "No merge request !1 in web", "{errors}");
    // an outsider: not there; a kept repository's comment is the application's
    let (_, errors) = rest(
        &fixture,
        Some(OUTSIDER),
        "get",
        "moqui/git/web/mergeRequests/1/threads",
        &[],
    );
    assert!(errors.contains("No merge request !1 in web"), "{errors}");
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let parameters: BTreeMap<String, Value> = [
            ("repositoryId".to_string(), s("kept")),
            ("sourceRef".to_string(), s("topic")),
            ("title".to_string(), s("Kept")),
        ]
        .into();
        moqui_core::user::internal_login_user(WRITER).unwrap();
        moqui_core::service::sync("org.moqui.git.GitReviewServices.create#MergeRequest", &parameters).unwrap();
        let _ = moqui_core::user::logout();
    }
    let (_, errors) = rest(
        &fixture,
        Some(WRITER),
        "post",
        "moqui/git/kept/mergeRequests/1/threads",
        &[("body", s("Hello"))],
    );
    assert!(errors.contains("GitKeptByApplication"), "{errors}");
}

#[test]
fn a_server_with_no_thread_service_has_no_threads() {
    let fixture = fixture_in("rest-no-threads", moqui_core::context::framework_dir());
    grant_entities(&fixture);
    rest_paths(&fixture);
    define(&fixture, "web", "db");
    branches(&fixture, "web");
    let (_, errors) = rest(
        &fixture,
        Some(WRITER),
        "post",
        "moqui/git/web/mergeRequests",
        &[("sourceRef", s("topic")), ("title", s("Topic"))],
    );
    assert!(errors.is_empty(), "{errors}");
    let (read, errors) = rest(
        &fixture,
        Some(READER),
        "get",
        "moqui/git/web/mergeRequests/1/threads",
        &[],
    );
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(to_plain_string(&field(&read, "threadsAvailable")), "false");
    let (_, errors) = rest(
        &fixture,
        Some(READER),
        "post",
        "moqui/git/web/mergeRequests/1/threads",
        &[("body", s("Hi"))],
    );
    assert!(errors.contains("keeps no review threads"), "{errors}");
}

/// A commit status posted as GitHub's CI posts it, with `context`: the name the context stack keeps for itself is taken
/// as the status's context.
#[test]
fn a_status_takes_githubs_context_parameter() {
    let fixture = fixture_in("rest-context", moqui_core::context::framework_dir());
    grant_entities(&fixture);
    rest_paths(&fixture);
    define(&fixture, "web", "db");
    branches(&fixture, "web");
    let (posted, errors) = rest(
        &fixture,
        Some(WRITER),
        "post",
        "moqui/git/web/commits/main/statuses",
        &[
            ("state", s("failure")),
            ("context", s("ci/test")),
            ("description", s("1 failed")),
        ],
    );
    assert!(errors.is_empty(), "{errors}");
    assert!(posted.is_some());
    let (read, errors) = rest(
        &fixture,
        Some(READER),
        "get",
        "moqui/git/web/commits/main/statuses",
        &[],
    );
    assert!(errors.is_empty(), "{errors}");
    let contexts: Vec<String> = match field(&read, "statusList") {
        Value::List(items) => items.iter().map(|item| text_of(entry(item, "context"))).collect(),
        other => panic!("{other:?}"),
    };
    assert_eq!(contexts, ["ci/test"]);
}
