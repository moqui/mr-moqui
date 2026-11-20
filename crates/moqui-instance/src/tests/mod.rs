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

//! Managed instances against the in-memory entity facade (`moqui_core::testing::open()`) and the fake container engine
//! (moqui-lib's `docker-fake`), with the framework's own seed data (InstanceTypeData.xml, VaultTypeData.xml), an
//! operational vault whose members are the vault crate's own client sessions, and a recording database driver for
//! the conf connections `create#Database` makes: the rules of M1's Tests column (plans/Vault-Instances.md §6).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::driver::{ConfDb, DbCancel, DbConnection, DbDriver, DbRow};
use moqui_core::entity::EntityFacade;
use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};
use moqui_vault::ops::Session;
use moqui_vault::rng::OsRng;
use serde_json::{json, Value as Json};

mod docker_real;
mod kinds;
mod provision;

pub(super) fn setup() -> (ExecutionContext, EntityFacade) {
    keys();
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    );
    let facade = moqui_core::testing::open();
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    install_recorder();
    (ctx, facade)
}

/// The lease key's seed and the field pass, as a test's factory reads them (process properties of their environment names).
fn keys() {
    for (name, value) in [
        (
            "moqui_conf__vault__lease_key",
            "dGVzdC1sZWFzZS1rZXktc2VlZC0zMi1ieXRlcy0hISE=",
        ),
        (
            "moqui_conf__entity_facade__crypt_pass",
            "test-field-pass-for-the-machine-key",
        ),
    ] {
        if moqui_lib::env::get_property(name).is_none() {
            moqui_lib::env::set_property(name, value);
        }
    }
}

/// Loads a data file of the framework into the facade.
pub(super) fn load(facade: &EntityFacade, file: &str) {
    let path = moqui_core::context::framework_dir().join("data").join(file);
    sys(|| {
        moqui_core::entity::load_feed::load_entity_data(
            facade,
            &path.display().to_string(),
            &moqui_core::entity::load_feed::LoadOptions::default(),
        )
        .unwrap()
    });
}

pub(super) fn sys<T>(work: impl FnOnce() -> T) -> T {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    work()
}

pub(super) fn create(facade: &EntityFacade, entity: &str, fields: Json) {
    sys(|| {
        let mut row = facade.make_value(entity).unwrap();
        for (name, given) in fields.as_object().unwrap() {
            row.set(name.as_str(), moqui_lib::json::from_json(given));
        }
        row.create(facade).unwrap();
    });
}

/// A field of the row of `entity` whose key `key` is `id`, as text; empty when there is no row or no value.
pub(super) fn field_of(facade: &EntityFacade, entity: &str, key: &str, id: &str, name: &str) -> String {
    sys(|| {
        facade
            .find(entity)
            .unwrap()
            .eq(key, Value::string(id))
            .one()
            .unwrap()
            .and_then(|row| row.get(name).map(to_plain_string))
            .unwrap_or_default()
    })
}

/// Every field of every row of `entity` as text.
pub(super) fn all_text(facade: &EntityFacade, entity: &str) -> Vec<String> {
    sys(|| {
        facade
            .find(entity)
            .unwrap()
            .list()
            .unwrap()
            .iter()
            .flat_map(|row| row.fields().values().map(to_plain_string).collect::<Vec<String>>())
            .collect()
    })
}

pub(super) fn login(user_id: &str) {
    sys(|| moqui_core::user::internal_login_user(user_id).unwrap());
}

/// A strong, recent sign-in: an SSO login now.
pub(super) fn strong_login(user_id: &str) {
    let at = moqui_core::user::now_millis();
    sys(|| moqui_core::user::login_external_user(user_id, Some(at)).unwrap());
}

/// Calls a vault service as the signed-in user; its errors, or its results as JSON.
pub(super) fn vault_call(name: &str, parameters: Json) -> Result<Json, String> {
    let parameters: BTreeMap<String, Value> = parameters
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, given)| (key.clone(), moqui_lib::json::from_json(given)))
                .collect()
        })
        .unwrap_or_default();
    let answer = moqui_core::service::sync(&format!("org.moqui.security.VaultServices.{name}"), &parameters);
    let errors = ExecutionContext::with_current(|ec| {
        let text = ec.message().errors().join("\n");
        ec.message().clear_errors();
        text
    })
    .unwrap_or_default();
    match answer {
        Err(err) => Err(format!("{err} {errors}")),
        Ok(_) if !errors.is_empty() => Err(errors),
        Ok(out) => Ok(moqui_lib::json::to_json(&Value::Map(out.into_iter().collect()))),
    }
}

pub(super) fn new_id(prefix: &str) -> String {
    let mut bytes = [0_u8; 6];
    moqui_vault::rng::Rng::fill(&mut OsRng, &mut bytes);
    format!("{prefix}-{}", moqui_vault::text::hex(&bytes))
}

fn op(session: &mut Session, request: Json) -> Result<Json, String> {
    let answer: Json = serde_json::from_str(&session.call(&request.to_string(), &mut OsRng)).unwrap();
    if answer["ok"] == json!(true) {
        Ok(answer["result"].clone())
    } else {
        Err(answer["error"].as_str().unwrap_or_default().to_string())
    }
}

/// A user with their vault client, as a browser's worker is.
pub(super) struct Client {
    pub(super) user: String,
    key_id: String,
    public: String,
    session: Session,
}

impl Client {
    pub(super) fn new(facade: &EntityFacade, user: &str) -> Client {
        create(
            facade,
            "moqui.security.UserAccount",
            json!({ "userId": user, "username": user, "userFullName": format!("{user} Example") }),
        );
        let key_id = new_id("pk");
        let mut session = Session::default();
        let made = op(
            &mut session,
            json!({ "op": "personal_make", "personal_key_id": key_id, "user_id": user, "passphrase": "a long pass phrase" }),
        )
        .unwrap();
        strong_login(user);
        vault_call(
            "setup#PersonalKey",
            json!({ "personalKeyId": key_id, "public": made["public"], "passphraseWrap": made["passphrase_wrap"],
                    "recoveryWrap": made["recovery_wrap"] }),
        )
        .unwrap();
        sys(|| moqui_core::user::logout_local().unwrap());
        Client {
            user: user.to_string(),
            key_id,
            public: made["public"].as_str().unwrap_or_default().to_string(),
            session,
        }
    }

    pub(super) fn op(&mut self, request: Json) -> Result<Json, String> {
        op(&mut self.session, request)
    }

    pub(super) fn call(&self, name: &str, parameters: Json) -> Result<Json, String> {
        login(&self.user);
        vault_call(name, parameters)
    }

    fn checks(&self) -> Json {
        self.call("list#KeyChecks", json!({})).unwrap()["keyCheckList"].clone()
    }

    fn check(&mut self, other: &str) {
        let key = self.call("get#UserPublicKey", json!({ "userId": other })).unwrap();
        let check = self
            .op(json!({ "op": "key_check_make", "checked_key_id": key["personalKeyId"], "checked_user_id": key["userId"],
                        "checked_public": key["public"], "fingerprint": key["fingerprint"],
                        "checked_at": moqui_core::user::now_millis() }))
            .unwrap();
        self.call("store#KeyCheck", json!({ "check": check })).unwrap();
    }

    fn member(&self, role: &str) -> Json {
        json!({ "user_id": self.user, "role": role, "key_id": self.key_id, "public": self.public })
    }
}

/// An operational vault with its deposit key: `maker` manages it (and holds VAULT_ADMIN and INSTANCE_ADMIN),
/// `approver` approves and has checked the lease key.
pub(super) struct Vault {
    pub(super) maker: Client,
    pub(super) approver: Client,
    pub(super) vault_id: String,
}

pub(super) fn vault(facade: &EntityFacade, tag: &str) -> Vault {
    let mut maker = Client::new(facade, &format!("{tag}-maker"));
    let mut approver = Client::new(facade, &format!("{tag}-approver"));
    let group = format!("IT_{tag}");
    create(facade, "moqui.security.UserGroup", json!({ "userGroupId": group }));
    create(
        facade,
        "moqui.security.UserGroupMember",
        json!({ "userGroupId": group, "userId": maker.user, "fromDate": 0 }),
    );
    for permission in ["VAULT_ADMIN", "INSTANCE_ADMIN"] {
        create(
            facade,
            "moqui.security.UserGroupPermission",
            json!({ "userGroupId": group, "userPermissionId": permission, "fromDate": 0 }),
        );
    }
    maker.check(&approver.user);
    let vault_id = new_id("vault");
    let members = json!([maker.member("manage"), approver.member("approve")]);
    let checks = maker.checks();
    let entry = maker
        .op(json!({ "op": "key_version_first", "vault_id": vault_id, "members": members, "key_checks": checks }))
        .unwrap();
    maker
        .op(json!({ "op": "vault_key_new", "vault_id": vault_id, "key_version": 1 }))
        .unwrap();
    let mut wraps = Vec::new();
    for member in members.as_array().unwrap() {
        wraps.push(
            maker
                .op(
                    json!({ "op": "vault_key_wrap_member", "vault_id": vault_id, "key_version": 1,
                            "member_user_id": member["user_id"], "member_key_id": member["key_id"],
                            "member_public": member["public"], "key_checks": checks }),
                )
                .unwrap(),
        );
    }
    maker
        .call(
            "create#Vault",
            json!({ "vaultId": vault_id, "vaultKindEnumId": "VkOperational", "lockEnumId": "VlSharedKey",
                    "vaultName": tag, "entry": entry, "memberWraps": wraps }),
        )
        .unwrap();
    approver.check(&maker.user);
    let deposit = maker
        .op(json!({ "op": "deposit_key_new", "vault_id": vault_id, "key_version": 1 }))
        .unwrap();
    let revision = maker.call("get#Vault", json!({ "vaultId": vault_id })).unwrap()["vault"]["revision"].clone();
    maker
        .call(
            "set#VaultDepositKey",
            json!({ "vaultId": vault_id, "expectedRevision": revision, "deposit": deposit["deposit"],
                    "depositWrappedPrivate": deposit["wrapped_private"] }),
        )
        .unwrap();
    let key = approver.call("get#VaultLeaseKey", json!({})).unwrap();
    let check = approver
        .op(
            json!({ "op": "key_check_make", "lease_key": key["leaseKey"], "fingerprint": key["fingerprint"],
                    "checked_at": moqui_core::user::now_millis() }),
        )
        .unwrap();
    approver.call("store#KeyCheck", json!({ "check": check })).unwrap();
    Vault {
        maker,
        approver,
        vault_id,
    }
}

impl Vault {
    /// The maker seals an operational item and creates it, as their client does.
    pub(super) fn item(&mut self, item_type: &str, binding: &str, fields: &Json) {
        let item_id = new_id("item");
        let label = json!({ "item_type": item_type, "binding": binding, "name": binding });
        let body = json!({ "v": 1, "type": item_type, "name": binding, "fields": fields });
        let sealed = self
            .maker
            .op(
                json!({ "op": "item_seal", "vault_id": self.vault_id, "item_id": item_id, "key_version": 1,
                        "label": label, "body": body.to_string() }),
            )
            .unwrap();
        self.maker
            .call(
                "create#VaultItem",
                json!({ "vaultId": self.vault_id, "vaultItemId": item_id, "keyVersion": 1,
                        "wrappedItemKey": sealed["wrapped_item_key"], "bodyCipher": sealed["sealed_body"],
                        "vaultItemTypeId": item_type, "bindingKey": binding, "itemName": binding }),
            )
            .unwrap();
    }

    /// The approver opens the vault and approves the lease in their client, then stores the approval.
    pub(super) fn approve(&mut self, lease_id: &str) {
        let vault = self
            .approver
            .call("get#Vault", json!({ "vaultId": self.vault_id }))
            .unwrap();
        let wrap = vault["wraps"].as_array().unwrap()[0].clone();
        let checks = self.approver.checks();
        if let Err(err) = self
            .approver
            .op(json!({ "op": "vault_key_open", "entries": vault["entries"], "key_checks": checks, "wrap": wrap }))
        {
            assert!(err.contains("already open"), "{err}");
        }
        let got = self
            .approver
            .call("get#VaultLease", json!({ "leaseId": lease_id }))
            .unwrap();
        let key = self.approver.call("get#VaultLeaseKey", json!({})).unwrap();
        let approval = self
            .approver
            .op(json!({ "op": "lease_approve", "lease": got["lease"], "lease_key": key["leaseKey"], "key_checks": checks,
                        "wrapped_item_keys": got["wrappedItemKeys"], "now": moqui_core::user::now_millis() }))
            .unwrap();
        strong_login(&self.approver.user);
        vault_call(
            "approve#VaultLease",
            json!({ "leaseId": lease_id, "approval": approval }),
        )
        .unwrap();
    }
}

// ----- a recording driver for the conf connections of create#Database -----

/// A connection's conf (with its password, which the tests compare) and the statements run on it, by host.
#[derive(Debug, Clone, Default)]
pub(super) struct Recorded {
    pub(super) user: String,
    pub(super) password: String,
    pub(super) database: String,
    pub(super) sql: Vec<String>,
}

fn recorded() -> &'static Mutex<BTreeMap<String, Recorded>> {
    static RECORDED: OnceLock<Mutex<BTreeMap<String, Recorded>>> = OnceLock::new();
    RECORDED.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// What the conf connections to `host` did.
pub(super) fn recorded_for(host: &str) -> Recorded {
    recorded().lock().unwrap().get(host).cloned().unwrap_or_default()
}

fn install_recorder() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        moqui_core::driver::install_conf_driver_factory(Arc::new(|conf: &ConfDb| {
            let key = format!("{}:{}", conf.host, conf.port);
            {
                let mut all = recorded().lock().unwrap();
                let entry = all.entry(key.clone()).or_default();
                entry.user.clone_from(&conf.user);
                entry.password.clone_from(&conf.password);
                entry.database.clone_from(&conf.database);
            }
            Ok(Arc::new(Recorder { key }) as Arc<dyn DbDriver>)
        }));
    });
}

struct Recorder {
    key: String,
}

struct RecorderConn {
    key: String,
}

impl DbDriver for Recorder {
    fn connect(&self) -> Result<Box<dyn DbConnection>, Error> {
        Ok(Box::new(RecorderConn { key: self.key.clone() }))
    }
}

impl RecorderConn {
    fn note(&self, sql: &str) {
        recorded()
            .lock()
            .unwrap()
            .entry(self.key.clone())
            .or_default()
            .sql
            .push(sql.to_string());
    }
}

impl DbConnection for RecorderConn {
    fn execute(&mut self, sql: &str, _params: &[Value]) -> Result<u64, Error> {
        self.note(sql);
        Ok(1)
    }
    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
        self.query_once(sql, params)
    }
    fn query_once(&mut self, sql: &str, _params: &[Value]) -> Result<Vec<DbRow>, Error> {
        self.note(sql);
        Ok(vec![DbRow {
            columns: vec![("row_count".into(), Value::I64(0))],
            origins: None,
        }])
    }
    fn open_stream(&mut self, _sql: &str, _params: &[Value]) -> Result<(), Error> {
        Err(Error::new("no streams"))
    }
    fn next_row(&mut self) -> Result<Option<DbRow>, Error> {
        Ok(None)
    }
    fn close_stream(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn stream_open(&self) -> bool {
        false
    }
    fn begin(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn commit(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn rollback(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }
    fn rollback_to_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }
    fn release_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }
    fn canceller(&self) -> Box<dyn DbCancel> {
        Box::new(NoCancel)
    }
}

struct NoCancel;

impl DbCancel for NoCancel {
    fn cancel(&self) -> Result<(), Error> {
        Ok(())
    }
}

/// The Rust services register at the `moqui_instance::` locations the service file names, and at no other.
#[test]
fn the_services_register_at_the_locations_the_service_file_names() {
    let mut map = BTreeMap::new();
    super::register_services(&mut map).unwrap();
    let registered: Vec<&str> = map.keys().copied().collect();
    let path = moqui_core::context::framework_dir().join("service/org/moqui/dev/InstanceServices.xml");
    let text = std::fs::read_to_string(path).unwrap();
    let mut named: Vec<String> = text
        .split("location=\"")
        .skip(1)
        .filter_map(|rest| rest.split_once('"').map(|(location, _)| location.to_string()))
        .filter(|location| location.starts_with("moqui_instance::"))
        .collect();
    named.sort_unstable();
    named.dedup();
    assert_eq!(registered, named);
}
