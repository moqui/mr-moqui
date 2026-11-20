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

//! The vault services against the in-memory entity facade (`moqui_core::testing::open()`), with clients that are the
//! vault crate's own `ops::Session`, as the browser module and `moqui vault` are: every rule of V1's Tests column
//! (plans/Vault-Instances.md §6) that the server keeps, and the client's refusals of what a server or a database
//! writer could hand it.

use std::collections::BTreeMap;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_lib::object::{to_plain_string, Value};
use moqui_vault::ops::Session;
use moqui_vault::rng::OsRng;
use serde_json::{json, Value as Json};

mod deliveries;
mod leases;
mod members;
mod personal;
mod recovery;
mod vaults;

const SERVICES: &str = "org.moqui.security.VaultServices.";

pub(super) fn setup() -> (ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    );
    let facade = moqui_core::testing::open();
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    (ctx, facade)
}

pub(super) fn sys<T>(work: impl FnOnce() -> T) -> T {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    work()
}

pub(super) fn make_user(facade: &EntityFacade, user_id: &str) {
    sys(|| {
        let mut row = facade.make_value("moqui.security.UserAccount").unwrap();
        row.set("userId", Value::string(user_id));
        row.set("username", Value::string(user_id));
        row.set("userFullName", Value::string(format!("{user_id} Example")));
        row.create(facade).unwrap();
    });
}

/// A sign-in that is not strong: an internal login.
pub(super) fn login(user_id: &str) {
    sys(|| moqui_core::user::internal_login_user(user_id).unwrap());
}

/// A strong sign-in: an SSO login at the provider's time `ago_millis` before now.
pub(super) fn strong_login(user_id: &str, ago_millis: i64) {
    let at = moqui_core::user::now_millis() - ago_millis;
    sys(|| moqui_core::user::login_external_user(user_id, Some(at)).unwrap());
}

/// Calls a vault service as the signed-in user; its errors, or its results as JSON.
pub(super) fn call(name: &str, parameters: Json) -> Result<Json, String> {
    let parameters: BTreeMap<String, Value> = parameters
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, given)| (key.clone(), moqui_lib::json::from_json(given)))
                .collect()
        })
        .unwrap_or_default();
    let answer = moqui_core::service::sync(&format!("{SERVICES}{name}"), &parameters);
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

/// `call` as `user_id`, after an internal login.
pub(super) fn call_as(user_id: &str, name: &str, parameters: Json) -> Result<Json, String> {
    login(user_id);
    call(name, parameters)
}

pub(super) fn text(value: &Json, name: &str) -> String {
    match &value[name] {
        Json::String(text) => text.clone(),
        Json::Null => String::new(),
        other => other.to_string(),
    }
}

/// A random client-made id.
pub(super) fn new_id(prefix: &str) -> String {
    let mut bytes = [0_u8; 8];
    moqui_vault::rng::Rng::fill(&mut OsRng, &mut bytes);
    format!("{prefix}-{}", moqui_vault::text::hex(&bytes))
}

/// A user with their vault client: the vault crate's session holding their personal key, as a browser's worker does.
pub(super) struct Client {
    pub(super) user: String,
    pub(super) key_id: String,
    pub(super) public: String,
    pub(super) made: Json,
    pub(super) session: Session,
}

impl Client {
    /// A new user, their personal key made in their client and set up on the server.
    pub(super) fn new(facade: &EntityFacade, user: &str) -> Client {
        make_user(facade, user);
        let key_id = new_id("pk");
        let mut session = Session::default();
        let request = json!({ "op": "personal_make", "personal_key_id": key_id, "user_id": user, "passphrase": "a long pass phrase" });
        let made = op(&mut session, request).unwrap();
        let setup = json!({ "personalKeyId": key_id, "public": made["public"], "passphraseWrap": made["passphrase_wrap"],
                            "recoveryWrap": made["recovery_wrap"] });
        strong_login(user, 0);
        call("setup#PersonalKey", setup).unwrap();
        // the strong sign-in ends with the setup: each later call signs in as the test asks
        sys(|| moqui_core::user::logout_local().unwrap());
        Client {
            user: user.to_string(),
            key_id,
            public: text(&made, "public"),
            made,
            session,
        }
    }

    pub(super) fn op(&mut self, request: Json) -> Result<Json, String> {
        op(&mut self.session, request)
    }

    pub(super) fn call(&self, name: &str, parameters: Json) -> Result<Json, String> {
        call_as(&self.user, name, parameters)
    }

    /// The caller's key checks as the server keeps them.
    pub(super) fn checks(&self) -> Json {
        self.call("list#KeyChecks", json!({})).unwrap()["keyCheckList"].clone()
    }

    /// Checks `other`'s active key: its fingerprint compared, the check signed here and stored.
    pub(super) fn check(&mut self, other: &str) {
        let key = self.call("get#UserPublicKey", json!({ "userId": other })).unwrap();
        let request = json!({ "op": "key_check_make", "checked_key_id": key["personalKeyId"], "checked_user_id": key["userId"], "checked_public": key["public"],
                              "fingerprint": key["fingerprint"], "checked_at": moqui_core::user::now_millis() });
        let check = self.op(request).unwrap();
        self.call("store#KeyCheck", json!({ "check": check })).unwrap();
    }

    pub(super) fn member(&self, role: &str) -> Json {
        json!({ "user_id": self.user, "role": role, "key_id": self.key_id, "public": self.public })
    }
}

pub(super) fn op(session: &mut Session, request: Json) -> Result<Json, String> {
    let answer: Json = serde_json::from_str(&session.call(&request.to_string(), &mut OsRng)).unwrap();
    if answer["ok"] == json!(true) {
        Ok(answer["result"].clone())
    } else {
        Err(answer["error"].as_str().unwrap_or_default().to_string())
    }
}

/// A shared-key personal vault made by `maker` with `others` (each `(client, role)`), its key wrapped to each after the
/// maker checked them. Its id.
pub(super) fn shared_vault(maker: &mut Client, others: &mut [(&mut Client, &str)]) -> String {
    let vault_id = new_id("vault");
    let mut members = vec![maker.member("manage")];
    for (other, role) in others.iter() {
        maker.check(&other.user);
        members.push(other.member(role));
    }
    let checks = maker.checks();
    let first = json!({ "op": "key_version_first", "vault_id": vault_id, "members": members, "key_checks": checks });
    let entry = maker.op(first).unwrap();
    maker
        .op(json!({ "op": "vault_key_new", "vault_id": vault_id, "key_version": 1 }))
        .unwrap();
    let mut wraps = Vec::new();
    for member in &members {
        let wrap = json!({ "op": "vault_key_wrap_member", "vault_id": vault_id, "key_version": 1,
                           "member_user_id": member["user_id"], "member_key_id": member["key_id"],
                           "member_public": member["public"], "key_checks": checks });
        wraps.push(maker.op(wrap).unwrap());
    }
    let create = json!({ "vaultId": vault_id, "vaultKindEnumId": "VkPersonal", "lockEnumId": "VlSharedKey",
                         "entry": entry, "memberWraps": wraps });
    maker.call("create#Vault", create).unwrap();
    // each other member checks the maker, the chain's first signer, so their client accepts it
    for (other, _) in others.iter_mut() {
        other.check(&maker.user);
    }
    vault_id
}

/// `client` opens its wrap of the vault's current key version through the chain, as its client does.
pub(super) fn open_vault(client: &mut Client, vault_id: &str) -> Result<Json, String> {
    let vault = client.call("get#Vault", json!({ "vaultId": vault_id }))?;
    let version = vault["vault"]["keyVersion"].as_u64().unwrap_or(0);
    let wrap = vault["wraps"]
        .as_array()
        .and_then(|wraps| wraps.iter().find(|wrap| wrap["key_version"].as_u64() == Some(version)))
        .cloned()
        .ok_or_else(|| "no wrap of the current version".to_string())?;
    let checks = client.checks();
    client.op(json!({ "op": "vault_key_open", "entries": vault["entries"], "key_checks": checks, "wrap": wrap }))
}

/// Seals a personal item in `client`'s session and creates it. Its id.
pub(super) fn add_item(client: &mut Client, vault_id: &str, key_version: u64, body: &Json) -> String {
    let item_id = new_id("item");
    let sealed = client
        .op(
            json!({ "op": "item_seal", "vault_id": vault_id, "item_id": item_id, "key_version": key_version,
                    "body": body.to_string() }),
        )
        .unwrap();
    let create = json!({ "vaultId": vault_id, "vaultItemId": item_id, "keyVersion": key_version,
                         "wrappedItemKey": sealed["wrapped_item_key"], "bodyCipher": sealed["sealed_body"] });
    client.call("create#VaultItem", create).unwrap();
    item_id
}

/// The event kinds of a vault, oldest first, as a member sees them.
pub(super) fn event_kinds(client: &Client, vault_id: &str) -> Vec<String> {
    let events = client.call("list#VaultEvents", json!({ "vaultId": vault_id })).unwrap();
    let mut kinds: Vec<String> = events["eventList"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| text(event, "eventTypeEnumId"))
        .collect();
    kinds.reverse();
    kinds
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

/// The Rust services register at the `moqui_vault_server::` locations the service file names, and at no other.
#[test]
fn the_services_register_at_the_locations_the_service_file_names() {
    let mut map = BTreeMap::new();
    super::register_services(&mut map).unwrap();
    let registered: Vec<&str> = map.keys().copied().collect();
    let path = moqui_core::context::framework_dir().join("service/org/moqui/security/VaultServices.xml");
    let text = std::fs::read_to_string(path).unwrap();
    let mut named: Vec<String> = text
        .split("location=\"")
        .skip(1)
        .filter_map(|rest| rest.split_once('"').map(|(location, _)| location.to_string()))
        .filter(|location| location.starts_with("moqui_vault_server::"))
        .collect();
    named.sort_unstable();
    named.dedup();
    assert_eq!(registered, named);
}
