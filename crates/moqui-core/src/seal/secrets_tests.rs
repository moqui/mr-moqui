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
    // a bootstrap secret it makes lives in memory only, the same for the rest of the process
    let made = bootstrap_or_make(
        &keeper,
        "entity-facade/datasource[secrets-test]@password",
        random_secret,
    )
    .unwrap();
    assert_eq!(
        bootstrap_or_make(
            &keeper,
            "entity-facade/datasource[secrets-test]@password",
            random_secret
        )
        .unwrap(),
        made
    );
    // a generated secret lives in this process's memory only: no row, no seal.json, no .env
    let ctx = ExecutionContextFactory::test_context_with_conf(
        r#"<moqui-conf><git-server lfs-token-key="CHANGEME"/></moqui-conf>"#,
    );
    ctx.factory().set_entity(facade.clone());
    ctx.factory().set_seal_keeper(Arc::clone(&keeper));
    let held = ctx.factory().secret("git-server@lfs-token-key").unwrap().unwrap();
    assert_eq!(ctx.factory().secret("git-server@lfs-token-key").unwrap().unwrap(), held);
    assert!(row(&facade, "git-server@lfs-token-key").is_none());
    assert!(!file::path_in(&scratch.0).exists() && !scratch.0.join(".env").exists());
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
