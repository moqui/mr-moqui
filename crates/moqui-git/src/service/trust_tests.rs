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

//! Git services that trusted their caller (SEC-GIT-07, -10, -11, -14, -15): each now checks the right it needs, or
//! takes a call with authorization disabled only.

use std::collections::BTreeMap;

use moqui_lib::object::Value;

use crate::server::hosting_tests::{call, get, grant_entities, ok, s};
use crate::server::tests::{create, define, fixture_in, user, Fixture, READER, WRITER};
use crate::service::commit::tests::internal;
use moqui_core::context::ExecutionContextFactory;

const GIT: &str = "org.moqui.git.GitServices";

fn git(name: &str) -> String {
    format!("{GIT}.{name}")
}

/// A db repository `site` with one commit on main.
fn site(name: &str) -> (Fixture, String) {
    let fixture = fixture_in(name, moqui_core::context::framework_dir());
    grant_entities(&fixture);
    define(&fixture, "site", "db");
    let files = [("a.txt".to_string(), s("a\n"))].into_iter().collect();
    let made = ok(
        &fixture,
        WRITER,
        &git("commit#GitFiles"),
        &[
            ("repositoryId", s("site")),
            ("message", s("First")),
            ("fileMap", Value::Map(files)),
        ],
    );
    let head = get(&made, "commitId").to_string();
    (fixture, head)
}

#[test]
fn services_that_trusted_their_caller_check_it() {
    let (fixture, _) = site("trust-rights");
    let repo = [("repositoryId", s("site"))];

    // the delete right, as a push mirror's rows: export, fetch and the older push mirror
    for service in ["export#GitRepository", "fetch#GitRepository", "push#GitMirror"] {
        let (_, errors) = call(&fixture, READER, &git(service), &repo);
        assert!(errors.contains("may not delete"), "{service}: {errors}");
    }
    // a targetPath only under runtime/git-export with authorization on; any place with it disabled
    for target in ["/tmp/elsewhere.git", "../outside.git", "a/../../b"] {
        let (out, errors) = call(
            &fixture,
            WRITER,
            &git("export#GitRepository"),
            &[("repositoryId", s("site")), ("targetPath", s(target))],
        );
        assert!(errors.contains("under runtime/git-export"), "{target}: {errors}");
        assert!(out.get("exportPath").is_none());
    }
    let place = fixture.dir.join("exported.git");
    let (out, errors) = internal(
        &fixture,
        WRITER,
        &git("export#GitRepository"),
        &[("repositoryId", s("site")), ("targetPath", s(&place.to_string_lossy()))],
    );
    assert!(errors.trim().is_empty(), "{errors}");
    assert_eq!(get(&out, "exportPath"), place.to_string_lossy());

    // a content repository's .gitattributes commit asks the update right, as setup#GitLfs
    let (_, errors) = call(
        &fixture,
        READER,
        &git("init#GitRepository"),
        &[("repositoryId", s("site")), ("contentRepository", Value::Bool(true))],
    );
    assert!(!errors.trim().is_empty(), "the reader may not commit .gitattributes");

    // the framework's own: a rule's, a job's or the queue's call, with authorization disabled
    let refused: [(&str, Vec<(&str, Value)>, &str); 6] = [
        (
            "org.moqui.git.GitServices.queue#GitWebhookPush",
            vec![("repositoryId", s("site")), ("userId", s(READER))],
            "gitECA rule only",
        ),
        (
            "org.moqui.git.GitServices.run#GitRepositoryMirrors",
            vec![("mirrorId", s("M1"))],
            "mirror queue only",
        ),
        (
            "org.moqui.git.GitServices.clean#GitUpdates",
            vec![("daysToKeep", Value::I64(0))],
            "its job only",
        ),
        (
            "org.moqui.git.GitReviewServices.check#MergeRequest",
            vec![("mergeRequestId", s("X"))],
            "merge request services only",
        ),
        (
            "org.moqui.git.GitReviewServices.sync#MergeRequest",
            vec![("mergeRequestId", s("X"))],
            "merge request services only",
        ),
        (
            "org.moqui.git.GitReviewServices.refresh#MergeRequests",
            vec![("repositoryId", s("site")), ("refName", s("refs/heads/main"))],
            "gitECA rule only",
        ),
    ];
    for (service, parameters, why) in refused {
        let (_, errors) = call(&fixture, WRITER, service, &parameters);
        assert!(errors.contains(why), "{service}: {errors}");
        let (_, errors) = internal(&fixture, WRITER, service, &parameters);
        assert!(!errors.contains(why), "{service} with authorization disabled: {errors}");
    }
}

#[test]
fn a_fork_id_is_one_safe_segment_and_only_a_branch_or_tag_is_deleted() {
    let (fixture, head) = site("trust-names");
    // SEC-GIT-10: the new id is a directory under runtime/git
    for id in ["../evil", "/abs", "a/b", ".hidden"] {
        let (_, errors) = internal(
            &fixture,
            WRITER,
            &git("fork#GitRepository"),
            &[("sourceRepositoryId", s("site")), ("repositoryId", s(id))],
        );
        assert!(errors.contains("is not a repository id"), "{id}: {errors}");
    }
    // SEC-GIT-11: a merge request's head is not a branch or tag to delete
    let name = "refs/merge-requests/1/head";
    {
        let _entered = fixture.context.enter();
        let provider = crate::provider(fixture.context.factory()).unwrap();
        let repo = provider.repo("site").unwrap();
        let id = gix::ObjectId::from_hex(head.as_bytes()).unwrap();
        assert!(repo.store.compare_and_set_ref(name, None, id).unwrap());
    }
    let (_, errors) = call(
        &fixture,
        WRITER,
        &git("delete#GitRef"),
        &[("repositoryId", s("site")), ("refName", s(name))],
    );
    assert!(errors.contains("GitBadRefName"), "{errors}");
    assert_eq!(
        crate::server::tests::head_of(&fixture, "site", name).as_deref(),
        Some(head.as_str())
    );
}

/// SEC-GIT-14: with `AT_GIT_REPOSITORY` authorization off every repository check would pass, so a call with
/// authorization on is refused, as the git server refuses everything then.
#[test]
fn with_repository_authorization_off_the_services_refuse() {
    let context = ExecutionContextFactory::test_context_in(
        "",
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    );
    let entity = moqui_core::testing::open();
    context.factory().set_entity(entity.clone());
    crate::install(context.factory()).unwrap();
    let _entered = context.enter();
    {
        let _authz = context.disable_authz();
        user(&entity, WRITER);
        create(
            &entity,
            crate::GIT_REPOSITORY,
            &[("repositoryId", "site"), ("repositoryKind", "db")],
        );
    }
    let parameters: BTreeMap<String, Value> = [("repositoryId".to_string(), s("site"))].into_iter().collect();
    let errors = || {
        moqui_core::context::ExecutionContext::with_current(|ec| {
            let text = ec.message().errors_text().join("\n");
            ec.message().clear();
            text
        })
        .unwrap_or_default()
    };
    moqui_core::user::internal_login_user(WRITER).unwrap();
    let _ = moqui_core::service::sync(&git("get#GitRefs"), &parameters);
    let refused = errors();
    assert!(
        refused.contains("AT_GIT_REPOSITORY authorization is not enabled"),
        "{refused}"
    );
    {
        let _authz = context.disable_authz();
        let _ = moqui_core::service::sync(&git("get#GitRefs"), &parameters);
        let internal = errors();
        assert!(!internal.contains("AT_GIT_REPOSITORY"), "{internal}");
    }
    moqui_core::user::logout().unwrap();
}

/// SEC-GIT-15: a deleted repository takes its push mirror rows, and its webhooks' queued deliveries are cancelled.
#[test]
fn a_deleted_repository_takes_its_mirrors_and_queued_deliveries() {
    let (fixture, _) = site("trust-delete");
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        create(
            &fixture.entity,
            "moqui.resource.GitRepositoryMirror",
            &[
                ("mirrorId", "M1"),
                ("repositoryId", "site"),
                ("remoteUrl", "https://example.com/site.git"),
            ],
        );
        create(
            &fixture.entity,
            "moqui.resource.GitWebhook",
            &[
                ("webhookId", "W1"),
                ("repositoryId", "site"),
                ("url", "https://example.com/hook"),
            ],
        );
        create(
            &fixture.entity,
            "moqui.service.message.SystemMessageType",
            &[("systemMessageTypeId", "GitWebhook")],
        );
        create(
            &fixture.entity,
            "moqui.service.message.SystemMessage",
            &[
                ("systemMessageId", "SM1"),
                ("systemMessageTypeId", "GitWebhook"),
                ("receiverId", "W1"),
                ("statusId", "SmsgProduced"),
                ("isOutgoing", "Y"),
            ],
        );
    }
    ok(
        &fixture,
        WRITER,
        &git("delete#GitRepository"),
        &[("repositoryId", s("site"))],
    );
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    let mirrors = fixture
        .entity
        .find("moqui.resource.GitRepositoryMirror")
        .unwrap()
        .eq("repositoryId", s("site"))
        .count()
        .unwrap();
    assert_eq!(mirrors, 0);
    let message = fixture
        .entity
        .find("moqui.service.message.SystemMessage")
        .unwrap()
        .eq("systemMessageId", s("SM1"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(
        message
            .get("statusId")
            .map(moqui_lib::object::to_plain_string)
            .as_deref(),
        Some("SmsgCancelled")
    );
}
