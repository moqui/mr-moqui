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

//! The secret store of S4 (plans/Sealed-Secrets.md §3.1): SecretValue rows sealed under their name and generation, a
//! generated secret made once into its row, a boot with no seal that keeps nothing, and the names a script never gets.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use moqui_lib::object::{to_plain_string, Value};

use super::*;
use crate::seal::{file, Keeper, Mode, SealConf};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "moqui-secrets-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn opened(scratch: &Scratch, purpose: &str) -> Arc<Keeper> {
    let keeper = Keeper::open(&file::path_in(&scratch.0), SealConf::with_purpose(purpose)).unwrap();
    keeper.start(Mode::Unattended).unwrap();
    Arc::new(keeper)
}

fn row(facade: &EntityFacade, name: &str) -> Option<crate::entity::EntityValue> {
    facade
        .find(ENTITY)
        .unwrap()
        .eq("secretName", Value::string(name))
        .one()
        .unwrap()
}

#[test]
fn a_secret_value_is_sealed_under_its_name_and_generation_and_a_moved_row_does_not_open() {
    let scratch = Scratch::new();
    let keeper = opened(&scratch, "dev");
    let facade = crate::testing::open();
    write_to(&keeper, Some(&facade), "ops-token", "the-ops-token-value").unwrap();
    assert_eq!(
        read_from(&keeper, &facade, "ops-token").unwrap().unwrap().as_str(),
        "the-ops-token-value"
    );
    let stored = row(&facade, "ops-token").unwrap();
    let sealed = to_plain_string(stored.get("sealedValue").unwrap());
    assert!(!sealed.contains("the-ops-token-value"));
    assert!(!sealed.contains(&encode(b"the-ops-token-value")));
    assert_eq!(generation_of(&stored), 1);
    write_to(&keeper, Some(&facade), "ops-token", "the-second-value").unwrap();
    assert_eq!(generation_of(&row(&facade, "ops-token").unwrap()), 2);

    // a database writer copies the sealed value to another name: it does not open there, and the others still read
    let mut moved = facade.make_value(ENTITY).unwrap();
    moved.set("secretName", Value::string("moved-token"));
    moved.set("generation", Value::I64(2));
    moved.set(
        "sealedValue",
        row(&facade, "ops-token").unwrap().get("sealedValue").unwrap().clone(),
    );
    moved.create(&facade).unwrap();
    let again = opened(&scratch, "dev");
    assert!(read_from(&again, &facade, "moved-token").unwrap().is_none());
    assert_eq!(
        read_from(&again, &facade, "ops-token").unwrap().unwrap().as_str(),
        "the-second-value"
    );
    // list names and generations, never a value; remove
    let listed = list_from(&again, Some(&facade)).unwrap();
    assert!(listed.contains(&Listed {
        name: "ops-token".to_string(),
        kind: "value",
        generation: 2
    }));
    assert!(remove_from(&again, Some(&facade), "ops-token").unwrap());
    assert!(row(&facade, "ops-token").is_none());
    assert!(read_from(&again, &facade, "ops-token").unwrap().is_none());
}

#[test]
fn a_bootstrap_secret_goes_to_seal_json_and_seal_drops_the_opened_values() {
    let scratch = Scratch::new();
    let keeper = opened(&scratch, "dev");
    let facade = crate::testing::open();
    write_to(&keeper, Some(&facade), DATASOURCE_PASSWORD, "db-password-value").unwrap();
    assert!(row(&facade, DATASOURCE_PASSWORD).is_none());
    assert_eq!(
        keeper
            .bootstrap_secret(DATASOURCE_PASSWORD)
            .unwrap()
            .unwrap()
            .as_slice(),
        b"db-password-value"
    );
    let text = std::fs::read_to_string(file::path_in(&scratch.0)).unwrap();
    assert!(!text.contains("db-password-value"));
    // the spelling of a selector does not matter
    assert!(is_bootstrap("entity-facade/datasource[transactional]@password"));
    assert!(is_bootstrap("entity-facade@crypt-pass") && is_bootstrap(EDGE_MEMBER_KEY));
    assert!(!is_bootstrap("webapp-list/webapp[webroot]/session-config@cookie-key"));

    write_to(&keeper, Some(&facade), "opened-token", "opened").unwrap();
    assert!(read_from(&keeper, &facade, "opened-token").unwrap().is_some());
    keeper.seal();
    assert!(read_from(&keeper, &facade, "opened-token").is_err());
    assert!(lock(&keeper.secret_values).values.is_empty());
}

#[test]
fn a_generated_secret_is_made_once_into_its_row_and_answers_the_conf_reader() {
    let scratch = Scratch::new();
    let keeper = opened(&scratch, "dev");
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context_with_conf(
        r#"<moqui-conf><push><web vapid-key="CHANGEME"/></push><entity-facade crypt-pass="MoquiDefaultPassword:CHANGEME"/></moqui-conf>"#,
    );
    ctx.factory().set_entity(facade.clone());
    ctx.factory().set_seal_keeper(Arc::clone(&keeper));
    let made = ctx
        .factory()
        .secret("push/web@vapid-key")
        .unwrap()
        .expect("made at first use");
    assert_eq!(made.len(), 44);
    assert_eq!(ctx.factory().secret("push/web@vapid-key").unwrap().unwrap(), made);
    assert!(row(&facade, "push/web@vapid-key").is_some());
    // the shim of the readers that take a String
    assert_eq!(
        ctx.factory().conf_value("push/web@vapid-key").unwrap().value,
        made.as_str()
    );
    // a restart reads the row, and does not make another
    let restarted = opened(&scratch, "dev");
    assert_eq!(
        read_from(&restarted, &facade, "push/web@vapid-key").unwrap().unwrap(),
        made
    );
    // a value the conf leaves at CHANGEME but the schema does not mark generated is not made
    assert!(ctx.factory().secret("push/fcm@service-account").unwrap().is_none());
}

#[test]
fn a_boot_with_no_seal_keeps_nothing_and_names_operator_init() {
    let scratch = Scratch::new();
    let keeper = opened(&scratch, "production");
    assert!(keeper.is_ephemeral());
    let facade = crate::testing::open();
    let err = write_to(&keeper, Some(&facade), "ops-token", "value").unwrap_err();
    assert!(err.to_string().contains("moqui operator init"), "{err}");
    // nothing is made under a memory-only root
    let err = bootstrap_or_make(&keeper, DATASOURCE_PASSWORD, random_secret).unwrap_err();
    assert!(err.to_string().contains("moqui operator init"), "{err}");
    // a generated session or signing key lives in this process's memory only: no row, no seal.json, no .env
    let ctx = ExecutionContextFactory::test_context_with_conf(
        r#"<moqui-conf><git-server lfs-token-key="CHANGEME"/><entity-facade crypt-pass="MoquiDefaultPassword:CHANGEME"/></moqui-conf>"#,
    );
    ctx.factory().set_entity(facade.clone());
    ctx.factory().set_seal_keeper(Arc::clone(&keeper));
    let held = ctx.factory().secret("git-server@lfs-token-key").unwrap().unwrap();
    assert_eq!(ctx.factory().secret("git-server@lfs-token-key").unwrap().unwrap(), held);
    assert!(row(&facade, "git-server@lfs-token-key").is_none());
    assert!(!file::path_in(&scratch.0).exists() && !scratch.0.join(".env").exists());

    // the crypt pass is left unset: the first encrypt fails naming the remedy, and `moqui load` refuses the boot
    assert!(ctx.factory().secret(CRYPT_PASS).unwrap().is_none());
    assert!(unsealed_without_crypt_pass(ctx.factory()));
    let conf = conf_with_secrets(ctx.factory()).unwrap();
    let pass = conf
        .first_named("entity-facade")
        .unwrap()
        .attribute("crypt-pass")
        .unwrap();
    assert!(pass.starts_with(crate::entity::crypto::REFUSED_PASS_PREFIX));
    let config = crate::entity::crypto::CryptConfig {
        pass,
        salt: String::new(),
        iterations: 10,
        algorithm: String::new(),
    };
    let err = crate::entity::crypto::encrypt("a card number", &config).unwrap_err();
    assert!(err.to_string().contains("moqui operator init"), "{err}");
    // a crypt pass the conf or the environment gives is used as given
    let given = ExecutionContextFactory::test_context_with_conf(
        r#"<moqui-conf><entity-facade crypt-pass="an-operator-given-pass"/></moqui-conf>"#,
    );
    given.factory().set_seal_keeper(Arc::clone(&keeper));
    assert!(!unsealed_without_crypt_pass(given.factory()));
    let conf = conf_with_secrets(given.factory()).unwrap();
    assert_eq!(
        conf.first_named("entity-facade")
            .unwrap()
            .attribute("crypt-pass")
            .unwrap(),
        "an-operator-given-pass"
    );
}

/// Two members that make the cluster TLS pair at once keep one pair: the second finds the first's rows and takes both,
/// never one half of each.
#[cfg(feature = "cluster-tls")]
#[test]
fn a_generated_pair_is_made_in_one_transaction_and_a_lost_race_takes_the_winners_pair() {
    let scratch = Scratch::new();
    let keeper = opened(&scratch, "dev");
    let facade = crate::testing::open();
    let key = keeper.secret_value_key().unwrap();
    let first = make(CLUSTER_CERT, Made::ClusterPair).unwrap();
    assert!(create_rows(&key, &facade, &first).unwrap());
    // the second member's pair finds the rows there and writes neither half
    let second = make(CLUSTER_KEY, Made::ClusterPair).unwrap();
    assert!(!create_rows(&key, &facade, &second).unwrap());
    let ctx = ExecutionContextFactory::test_context_with_conf(
        r#"<moqui-conf><cluster-member tls-cert="CHANGEME" tls-key="CHANGEME"/></moqui-conf>"#,
    );
    ctx.factory().set_entity(facade.clone());
    ctx.factory().set_seal_keeper(Arc::clone(&keeper));
    assert_eq!(ctx.factory().secret(CLUSTER_CERT).unwrap().unwrap(), first[0].1);
    assert_eq!(ctx.factory().secret(CLUSTER_KEY).unwrap().unwrap(), first[1].1);
}

/// `moqui operator secret rm` keeps the passes the fields and the connection depend on, and a new crypt pass needs
/// `--force` and keeps the old one as `crypt-pass-old`.
#[test]
fn the_crypt_pass_is_replaced_only_by_force_and_the_guarded_passes_are_not_removed() {
    let scratch = Scratch::new();
    let keeper = opened(&scratch, "dev");
    let facade = crate::testing::open();
    let old = keeper.bootstrap_secret(CRYPT_PASS).unwrap().expect("made at init");
    assert!(operator_put(&keeper, Some(&facade), CRYPT_PASS, "a-new-pass", false).is_err());
    operator_put(&keeper, Some(&facade), CRYPT_PASS, "a-new-pass", true).unwrap();
    assert_eq!(
        keeper.bootstrap_secret(CRYPT_PASS).unwrap().unwrap().as_slice(),
        b"a-new-pass"
    );
    assert_eq!(keeper.bootstrap_secret(CRYPT_PASS_OLD).unwrap().unwrap(), old);
    for name in [CRYPT_PASS, CRYPT_PASS_OLD, DATASOURCE_PASSWORD] {
        assert!(operator_remove(&keeper, Some(&facade), name).is_err(), "{name}");
    }
    write_to(&keeper, Some(&facade), "ops-token", "x").unwrap();
    assert!(operator_remove(&keeper, Some(&facade), "ops-token").unwrap());
}

/// A member whose seal is not the cluster's (it made its own against a shared database) is refused when it reads the
/// store, rather than making secrets of its own beside the cluster's.
#[test]
fn a_seal_under_which_no_stored_secret_opens_is_refused_with_an_alert() {
    let first_dir = Scratch::new();
    let second_dir = Scratch::new();
    let facade = crate::testing::open();
    let first = opened(&first_dir, "dev");
    write_to(&first, Some(&facade), "ops-token", "x").unwrap();
    let second = opened(&second_dir, "dev");
    let err = read_from(&second, &facade, "ops-token").unwrap_err();
    assert!(err.to_string().contains("not the one"), "{err}");
}

/// A script's `System.getProperty` answers no name the store keeps (SEC-SCR-03 extended to SecretValue names).
#[test]
fn a_name_the_store_keeps_is_a_secret_name() {
    let scratch = Scratch::new();
    let keeper = opened(&scratch, "dev");
    let facade = crate::testing::open();
    assert!(!is_secret_name("secrets-test-relay-token"));
    write_to(&keeper, Some(&facade), "secrets-test-relay-token", "value").unwrap();
    assert!(is_secret_name("secrets-test-relay-token") && is_secret_name("Secrets-Test-Relay-Token"));
    assert!(is_secret_name("tls-key/example.com") && is_secret_name(EDGE_MEMBER_KEY));
    assert!(!is_secret_name("secrets-test-plain-setting"));
}
