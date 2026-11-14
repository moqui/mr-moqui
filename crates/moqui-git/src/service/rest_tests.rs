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

use super::{kept_message, url_paths, Keeper};
use crate::server::hosting_tests::{entry, grant_entities, ok, s, text_of};
use crate::server::tests::{create, define, fixture_in, fixture_with, Fixture, READER, WRITER};
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
            allowed = !kept || operation === 'read'
            if (!allowed) message = 'kept by Smithy: ' + operation
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
