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

//! Host checks for field encryption, the audit log, and localized reads.

#![cfg(all(unix, feature = "embedded-postgres"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moqui_core::driver::{DbDriver, DbRow};
use moqui_core::entity::{EntityFacade, EntityList, EntityValue};
use moqui_core::transaction::PoolConfig;
use moqui_lib::error::Error;
use moqui_lib::object::Value;

use moqui_host::db::embedded::EmbeddedCluster;
use moqui_host::db::entity_facade::open_entity_facade;

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct CryptPassGuard;

impl Drop for CryptPassGuard {
    fn drop(&mut self) {
        moqui_lib::env::clear_property("entity_ds_crypt_pass");
    }
}

#[test]
fn encrypt_audit_and_localized_reads() {
    if !crate::common::own_process() {
        return;
    }
    let _lock = crate::common::serial();
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    moqui_lib::env::set_property("entity_ds_crypt_pass", "MoquiDefaultPassword:CHANGEME");
    let _crypt_pass = CryptPassGuard;
    let facade = open_entity_facade(
        &framework,
        driver,
        PoolConfig {
            min: 0,
            max: 4,
            reserve: 1,
            acquire_timeout: Duration::from_secs(30),
            ..Default::default()
        },
    )
    .unwrap();
    let (primary, _alts) = facade.crypt().expect("conf cipher");
    assert_eq!(
        moqui_core::entity::crypto::decrypt(":YWMS3u2Ct95vQrzfkJnX0ujHZwEyoTeuiqWrUb9dlEw=", &primary).unwrap(),
        "TestEntityFieldValue"
    );
    facade
        .check_tables(&[
            "moqui.server.instance.InstanceHost",
            "moqui.security.UserAccount",
            "moqui.entity.EntityAuditLog",
            "moqui.entity.SequenceValueItem",
            "moqui.basic.StatusItem",
            "moqui.basic.LocalizedEntityField",
        ])
        .unwrap();

    let mut host = facade.make_value("moqui.server.instance.InstanceHost").unwrap();
    host.set("instanceHostId", Value::string("H1"));
    host.set("password", Value::string("TestEntityFieldValue"));
    host.create(&facade).unwrap();
    assert_eq!(password_of(&facade).as_deref(), Some("TestEntityFieldValue"));
    let stored = stored_password(&facade);
    assert!(stored.contains(':'), "{stored}");
    assert_eq!(stored, ":YWMS3u2Ct95vQrzfkJnX0ujHZwEyoTeuiqWrUb9dlEw=");

    set_password(&facade, "legacy-plain");
    assert_eq!(password_of(&facade).as_deref(), Some("legacy-plain"));
    // As Java's `FieldInfo`, a value that does not decrypt reads as a clear placeholder, and writing it back is refused.
    let failed = moqui_core::entity::field_rw::DECRYPT_FAILED;
    set_password(&facade, "abcd");
    assert_eq!(password_of(&facade).as_deref(), Some(failed));
    set_password(&facade, ":not-base64");
    assert_eq!(password_of(&facade).as_deref(), Some(failed));
    let mut back = facade.make_value("moqui.server.instance.InstanceHost").unwrap();
    back.set("instanceHostId", Value::string("H1"));
    back.set("password", Value::string(failed));
    let err = back.update(&facade).unwrap_err();
    assert!(err.message().contains("prevent data loss"), "{err}");
    assert_eq!(stored_password(&facade), ":not-base64");

    let mut user = facade.make_value("moqui.security.UserAccount").unwrap();
    user.set("userId", Value::string("U1"));
    user.set("username", Value::string("alice"));
    user.create(&facade).unwrap();
    let created = username_audits(&facade);
    assert_eq!(created.len(), 1);
    let audit = created.get(0).unwrap();
    assert_eq!(audit.get("newValueText").and_then(Value::as_str), Some("alice"));
    assert_eq!(audit.get("pkPrimaryValue").and_then(Value::as_str), Some("U1"));
    assert!(audit.get("oldValueText").is_none_or(Value::is_null));
    assert!(audit.get("changedByUserId").is_none_or(Value::is_null));
    assert!(audit.get("changedInVisitId").is_none_or(Value::is_null));

    let mut same = facade.make_value("moqui.security.UserAccount").unwrap();
    same.set("userId", Value::string("U1"));
    same.set("username", Value::string("alice"));
    same.update(&facade).unwrap();
    assert_eq!(username_audits(&facade).len(), 1);

    let mut changed = facade.make_value("moqui.security.UserAccount").unwrap();
    changed.set("userId", Value::string("U1"));
    changed.set("username", Value::string("alice2"));
    changed.update(&facade).unwrap();
    let updated = username_audits(&facade);
    assert_eq!(updated.len(), 2);
    assert!(updated.iter().any(|row| {
        row.get("oldValueText").and_then(Value::as_str) == Some("alice")
            && row.get("newValueText").and_then(Value::as_str) == Some("alice2")
    }));

    facade.set_disable_audit_log(true);
    let mut other = facade.make_value("moqui.security.UserAccount").unwrap();
    other.set("userId", Value::string("U2"));
    other.set("username", Value::string("bob"));
    other.create(&facade).unwrap();
    assert_eq!(username_audits(&facade).len(), 2);

    facade.set_disable_audit_log(false);
    facade.set_sim_active(true);
    let mut sim_user = facade.make_value("moqui.security.UserAccount").unwrap();
    sim_user.set("userId", Value::string("U3"));
    sim_user.set("username", Value::string("cara"));
    sim_user.create(&facade).unwrap();
    facade.set_sim_active(false);
    assert_eq!(username_audits(&facade).len(), 2);

    let mut status = facade.make_value("moqui.basic.StatusItem").unwrap();
    status.set("statusId", Value::string("SI_TEST"));
    status.set("description", Value::string("Stored text"));
    status.create(&facade).unwrap();
    let mut localized = facade.make_value("moqui.basic.LocalizedEntityField").unwrap();
    localized.set("entityName", Value::string("moqui.basic.StatusItem"));
    localized.set("fieldName", Value::string("description"));
    localized.set("pkValue", Value::string("SI_TEST"));
    localized.set("locale", Value::string("fr"));
    localized.set("localized", Value::string("Texte francais"));
    localized.create(&facade).unwrap();

    facade.set_locale("en");
    assert_eq!(status_description(&facade), "Stored text");
    facade.set_locale("fr");
    assert_eq!(status_description(&facade), "Texte francais");
    facade.set_locale("fr_FR");
    assert_eq!(status_description(&facade), "Texte francais");
    facade.set_locale("en");
    assert_eq!(status_description(&facade), "Stored text");

    drop(facade);
    drop(cluster);
}

#[test]
fn two_threads_refill_one_sequence_bank() {
    let _lock = crate::common::shared();
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = open_entity_facade(
        &framework,
        driver,
        PoolConfig {
            min: 0,
            max: 4,
            reserve: 1,
            acquire_timeout: Duration::from_secs(30),
            ..Default::default()
        },
    )
    .unwrap();
    facade.check_tables(&["moqui.entity.SequenceValueItem"]).unwrap();
    let facade = Arc::new(facade);
    let mut joins = Vec::new();
    for _ in 0..2 {
        let facade = Arc::clone(&facade);
        joins.push(std::thread::spawn(move || {
            let mut ids = Vec::new();
            for _ in 0..40 {
                let mut value = facade.make_value("moqui.basic.Enumeration").unwrap();
                value.set_sequenced_id_primary(&facade).unwrap();
                ids.push(value.get("enumId").and_then(Value::as_str).unwrap_or("").to_string());
            }
            ids
        }));
    }
    let mut ids = Vec::new();
    for join in joins {
        ids.extend(join.join().unwrap());
    }
    let mut numbers: Vec<i64> = ids.iter().map(|id| id.parse::<i64>().unwrap()).collect();
    numbers.sort_unstable();
    let unique = numbers.len();
    numbers.dedup();
    assert_eq!(numbers.len(), unique);
    assert_eq!(numbers.len(), 80, "{numbers:?}");
    assert_eq!(numbers[0], 100_000);
    assert_eq!(*numbers.last().unwrap(), 100_079);
    let stored = facade
        .find("moqui.entity.SequenceValueItem")
        .unwrap()
        .eq("seqName", Value::string("moqui.basic.Enumeration"))
        .one()
        .unwrap()
        .unwrap();
    let seq_num = moqui_lib::object::basic_convert(stored.get("seqNum").unwrap(), "Long").value;
    assert_eq!(seq_num, Value::I64(100_101), "{stored:?}");
    drop(facade);
    drop(cluster);
}

fn password_result(facade: &EntityFacade) -> Result<Option<EntityValue>, Error> {
    facade
        .find("moqui.server.instance.InstanceHost")?
        .eq("instanceHostId", Value::string("H1"))
        .one()
}

fn password_of(facade: &EntityFacade) -> Option<String> {
    password_result(facade)
        .unwrap()
        .unwrap()
        .get("password")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn stored_password(facade: &EntityFacade) -> String {
    let tx = facade.transactions_for("moqui.server.instance.InstanceHost").unwrap();
    let rows = tx
        .with_connection(|conn| {
            conn.query(
                "SELECT password FROM instance_host WHERE instance_host_id = ?",
                &[Value::string("H1")],
            )
        })
        .unwrap();
    column_text(rows.first().unwrap(), "password")
}

fn set_password(facade: &EntityFacade, password: &str) {
    let tx = facade.transactions_for("moqui.server.instance.InstanceHost").unwrap();
    tx.with_connection(|conn| {
        conn.execute(
            "UPDATE instance_host SET password = ? WHERE instance_host_id = ?",
            &[Value::string(password), Value::string("H1")],
        )
    })
    .unwrap();
}

fn column_text(row: &DbRow, name: &str) -> String {
    row.get(name)
        .or_else(|| row.columns.first().map(|(_, value)| value))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn username_audits(facade: &EntityFacade) -> EntityList {
    facade
        .find("moqui.entity.EntityAuditLog")
        .unwrap()
        .eq("changedEntityName", Value::string("moqui.security.UserAccount"))
        .eq("changedFieldName", Value::string("username"))
        .list()
        .unwrap()
}

fn status_description(facade: &EntityFacade) -> String {
    facade
        .find("moqui.basic.StatusItem")
        .unwrap()
        .eq("statusId", Value::string("SI_TEST"))
        .one()
        .unwrap()
        .unwrap()
        .get_localized(facade, "description")
        .as_ref()
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn temp_cluster_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = format!("moqui-pg-{}-{nanos}", std::process::id());
    if Path::new("/tmp").is_dir() {
        PathBuf::from("/tmp").join(name)
    } else {
        std::env::temp_dir().join(name)
    }
}
