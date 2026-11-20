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

//! An operational vault for another crate's tests (feature `test-support`): its manager and approver, each with the
//! vault crate's own client session as a browser's worker is, its items sealed in the manager's client, a lease
//! approved in the approver's client, and machine access given to an environment. Through the vault services, as a
//! client would call them; no shortcut past their rules.

use std::collections::BTreeMap;
use std::time::Duration;

use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;
use moqui_lib::object::Value;
use moqui_vault::ops::Session;
use moqui_vault::rng::OsRng;
use serde_json::{json, Value as Json};

use crate::lease::{registry, LeaseRegistry};

fn sys<T>(work: impl FnOnce() -> T) -> T {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    work()
}

fn create(facade: &EntityFacade, entity: &str, fields: &Json) {
    sys(|| {
        let mut row = facade.make_value(entity).expect("an entity of the framework");
        for (name, given) in fields.as_object().expect("fields as a map") {
            row.set(name.as_str(), moqui_lib::json::from_json(given));
        }
        row.create(facade).expect("a row the test makes");
    });
}

fn login(user_id: &str) {
    sys(|| moqui_core::user::internal_login_user(user_id).expect("a user the test made"));
}

/// A strong, recent sign-in: an SSO login now.
fn strong_login(user_id: &str) {
    let at = moqui_core::user::now_millis();
    sys(|| moqui_core::user::login_external_user(user_id, Some(at)).expect("a user the test made"));
}

/// Calls a vault service as the signed-in user; its errors, or its results as JSON.
///
/// # Errors
///
/// The service's errors.
pub fn vault_call(name: &str, parameters: &Json) -> Result<Json, String> {
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

fn new_id(prefix: &str) -> String {
    let mut bytes = [0_u8; 6];
    moqui_vault::rng::Rng::fill(&mut OsRng, &mut bytes);
    format!("{prefix}-{}", moqui_vault::text::hex(&bytes))
}

fn op(session: &mut Session, request: &Json) -> Result<Json, String> {
    let answer: Json =
        serde_json::from_str(&session.call(&request.to_string(), &mut OsRng)).map_err(|err| err.to_string())?;
    if answer["ok"] == json!(true) {
        Ok(answer["result"].clone())
    } else {
        Err(answer["error"].as_str().unwrap_or_default().to_string())
    }
}

/// The lease key's seed and the field pass, as a test's factory reads them (process properties of their environment
/// names), when the process has none.
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

/// The framework's item types (VaultTypeData.xml), when the facade has none.
fn item_types(facade: &EntityFacade) {
    sys(|| {
        let there = facade
            .find("moqui.security.vault.VaultItemType")
            .and_then(|find| find.count())
            .unwrap_or(0);
        if there == 0 {
            let path = moqui_core::context::framework_dir().join("data/VaultTypeData.xml");
            moqui_core::entity::load_feed::load_entity_data(
                facade,
                &path.display().to_string(),
                &moqui_core::entity::load_feed::LoadOptions::default(),
            )
            .expect("the vault's type data");
        }
    });
}

/// A user with their vault client.
pub struct Client {
    pub user: String,
    key_id: String,
    public: String,
    session: Session,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// A new user with a personal key.
    ///
    /// # Panics
    ///
    /// When the vault services refuse it.
    pub fn new(facade: &EntityFacade, user: &str) -> Client {
        create(
            facade,
            "moqui.security.UserAccount",
            &json!({ "userId": user, "username": user, "userFullName": format!("{user} Example") }),
        );
        let key_id = new_id("pk");
        let mut session = Session::default();
        let made = op(
            &mut session,
            &json!({ "op": "personal_make", "personal_key_id": key_id, "user_id": user,
                     "passphrase": "a long pass phrase" }),
        )
        .expect("a personal key");
        strong_login(user);
        vault_call(
            "setup#PersonalKey",
            &json!({ "personalKeyId": key_id, "public": made["public"], "passphraseWrap": made["passphrase_wrap"],
                     "recoveryWrap": made["recovery_wrap"] }),
        )
        .expect("the personal key set up");
        sys(|| moqui_core::user::logout_local().expect("a sign-out"));
        Client {
            user: user.to_string(),
            key_id,
            public: made["public"].as_str().unwrap_or_default().to_string(),
            session,
        }
    }

    /// A request to the client.
    ///
    /// # Errors
    ///
    /// The client's refusal.
    pub fn op(&mut self, request: &Json) -> Result<Json, String> {
        op(&mut self.session, request)
    }

    /// A vault service, as this user.
    ///
    /// # Errors
    ///
    /// The service's errors.
    pub fn call(&self, name: &str, parameters: &Json) -> Result<Json, String> {
        login(&self.user);
        vault_call(name, parameters)
    }

    fn checks(&self) -> Json {
        self.call("list#KeyChecks", &json!({})).expect("the key checks")["keyCheckList"].clone()
    }

    fn check(&mut self, other: &str) {
        let key = self
            .call("get#UserPublicKey", &json!({ "userId": other }))
            .expect("the other's key");
        let check = self
            .op(&json!({ "op": "key_check_make", "checked_key_id": key["personalKeyId"], "checked_user_id": key["userId"],
                         "checked_public": key["public"], "fingerprint": key["fingerprint"],
                         "checked_at": moqui_core::user::now_millis() }))
            .expect("a key check");
        self.call("store#KeyCheck", &json!({ "check": check }))
            .expect("the key check stored");
    }

    fn member(&self, role: &str) -> Json {
        json!({ "user_id": self.user, "role": role, "key_id": self.key_id, "public": self.public })
    }
}

/// An operational vault: `maker` manages it and holds `VAULT_ADMIN`; `approver` approves and has checked the lease key.
#[derive(Debug)]
pub struct OperationalVault {
    pub maker: Client,
    pub approver: Client,
    pub vault_id: String,
}

impl OperationalVault {
    /// A new vault, with users named for `tag`.
    ///
    /// # Panics
    ///
    /// When the vault services refuse it.
    pub fn new(facade: &EntityFacade, tag: &str) -> OperationalVault {
        keys();
        item_types(facade);
        let mut maker = Client::new(facade, &format!("{tag}-maker"));
        let mut approver = Client::new(facade, &format!("{tag}-approver"));
        let group = format!("VT_{tag}");
        create(facade, "moqui.security.UserGroup", &json!({ "userGroupId": group }));
        create(
            facade,
            "moqui.security.UserGroupMember",
            &json!({ "userGroupId": group, "userId": maker.user, "fromDate": 0 }),
        );
        create(
            facade,
            "moqui.security.UserGroupPermission",
            &json!({ "userGroupId": group, "userPermissionId": "VAULT_ADMIN", "fromDate": 0 }),
        );
        maker.check(&approver.user);
        let vault_id = new_id("vault");
        let members = json!([maker.member("manage"), approver.member("approve")]);
        let checks = maker.checks();
        let entry = maker
            .op(&json!({ "op": "key_version_first", "vault_id": vault_id, "members": members, "key_checks": checks }))
            .expect("the first key version");
        maker
            .op(&json!({ "op": "vault_key_new", "vault_id": vault_id, "key_version": 1 }))
            .expect("a vault key");
        let mut wraps = Vec::new();
        for member in members.as_array().into_iter().flatten() {
            wraps.push(
                maker
                    .op(
                        &json!({ "op": "vault_key_wrap_member", "vault_id": vault_id, "key_version": 1,
                                 "member_user_id": member["user_id"], "member_key_id": member["key_id"],
                                 "member_public": member["public"], "key_checks": checks }),
                    )
                    .expect("a member's wrap"),
            );
        }
        maker
            .call(
                "create#Vault",
                &json!({ "vaultId": vault_id, "vaultKindEnumId": "VkOperational", "lockEnumId": "VlSharedKey",
                         "vaultName": tag, "entry": entry, "memberWraps": wraps }),
            )
            .expect("the vault made");
        approver.check(&maker.user);
        let key = approver.call("get#VaultLeaseKey", &json!({})).expect("the lease key");
        let check = approver
            .op(
                &json!({ "op": "key_check_make", "lease_key": key["leaseKey"], "fingerprint": key["fingerprint"],
                         "checked_at": moqui_core::user::now_millis() }),
            )
            .expect("the lease key's check");
        approver
            .call("store#KeyCheck", &json!({ "check": check }))
            .expect("the lease key's check stored");
        OperationalVault {
            maker,
            approver,
            vault_id,
        }
    }

    /// The maker seals an operational item in their client and creates it; its id.
    ///
    /// # Panics
    ///
    /// When the vault services refuse it.
    pub fn item(&mut self, item_type: &str, binding: &str, fields: &Json) -> String {
        let item_id = new_id("item");
        let label = json!({ "item_type": item_type, "binding": binding, "name": binding });
        let body = json!({ "v": 1, "type": item_type, "name": binding, "fields": fields });
        let sealed = self
            .maker
            .op(
                &json!({ "op": "item_seal", "vault_id": self.vault_id, "item_id": item_id, "key_version": 1,
                         "label": label, "body": body.to_string() }),
            )
            .expect("the item sealed");
        self.maker
            .call(
                "create#VaultItem",
                &json!({ "vaultId": self.vault_id, "vaultItemId": item_id, "keyVersion": 1,
                         "wrappedItemKey": sealed["wrapped_item_key"], "bodyCipher": sealed["sealed_body"],
                         "vaultItemTypeId": item_type, "bindingKey": binding, "itemName": binding }),
            )
            .expect("the item created");
        item_id
    }

    /// The approver's client's approval of a lease (`approve#VaultLease`'s `approval`), not sent.
    ///
    /// # Panics
    ///
    /// When the client refuses it.
    pub fn approval(&mut self, lease_id: &str) -> Json {
        let vault = self
            .approver
            .call("get#Vault", &json!({ "vaultId": self.vault_id }))
            .expect("the vault");
        let wrap = vault["wraps"][0].clone();
        let checks = self.approver.checks();
        if let Err(err) = self
            .approver
            .op(&json!({ "op": "vault_key_open", "entries": vault["entries"], "key_checks": checks, "wrap": wrap }))
        {
            assert!(err.contains("already open"), "{err}");
        }
        let got = self
            .approver
            .call("get#VaultLease", &json!({ "leaseId": lease_id }))
            .expect("the lease");
        let key = self
            .approver
            .call("get#VaultLeaseKey", &json!({}))
            .expect("the lease key");
        self.approver
            .op(&json!({ "op": "lease_approve", "lease": got["lease"], "lease_key": key["leaseKey"], "key_checks": checks,
                         "wrapped_item_keys": got["wrappedItemKeys"], "now": moqui_core::user::now_millis() }))
            .expect("the lease approved in the client")
    }

    /// The approver signs in strongly, as the lease dialog asks, and approves the lease.
    ///
    /// # Panics
    ///
    /// When the client or the service refuses it.
    pub fn approve(&mut self, lease_id: &str) {
        let approval = self.approval(lease_id);
        strong_login(&self.approver.user);
        vault_call(
            "approve#VaultLease",
            &json!({ "leaseId": lease_id, "approval": approval }),
        )
        .expect("the lease approved");
    }

    /// Gives `environment_id` machine access to the items bound to `bindings` (§3.8): asked by the maker, approved by
    /// the approver, sealed on this member.
    ///
    /// # Panics
    ///
    /// When any step is refused.
    pub fn give_machine_access(&mut self, facade: &EntityFacade, environment_id: &str, bindings: &[String]) {
        let registry: std::sync::Arc<LeaseRegistry> =
            ExecutionContext::with_current(|ec| registry(ec.factory())).expect("an entered context");
        login(&self.maker.user);
        let lease = crate::machine::ask_grant(&registry, facade, environment_id, &self.vault_id, bindings)
            .expect("the grant's lease");
        self.approve(&lease.lease_id);
        crate::machine::complete_grant(&registry, facade, &lease.lease_id, Duration::from_secs(10))
            .expect("machine access given");
        sys(|| moqui_core::user::logout_local().expect("a sign-out"));
    }
}
