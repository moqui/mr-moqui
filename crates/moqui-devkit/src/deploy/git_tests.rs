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

/// SEC-OUT-05: the request's token reaches git in its environment, which only this user can read, never in its
/// arguments, which any local user can.
#[test]
fn the_deploy_token_is_in_git_s_environment_not_its_arguments() {
    use base64::Engine as _;
    let request = DeployRequest {
        url: "https://example.com/r.git".to_string(),
        branch: "main".to_string(),
        username: Some("deployer".to_string()),
        token: Some("s3cret-token".to_string()),
        ..DeployRequest::default()
    };
    let command = super::git_process_with(None, &request, &["fetch", "origin"], None, &[]);
    let args: Vec<String> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args, ["fetch", "origin"]);
    let basic = base64::engine::general_purpose::STANDARD.encode("deployer:s3cret-token");
    let env: Vec<(String, String)> = command
        .get_envs()
        .filter_map(|(name, value)| {
            Some((
                name.to_string_lossy().into_owned(),
                value?.to_string_lossy().into_owned(),
            ))
        })
        .collect();
    for (name, value) in [
        ("GIT_CONFIG_COUNT", "1".to_string()),
        ("GIT_CONFIG_KEY_0", "http.extraHeader".to_string()),
        ("GIT_CONFIG_VALUE_0", format!("Authorization: Basic {basic}")),
    ] {
        assert!(env.contains(&(name.to_string(), value)), "{name} in {env:?}");
    }
    // with no token, no configuration is added
    let plain = super::git_process_with(None, &DeployRequest::default(), &["status"], None, &[]);
    assert!(plain.get_envs().all(|(name, _)| name != "GIT_CONFIG_COUNT"));
}

/// SEC-TLS-08: a working clone's fetch on a production instance is pinned to the address its host was checked at: curl's
/// resolve entry over https, an `ssh` command with the address as its host name and the name as its host key alias over
/// ssh, in git's environment beside the token. A host written as a public address needs no pin; an internal one is
/// refused, and a host name the shell would read as more than a word.
#[test]
fn a_clone_fetch_is_pinned_to_the_checked_address() {
    let v4: std::net::IpAddr = "93.184.216.34".parse().unwrap();
    let v6: std::net::IpAddr = "2606:2800:220:1::1".parse().unwrap();
    assert_eq!(
        super::pins_to("https://git.example.com/r.git", "git.example.com", 443, v4).unwrap(),
        [(
            "http.curloptResolve".to_string(),
            "git.example.com:443:93.184.216.34".to_string()
        )]
    );
    assert_eq!(
        super::pins_to("https://git.example.com:8443/r.git", "git.example.com", 8443, v6).unwrap(),
        [(
            "http.curloptResolve".to_string(),
            "git.example.com:8443:[2606:2800:220:1::1]".to_string()
        )]
    );
    assert_eq!(
        super::pins_to("git@git.example.com:org/r.git", "git.example.com", 22, v4).unwrap(),
        [(
            "core.sshCommand".to_string(),
            "ssh -o HostName=93.184.216.34 -o HostKeyAlias=git.example.com -o CheckHostIP=no".to_string()
        )]
    );
    assert!(super::pins_to("ssh://x;touch y/r.git", "x;touch y", 22, v4).is_err());
    assert!(super::address_pins("https://93.184.216.34/r.git").unwrap().is_empty());
    assert!(super::address_pins("https://127.0.0.1/r.git").is_err());
    assert!(super::address_pins("https://user@10.0.0.1/r.git").is_err());

    let request = DeployRequest {
        url: "https://git.example.com/r.git".to_string(),
        token: Some("t".to_string()),
        ..DeployRequest::default()
    };
    let pins = super::pins_to(&request.url, "git.example.com", 443, v4).unwrap();
    let command = super::git_process_with(None, &request, &["fetch", "origin"], None, &pins);
    let env: Vec<(String, String)> = command
        .get_envs()
        .filter_map(|(name, value)| {
            Some((
                name.to_string_lossy().into_owned(),
                value?.to_string_lossy().into_owned(),
            ))
        })
        .collect();
    for (name, value) in [
        ("GIT_CONFIG_COUNT", "2"),
        ("GIT_CONFIG_KEY_1", "http.curloptResolve"),
        ("GIT_CONFIG_VALUE_1", "git.example.com:443:93.184.216.34"),
    ] {
        assert!(
            env.contains(&(name.to_string(), value.to_string())),
            "{name} in {env:?}"
        );
    }
}

/// SEC-TLS-08: on a production instance user info does not hide an internal host, and a host that does not resolve is
/// refused rather than taken as public.
#[test]
fn on_production_an_internal_or_unknown_deploy_host_is_refused() {
    assert_eq!(
        super::url_host("https://user:secret@127.0.0.1:8443/r.git"),
        Some(("127.0.0.1".to_string(), 8443))
    );
    assert_eq!(
        super::url_host("git@10.0.0.5:org/r.git"),
        Some(("10.0.0.5".to_string(), 22))
    );
    moqui_core::web::TEST_PRODUCTION.with(|flag| flag.set(Some(true)));
    let checked: Vec<_> = [
        "https://user:secret@127.0.0.1/r.git",
        "ssh://git@[::1]:2222/r.git",
        "git@169.254.169.254:r.git",
        "https://no-such-host.invalid/r.git",
        "https://93.184.216.34/r.git",
    ]
    .iter()
    .map(|url| check_url(url))
    .collect();
    moqui_core::web::TEST_PRODUCTION.with(|flag| flag.set(None));
    for refused in &checked[..4] {
        assert!(refused.is_err(), "{checked:?}");
    }
    assert!(checked[4].is_ok(), "{checked:?}");
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
const OTHER_COMMIT: &str = "fedcba9876543210fedcba9876543210fedcba98";
const SOURCE_URL: &str = "https://source.example.com/git/site.git";
const TARGET_URL: &str = "https://target.example.com";

/// The conf of a target with the gate on, its secret, its public URL, and allow entries for `site` from
/// [`SOURCE_URL`] on main and release.
fn gate_conf() -> String {
    format!(
        r#"<moqui-conf><component-deploy require-passed-run="true" attestation-secret="{SECRET}" max-age-seconds="300"
            public-urls="{TARGET_URL}">
            <allow component="site" repository-url="{SOURCE_URL}" branch="main"/>
            <allow component="site" repository-url="{SOURCE_URL}" branch="release"/>
        </component-deploy></moqui-conf>"#
    )
}

/// An attestation of `site` at [`COMMIT`] on main, for [`TARGET_URL`], issued at `issued_at`.
fn attestation(issued_at: i64) -> moqui_git::ci::attest::Attestation {
    use moqui_git::ci::attest::{Attestation, KIND, VERSION};
    Attestation {
        kind: KIND.to_string(),
        v: VERSION,
        audience: TARGET_URL.to_string(),
        repository_url: SOURCE_URL.to_string(),
        ref_name: "refs/heads/main".to_string(),
        commit_id: COMMIT.to_string(),
        component: "site".to_string(),
        pipeline: "release".to_string(),
        pipeline_run_id: "100".to_string(),
        run_num: 3,
        run_status: "success".to_string(),
        environment: "prod".to_string(),
        deployment_id: format!("D{issued_at}"),
        issued_at,
        source: "https://source.example.com".to_string(),
        data_types: Vec::new(),
    }
}

/// `attestation` signed with `secret`: its text and signature.
fn signed(secret: &str, attestation: &moqui_git::ci::attest::Attestation) -> (String, String) {
    let signed = moqui_git::ci::attest::sign(secret, attestation).unwrap();
    (signed.text, signed.signature)
}

/// [`attestation`] issued at `issued_at`, signed with `secret`.
fn attested(secret: &str, issued_at: i64) -> (String, String) {
    signed(secret, &attestation(issued_at))
}

/// A target with the gate's conf, an in-memory database, entered.
fn target(conf: &str) -> (moqui_core::context::ExecutionContext, moqui_core::entity::EntityFacade) {
    // not production on this thread, whatever instance@purpose another test's start left in the process: the gate
    // refuses every deploy on a production instance, and the deploy tests failed so in a full workspace run
    moqui_core::web::TEST_PRODUCTION.with(|flag| flag.set(Some(false)));
    let context = ExecutionContextFactory::test_context_with_conf(conf);
    let entity = moqui_core::testing::open();
    context.factory().set_entity(entity.clone());
    (context, entity)
}

/// A `DeployedComponent` row of `site` with these fields.
fn deployed(entity: &moqui_core::entity::EntityFacade, fields: &[(&str, &str)]) {
    let mut row = entity.make_value("moqui.server.DeployedComponent").unwrap();
    row.set("componentName", moqui_lib::object::Value::string("site"));
    for (name, value) in fields {
        row.set(*name, moqui_lib::object::Value::string(*value));
    }
    row.create_or_update(entity).unwrap();
}

/// The gate (docs/tools/Component-Deploy.md "The gate"): with `require-passed-run` an unattested commit is refused; a
/// good attestation lets in its component at its commit only, for the deploy it is verified for; a stale one, one
/// addressed to another server, and one of a component, repository URL or branch no allow entry names are refused;
/// a forged or altered one is one generic refusal, counted as a failed sign-in.
#[test]
fn the_gate_refuses_an_unattested_stale_forged_misaddressed_or_unallowed_deploy_and_takes_a_good_one() {
    use super::{gate, gate_commit, verify_attestation, Attested, AttestedScope, NOT_ACCEPTED};
    let (context, _entity) = target(&gate_conf());
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
    let good = verify_attestation(factory, &text, &signature, now).unwrap();
    assert_eq!((good.component.as_str(), good.commit_id.as_str()), ("site", COMMIT));
    {
        let _scope = AttestedScope::enter(Attested {
            component: "site".to_string(),
            commit: COMMIT.to_string(),
            managed: false,
        });
        assert!(gate().is_ok(), "the attestation stands for the permission");
        assert!(gate_commit(factory, "site", COMMIT).is_ok());
        assert!(gate_commit(factory, "site", OTHER_COMMIT).is_err());
        assert!(gate_commit(factory, "other", COMMIT).is_err());
    }
    assert!(gate_commit(factory, "site", COMMIT).is_err(), "only for its own deploy");
    assert!(gate().is_err(), "no attestation, no permission: COMPONENT_DEPLOY again");

    // stale
    let (old, old_signature) = attested(SECRET, now - 301_000);
    let err = verify_attestation(factory, &old, &old_signature, now).unwrap_err();
    assert!(err.contains("too old"), "{err}");
    // forged (another secret) and altered: one refusal, a failed sign-in, the secret in neither
    let _ = moqui_core::web::take_auth_failure();
    let (forged, forged_signature) = attested("another-secret-entirely-0123456789", now);
    assert_eq!(
        verify_attestation(factory, &forged, &forged_signature, now),
        Err(NOT_ACCEPTED.to_string())
    );
    assert!(
        moqui_core::web::take_auth_failure(),
        "a forged attestation is a failed sign-in"
    );
    let err = verify_attestation(factory, &text.replace("\"site\"", "\"other\""), &signature, now).unwrap_err();
    assert_eq!(err, NOT_ACCEPTED);
    // addressed to another server that shares the secret
    let mut elsewhere = attestation(now);
    elsewhere.audience = "https://other-target.example.com".to_string();
    let (text, signature) = signed(SECRET, &elsewhere);
    let err = verify_attestation(factory, &text, &signature, now).unwrap_err();
    assert!(err.contains("addressed to another server"), "{err}");
    // signed, but of a component, a repository or a branch no allow entry names
    for (component, url, branch) in [
        ("admin-tools", SOURCE_URL, "refs/heads/main"),
        ("site", "https://attacker.example.com/site.git", "refs/heads/main"),
        ("site", SOURCE_URL, "refs/heads/experiment"),
    ] {
        let mut unallowed = attestation(now);
        unallowed.component = component.to_string();
        unallowed.repository_url = url.to_string();
        unallowed.ref_name = branch.to_string();
        let (text, signature) = signed(SECRET, &unallowed);
        let err = verify_attestation(factory, &text, &signature, now).unwrap_err();
        assert!(
            err.contains("component-deploy allow"),
            "{component} {url} {branch}: {err}"
        );
    }
    // the allow entry's URL compares without the case of its host or a trailing slash
    let mut cased = attestation(now);
    cased.repository_url = "https://Source.Example.com/git/site.git/".to_string();
    let (text, signature) = signed(SECRET, &cased);
    assert!(verify_attestation(factory, &text, &signature, now).is_ok());
}

/// With no secret this server takes no attested deploy, and says so as it says of a forged signature (an
/// unauthenticated caller does not learn whether a secret is set); a server that does not know its own public URL
/// takes none either; with the gate off an unattested deploy is let through to the permission check, as before.
#[test]
fn a_server_with_no_secret_refuses_as_for_a_forged_signature_and_the_gate_is_off_by_default() {
    use super::{gate_commit, verify_attestation, NOT_ACCEPTED};
    let context = ExecutionContextFactory::test_context();
    let factory = context.factory();
    let _entered = context.enter();
    let now = moqui_core::service::util::now_millis();
    assert!(gate_commit(factory, "site", COMMIT).is_ok());
    let (text, signature) = attested(SECRET, now);
    let _ = moqui_core::web::take_auth_failure();
    assert_eq!(
        verify_attestation(factory, &text, &signature, now),
        Err(NOT_ACCEPTED.to_string())
    );
    assert!(moqui_core::web::take_auth_failure());

    let unknown = format!(
        r#"<moqui-conf><component-deploy attestation-secret="{SECRET}"><allow component="site"
            repository-url="{SOURCE_URL}" branch="main"/></component-deploy></moqui-conf>"#
    );
    let context = ExecutionContextFactory::test_context_with_conf(&unknown);
    let factory = context.factory();
    let _entered = context.enter();
    let err = verify_attestation(factory, &text, &signature, now).unwrap_err();
    assert!(err.contains("addressed to another server"), "{err}");
}

/// The claim of an attestation (docs/tools/Component-Deploy.md "The gate"): of many deploys of one attestation at
/// once, one claims it; an older or equal one after it is refused; and the record outlives the component's row.
#[test]
fn an_attestation_is_claimed_once_and_never_for_an_older_one() {
    use super::claim_attestation;
    let (context, entity) = target(&gate_conf());
    let factory = std::sync::Arc::clone(context.factory());
    let now = moqui_core::service::util::now_millis();

    // the component took one before (its record is made once, under the primary key, which this test's in-memory
    // database does not enforce): the claims of the next race on the conditional update alone
    claim_attestation(&factory, &attestation(now - 10)).unwrap();
    let same = attestation(now);
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let factory = std::sync::Arc::clone(&factory);
            let same = same.clone();
            std::thread::spawn(move || claim_attestation(&factory, &same).is_ok())
        })
        .collect();
    let claimed = threads
        .into_iter()
        .filter_map(|thread| thread.join().ok())
        .filter(|ok| *ok)
        .count();
    assert_eq!(claimed, 1, "one deploy of an attestation goes on");
    let err = claim_attestation(&factory, &same).unwrap_err();
    assert!(err.contains("not taken again"), "{err}");
    // older: no rolling back to an older attested commit
    let err = claim_attestation(&factory, &attestation(now - 1)).unwrap_err();
    assert!(err.contains("not taken again"), "{err}");
    // newer: taken, and the older one stays refused
    assert!(claim_attestation(&factory, &attestation(now + 1)).is_ok());
    assert!(claim_attestation(&factory, &attestation(now)).is_err());
    // the component's row discarded (a staged deploy dropped): the record stays, and so does the refusal
    let _entered = context.enter();
    deployed(&entity, &[("status", "staged")]);
    {
        let _authz = context.disable_authz();
        let row = entity
            .find("moqui.server.DeployedComponent")
            .unwrap()
            .condition(moqui_db::sql::Condition::equals(
                "componentName",
                moqui_lib::object::Value::string("site"),
            ))
            .one()
            .unwrap()
            .unwrap();
        row.delete(&entity).unwrap();
    }
    assert!(claim_attestation(&factory, &attestation(now + 1)).is_err());
}

/// `deploy_attested` claims its attestation before it fetches, so a replay of it is refused though the first deploy
/// did not finish (here the fetch is refused by sim mode); a newer one is claimed and goes on to the fetch.
#[test]
fn an_attested_deploy_records_its_claim_so_a_replay_is_refused() {
    use super::deploy_attested;
    let (context, entity) = target(&gate_conf());
    let factory = context.factory();
    let _entered = context.enter();
    let now = moqui_core::service::util::now_millis();
    let (text, signature) = attested(SECRET, now - 1_000);
    entity.set_sim_active(true);
    let first = deploy_attested(factory, &text, &signature).unwrap_err();
    let replay = deploy_attested(factory, &text, &signature).unwrap_err();
    let (fresh, fresh_signature) = attested(SECRET, now);
    let newer = deploy_attested(factory, &fresh, &fresh_signature).unwrap_err();
    entity.set_sim_active(false);
    assert!(
        first.contains("sim mode") || first.contains("git resources are not installed"),
        "{first}"
    );
    assert!(replay.contains("not taken again"), "{replay}");
    assert!(!newer.contains("not taken again"), "{newer}");
}

/// An attested deploy of a component deployed from git already updates it only from the same repository URL and the
/// same branch; what a fetch staged must be the attested component at the attested commit.
#[test]
fn an_attested_deploy_takes_only_its_components_repository_url_branch_and_staged_commit() {
    use super::{check_staged, deploy_attested, DeployPreview};
    let (context, entity) = target(&gate_conf());
    let factory = context.factory();
    let _entered = context.enter();
    let now = moqui_core::service::util::now_millis();
    deployed(
        &entity,
        &[
            ("source", "git"),
            ("status", "deployed"),
            ("storage", "database"),
            ("remoteUrl", "https://elsewhere.example.com/site.git"),
            ("branch", "main"),
        ],
    );
    let (text, signature) = attested(SECRET, now - 2_000);
    let err = deploy_attested(factory, &text, &signature).unwrap_err();
    assert!(err.contains("another repository URL"), "{err}");
    deployed(&entity, &[("remoteUrl", SOURCE_URL), ("branch", "main")]);
    let mut release = attestation(now - 1_000);
    release.ref_name = "refs/heads/release".to_string();
    let (text, signature) = signed(SECRET, &release);
    let err = deploy_attested(factory, &text, &signature).unwrap_err();
    assert!(err.contains("deployed from the branch main"), "{err}");

    let good = attestation(now);
    let staged = |component: &str, commit: &str| DeployPreview {
        component: component.to_string(),
        commit: commit.to_string(),
        ..DeployPreview::default()
    };
    assert!(check_staged(&staged("site", COMMIT), &good).is_ok());
    let err = check_staged(&staged("site", OTHER_COMMIT), &good).unwrap_err();
    assert!(err.contains("the branch main holds site at fedcba987654"), "{err}");
    assert!(check_staged(&staged("other", COMMIT), &good).is_err());
}

/// The gate's checks inside `deploy` and `update`: with `require-passed-run`, no unattested deploy or update; an
/// attestation lets in its own component only, and in `deploy` its own commit only.
#[test]
fn deploy_and_update_check_the_attested_component_and_commit() {
    use super::{deploy, update, Attested, AttestedScope};
    let (context, entity) = target(&gate_conf());
    let factory = context.factory();
    let _entered = context.enter();
    let location = std::env::temp_dir().join(format!("moqui-gate-deploy-{}", std::process::id()));
    deployed(
        &entity,
        &[
            ("source", "git"),
            ("status", "staged"),
            ("storage", "database"),
            ("commitId", COMMIT),
            ("location", &location.join("site").display().to_string()),
        ],
    );
    let problems = |report: crate::deploy::load::LoadReport| report.problems.join("; ");

    let refused = deploy(factory, "site", crate::deploy::load::Mode::Now, &[]);
    assert_eq!(problems(refused), crate::deploy::GATE_REFUSAL);
    let refused = update(factory, "site", &[]);
    assert_eq!(problems(refused), crate::deploy::GATE_REFUSAL);
    {
        let _scope = AttestedScope::enter(Attested {
            component: "other".to_string(),
            commit: COMMIT.to_string(),
            managed: false,
        });
        let refused = problems(deploy(factory, "site", crate::deploy::load::Mode::Now, &[]));
        assert!(refused.contains("the attestation is for other, not site"), "{refused}");
        let refused = problems(update(factory, "site", &[]));
        assert!(refused.contains("the attestation is for other, not site"), "{refused}");
    }
    {
        let _scope = AttestedScope::enter(Attested {
            component: "site".to_string(),
            commit: OTHER_COMMIT.to_string(),
            managed: false,
        });
        let refused = problems(deploy(factory, "site", crate::deploy::load::Mode::Now, &[]));
        assert!(
            refused.contains("the attestation is for site at fedcba987654"),
            "{refused}"
        );
    }
    {
        let _scope = AttestedScope::enter(Attested {
            component: "site".to_string(),
            commit: COMMIT.to_string(),
            managed: false,
        });
        // past the gate: the deploy goes on to the staged files, which this test did not write
        let report = problems(deploy(factory, "site", crate::deploy::load::Mode::Now, &[]));
        assert!(report.contains("staged files of site are gone"), "{report}");
    }
}

/// Under the gate a start writes from the database only the commit an attested deploy put in place (never the
/// repository's head), and another cluster member applies only the commit the last attestation names; with the gate
/// off, both as before.
#[test]
fn under_the_gate_a_start_and_another_member_write_only_an_attested_commit() {
    use super::{gate_remote, gate_restore};
    let (context, entity) = target(&gate_conf());
    let factory = context.factory();
    let _entered = context.enter();
    assert!(gate_remote(factory, "site", COMMIT).is_err(), "no attestation taken");
    assert!(gate_restore(factory, "site", COMMIT).is_err());
    {
        let _authz = context.disable_authz();
        let mut row = entity.make_value("moqui.server.DeployedComponentAttestation").unwrap();
        row.set("componentName", moqui_lib::object::Value::string("site"));
        row.set("commitId", moqui_lib::object::Value::string(OTHER_COMMIT));
        row.set("deployedCommitId", moqui_lib::object::Value::string(COMMIT));
        row.create(&entity).unwrap();
    }
    assert!(gate_remote(factory, "site", OTHER_COMMIT).is_ok());
    assert!(gate_remote(factory, "site", COMMIT).is_err());
    assert!(gate_remote(factory, "site", "").is_err());
    assert!(gate_restore(factory, "site", COMMIT).is_ok());
    assert!(gate_restore(factory, "site", OTHER_COMMIT).is_err());
    let err = gate_restore(factory, "site", "").unwrap_err();
    assert!(err.contains("its head"), "{err}");

    let open = ExecutionContextFactory::test_context();
    let _entered = open.enter();
    assert!(gate_remote(open.factory(), "site", COMMIT).is_ok());
    assert!(gate_restore(open.factory(), "site", "").is_ok());
}

/// A component deployed from a `db` repository (its source another `db` repository, `git://site-src`, so no network):
/// an attestation of a commit the branch has moved past is refused; `update` inside an attestation of another commit
/// than the branch's head is refused by its own commit check; an attestation of the head updates the component, and
/// the record then names that commit as the one a start writes.
#[test]
fn an_attested_update_puts_in_place_only_the_attested_head() {
    use super::{deploy_attested, gate_restore, update, Attested, AttestedScope};
    let (context, entity) = target(&gate_conf());
    let factory = context.factory();
    moqui_git::install(factory).unwrap();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let dir = std::env::temp_dir().join(format!("moqui-gate-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let git = moqui_git::provider(factory).unwrap();
    let repository = |id: &str, remote: Option<&str>| {
        let mut row = entity.make_value("moqui.resource.GitRepository").unwrap();
        row.set("repositoryId", moqui_lib::object::Value::string(id));
        row.set("repositoryKind", moqui_lib::object::Value::string("db"));
        row.set("branch", moqui_lib::object::Value::string("main"));
        if let Some(remote) = remote {
            row.set("remoteUrl", moqui_lib::object::Value::string(remote));
        }
        row.create(&entity).unwrap();
    };
    repository("site-src", None);
    let resources = factory.resource();
    resources
        .write_bytes("git://site-src/component.xml", br#"<component name="site"/>"#)
        .unwrap();
    let first = git.branch_head("site-src").unwrap().unwrap();
    repository("component-site", Some("git://site-src"));
    git.import("component-site", None).unwrap();
    let location = dir.join("addon").join("site");
    git.write_tree("component-site", Some(&first), &location).unwrap();
    deployed(
        &entity,
        &[
            ("source", "git"),
            ("status", "deployed"),
            ("storage", "database"),
            ("gitRepositoryId", "component-site"),
            ("commitId", &first),
            ("location", &location.display().to_string()),
            ("remoteUrl", SOURCE_URL),
            ("branch", "main"),
        ],
    );
    resources
        .write_bytes("git://site-src/screen/Site.xml", b"<screen/>")
        .unwrap();
    let second = git.branch_head("site-src").unwrap().unwrap();
    let now = moqui_core::service::util::now_millis();

    // the branch moved past the attested commit
    let mut moved = attestation(now - 2_000);
    moved.commit_id.clone_from(&first);
    let (text, signature) = signed(SECRET, &moved);
    let err = deploy_attested(factory, &text, &signature).unwrap_err();
    assert!(err.contains("deploy its new head"), "{err}");
    // update's own check of the commit it is about to put in place
    {
        let _scope = AttestedScope::enter(Attested {
            component: "site".to_string(),
            commit: first.clone(),
            managed: false,
        });
        let report = update(factory, "site", &[]);
        let problems = report.problems.join("; ");
        assert!(problems.contains("the attestation is for site at"), "{problems}");
        assert!(
            problems.contains(&format!("not site at {}", &second[..12])),
            "{problems}"
        );
    }
    // the head, attested: updated, and recorded as the commit a start writes
    let mut head = attestation(now);
    head.commit_id.clone_from(&second);
    let (text, signature) = signed(SECRET, &head);
    let done = deploy_attested(factory, &text, &signature).unwrap();
    assert_eq!(
        (done.status.as_str(), done.commit.as_str()),
        ("deployed", second.as_str())
    );
    assert!(location.join("screen/Site.xml").is_file());
    assert!(gate_restore(factory, "site", &second).is_ok());
    assert!(gate_restore(factory, "site", &first).is_err());

    // a managed deploy (plans/archive/Vault-Instances.md §4.6), its manager's over the pinned link with no signature: back to
    // the commit before, as a rollback goes, and recorded as the commit a start writes; a text that is not an
    // attestation of a passed run is refused
    let (text, _) = signed(SECRET, &attestation(now + 1_000));
    let mut back: moqui_git::ci::attest::Attestation = serde_json::from_str(&text).unwrap();
    back.commit_id.clone_from(&first);
    back.audience = String::new();
    back.issued_at = now + 1_000;
    // not marked a rollback: the branch's head only, as an attested deploy
    let err = super::deploy_managed(factory, &serde_json::to_string(&back).unwrap()).unwrap_err();
    assert!(err.contains("deploy its new head"), "{err}");
    back.issued_at = now + 1_500;
    let mut rollback = serde_json::to_value(&back).unwrap();
    rollback["rollback"] = serde_json::Value::Bool(true);
    let done = super::deploy_managed(factory, &rollback.to_string()).unwrap();
    assert_eq!(
        (done.status.as_str(), done.commit.as_str()),
        ("deployed", first.as_str())
    );
    assert!(!location.join("screen/Site.xml").is_file(), "back at the first commit");
    assert!(gate_restore(factory, "site", &first).is_ok());
    let mut failed = back.clone();
    failed.run_status = "failure".to_string();
    failed.issued_at = now + 2_000;
    let err = super::deploy_managed(factory, &serde_json::to_string(&failed).unwrap()).unwrap_err();
    assert!(err.contains("not an attestation of a passed run"), "{err}");
    assert!(super::deploy_managed(factory, "not json").is_err());
    let _ = std::fs::remove_dir_all(&dir);
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
