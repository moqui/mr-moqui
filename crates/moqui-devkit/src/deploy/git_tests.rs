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

use std::path::{Path, PathBuf};

use moqui_core::context::ExecutionContextFactory;

use super::{
    check_name, check_url, clone_directory, component_name, final_location, git_command, network_allowed,
    with_listed_sets, DeployRequest, Storage,
};

#[test]
fn a_deploy_fetches_over_https_or_ssh_only() {
    assert!(check_url("https://github.com/moqui/mr-pass.git").is_ok());
    assert!(check_url("ssh://git@github.com/moqui/mr-pass.git").is_ok());
    assert!(check_url("git@github.com:moqui/mr-pass.git").is_ok());
    for refused in [
        "file:///home/me/repo",
        "/home/me/repo",
        "http://example.com/repo.git",
        "git://example.com/repo.git",
        "https:///repo.git",
    ] {
        assert!(check_url(refused).is_err(), "{refused}");
    }
}

#[test]
fn a_component_name_is_one_directory_name() {
    for good in ["mr-pass", "my_site", "site.v2"] {
        assert!(check_name(good).is_ok(), "{good}");
    }
    for bad in ["", ".hidden", "a/b", "../up", "a b"] {
        assert!(check_name(bad).is_err(), "{bad:?}");
    }
    assert_eq!(
        component_name(br#"<component name="mr-pass" version="1"/>"#).unwrap(),
        "mr-pass"
    );
    assert!(component_name(b"<component/>").is_err());
}

#[test]
fn a_clone_goes_in_a_component_directory_under_the_runtime() {
    let runtime = Path::new("/srv/moqui/runtime");
    let dirs = [
        "components".to_string(),
        "components/base".to_string(),
        "components/addon".to_string(),
        "components/dev".to_string(),
    ];
    assert_eq!(
        clone_directory(runtime, "", &dirs).unwrap(),
        runtime.join("components/addon")
    );
    assert_eq!(
        clone_directory(runtime, "components/dev/", &dirs).unwrap(),
        runtime.join("components/dev")
    );
    assert!(clone_directory(runtime, "../elsewhere", &dirs).is_err());
    assert!(clone_directory(runtime, "/etc", &dirs).is_err());
    assert!(clone_directory(runtime, "tmp", &dirs).is_err());
    assert_eq!(
        final_location(&runtime.join("components/addon/.staging-site"), "site"),
        runtime.join("components/addon/site")
    );
    assert_eq!(Storage::parse(""), Some(Storage::Database));
    assert_eq!(Storage::parse("clone"), Some(Storage::Clone));
    assert_eq!(Storage::parse("zip"), None);
}

/// The system `git` in `dir`, with an author for its commits; panics with its error when it fails.
fn system_git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Commit `files` in the work tree `work` and push them to its origin's `main`.
fn push_files(work: &Path, files: &[(&str, &str)], message: &str) {
    for (path, text) in files {
        let path = work.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    system_git(work, &["add", "."]);
    system_git(work, &["commit", "-q", "-m", message]);
    system_git(work, &["push", "-q", "origin", "HEAD:main"]);
}

/// A working clone is made with the system `git` from a local bare repository, then fetched and fast-forwarded to a
/// new commit of its branch, as a deploy and an update of the clone storage do. A `file://` URL is taken here, at the
/// function level; the deploy's URL check refuses it.
#[test]
fn a_working_clone_is_made_and_fast_forwarded_from_a_local_bare_repository() {
    let dir = std::env::temp_dir().join(format!("moqui-deploy-clone-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let bare: PathBuf = dir.join("origin.git");
    system_git(&dir, &["init", "-q", "--bare", "origin.git"]);
    let url = format!("file://{}", bare.display());
    system_git(&dir, &["clone", "-q", &url, "work"]);
    let work = dir.join("work");
    push_files(&work, &[("component.xml", r#"<component name="cloned"/>"#)], "first");

    let request = DeployRequest {
        url: url.clone(),
        branch: "main".to_string(),
        storage: Storage::Clone,
        directory: String::new(),
        username: None,
        token: None,
    };
    let clone = dir.join("cloned");
    git_command(
        None,
        &request,
        &["clone", "--branch", "main", "--single-branch", "--", &url],
        Some(&clone),
    )
    .unwrap();
    assert_eq!(
        component_name(&std::fs::read(clone.join("component.xml")).unwrap()).unwrap(),
        "cloned"
    );
    let from = git_command(Some(&clone), &request, &["rev-parse", "HEAD"], None)
        .unwrap()
        .trim()
        .to_string();

    push_files(&work, &[("service/cloned/ClonedServices.xml", "<services/>")], "second");
    git_command(Some(&clone), &request, &["fetch", "origin"], None).unwrap();
    let to = git_command(Some(&clone), &request, &["rev-parse", "origin/main"], None)
        .unwrap()
        .trim()
        .to_string();
    assert_ne!(from, to);
    assert!(git_command(Some(&clone), &request, &["status", "--porcelain"], None)
        .unwrap()
        .trim()
        .is_empty());
    assert!(
        git_command(
            Some(&clone),
            &request,
            &["merge-base", "--is-ancestor", "HEAD", &to],
            None
        )
        .is_ok(),
        "the update is a fast-forward"
    );
    let changed = git_command(Some(&clone), &request, &["diff", "--name-only", &from, &to], None).unwrap();
    assert_eq!(
        changed.lines().collect::<Vec<_>>(),
        ["service/cloned/ClonedServices.xml"]
    );
    git_command(Some(&clone), &request, &["merge", "--ff-only", &to], None).unwrap();
    assert!(clone.join("service/cloned/ClonedServices.xml").is_file());
    assert_eq!(
        git_command(Some(&clone), &request, &["rev-parse", "HEAD"], None)
            .unwrap()
            .trim(),
        to
    );
    assert!(check_url(&url).is_err(), "the deploy refuses a file URL");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A working clone's fetch with the system `git` is refused while sim mode is active, as the in-process fetch of the
/// other storages is, with the URL's credentials left out of the message.
#[test]
fn a_working_clone_fetch_is_refused_in_sim_mode() {
    let context = ExecutionContextFactory::test_context();
    let entity = moqui_core::testing::open();
    context.factory().set_entity(entity.clone());
    let url = "https://someone:secret-token@example.com/site.git";
    assert!(network_allowed(context.factory(), url).is_ok());
    entity.set_sim_active(true);
    let refused = network_allowed(context.factory(), url);
    entity.set_sim_active(false);
    let err = refused.unwrap_err();
    assert!(err.contains("sim mode"), "{err}");
    assert!(!err.contains("secret-token"), "{err}");
}

const SECRET: &str = "issued-by-this-target-0123456789";
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// An attestation of `site` at [`COMMIT`], issued at `issued_at`, signed with `secret`: its text and signature.
fn attested(secret: &str, issued_at: i64) -> (String, String) {
    use moqui_git::ci::attest::{sign, Attestation, KIND};
    let attestation = Attestation {
        kind: KIND.to_string(),
        v: 1,
        repository_url: "https://source.example.com/git/site.git".to_string(),
        ref_name: "refs/heads/main".to_string(),
        commit_id: COMMIT.to_string(),
        component: "site".to_string(),
        pipeline: "release".to_string(),
        pipeline_run_id: "100".to_string(),
        run_num: 3,
        run_status: "success".to_string(),
        environment: "prod".to_string(),
        deployment_id: "D100".to_string(),
        issued_at,
        source: "https://source.example.com".to_string(),
    };
    let signed = sign(secret, &attestation).unwrap();
    (signed.text, signed.signature)
}

/// The gate (docs/tools/Component-Deploy.md "The gate"): with `require-passed-run` an unattested commit is refused, a
/// stale or forged attestation is refused (a forged one counting as a failed sign-in), and a good one lets in its
/// component at its commit only, for the deploy it is verified for. A replayed one is refused once a newer one was
/// taken, and a good one goes on to the fetch (stopped here by sim mode).
#[test]
fn the_gate_refuses_an_unattested_stale_forged_or_replayed_deploy_and_takes_a_good_one() {
    use super::{deploy_attested, gate, gate_commit, verify_attestation, Attested, AttestedScope};
    let conf = format!(
        r#"<moqui-conf><component-deploy require-passed-run="true" attestation-secret="{SECRET}" max-age-seconds="300"/></moqui-conf>"#
    );
    let context = ExecutionContextFactory::test_context_with_conf(&conf);
    let entity = moqui_core::testing::open();
    context.factory().set_entity(entity.clone());
    let factory = context.factory();
    let _entered = context.enter();
    let now = moqui_core::service::util::now_millis();

    // a deploy from this server's own pages, with no attestation
    assert_eq!(
        gate_commit(factory, "site", COMMIT),
        Err(crate::deploy::GATE_REFUSAL.to_string())
    );
    // a good attestation verifies, and in its deploy lets in its component at its commit only
    let (text, signature) = attested(SECRET, now - 1_000);
    let attestation = verify_attestation(factory, &text, &signature, now).unwrap();
    assert_eq!(
        (attestation.component.as_str(), attestation.commit_id.as_str()),
        ("site", COMMIT)
    );
    {
        let _scope = AttestedScope::enter(Attested {
            component: "site".to_string(),
            commit: COMMIT.to_string(),
        });
        assert!(gate().is_ok(), "the attestation stands for the permission");
        assert!(gate_commit(factory, "site", COMMIT).is_ok());
        assert!(gate_commit(factory, "site", &COMMIT.replace('0', "f")).is_err());
        assert!(gate_commit(factory, "other", COMMIT).is_err());
    }
    assert!(gate_commit(factory, "site", COMMIT).is_err(), "only for its own deploy");
    assert!(gate().is_err(), "no attestation, no permission: COMPONENT_DEPLOY again");

    // stale, forged (another secret), altered
    let (old, old_signature) = attested(SECRET, now - 301_000);
    let err = verify_attestation(factory, &old, &old_signature, now).unwrap_err();
    assert!(err.contains("too old"), "{err}");
    let _ = moqui_core::web::take_auth_failure();
    let (forged, forged_signature) = attested("another-secret-entirely-0123456789", now);
    let err = verify_attestation(factory, &forged, &forged_signature, now).unwrap_err();
    assert!(err.contains("signature does not verify"), "{err}");
    assert!(
        moqui_core::web::take_auth_failure(),
        "a forged attestation is a failed sign-in"
    );
    let err = verify_attestation(factory, &text.replace("\"site\"", "\"other\""), &signature, now).unwrap_err();
    assert!(err.contains("signature does not verify"), "{err}");
    assert!(!err.contains(SECRET));

    // replayed: the component took a newer attestation already
    {
        let _authz = context.disable_authz();
        let mut row = entity.make_value("moqui.server.DeployedComponent").unwrap();
        row.set("componentName", moqui_lib::object::Value::string("site"));
        row.set("status", moqui_lib::object::Value::string("removed"));
        row.set("attestedDate", moqui_core::user::stamp_value(now));
        row.create(&entity).unwrap();
    }
    let err = deploy_attested(factory, &text, &signature, &[]).unwrap_err();
    assert!(err.contains("not taken again"), "{err}");
    // a newer good one passes the gate and goes on to the fetch, which this test's factory (sim mode, no git
    // resources) does not make
    let (fresh, fresh_signature) = attested(SECRET, now + 1_000);
    entity.set_sim_active(true);
    let err = deploy_attested(factory, &fresh, &fresh_signature, &[]).unwrap_err();
    entity.set_sim_active(false);
    assert!(
        err.contains("sim mode") || err.contains("git resources are not installed"),
        "{err}"
    );
}

/// With no secret this server takes no attested deploy, and with the gate off an unattested deploy is let through to
/// the permission check, as before.
#[test]
fn a_server_with_no_secret_takes_no_attested_deploy_and_the_gate_is_off_by_default() {
    use super::{gate_commit, verify_attestation};
    let context = ExecutionContextFactory::test_context();
    let factory = context.factory();
    let _entered = context.enter();
    let now = moqui_core::service::util::now_millis();
    assert!(gate_commit(factory, "site", COMMIT).is_ok());
    let (text, signature) = attested(SECRET, now);
    let err = verify_attestation(factory, &text, &signature, now).unwrap_err();
    assert!(err.contains("takes no attested deploys"), "{err}");
}

#[test]
fn the_sets_components_xml_lists_are_component_directories_too() {
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime");
    assert_eq!(
        with_listed_sets(&runtime, vec!["components".to_string(), "elsewhere/".to_string()]),
        [
            "components",
            "components/base",
            "components/addon",
            "components/dev",
            "components/test",
            "elsewhere"
        ]
    );
}
