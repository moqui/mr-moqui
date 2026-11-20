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

//! What the instance modules share: the entity names, a row's fields, the parameters, the INSTANCE_ADMIN check, the
//! credentials a provision makes, and a host's client with its key from the thread's lease.

use std::collections::BTreeMap;

use moqui_core::context::{AuthzGuard, ExecutionContext};
use moqui_core::entity::{EntityFacade, EntityFind, EntityValue};
use moqui_lib::docker::{DockerHost, HostAddress};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use moqui_vault::rng::{OsRng, Rng};
use zeroize::Zeroizing;

pub(crate) type Params = BTreeMap<String, Value>;
pub(crate) type Out = Result<BTreeMap<String, Value>, LibError>;

pub(crate) const APP_INSTANCE: &str = "moqui.server.instance.AppInstance";
pub(crate) const INSTANCE_ENV: &str = "moqui.server.instance.AppInstanceEnv";
pub(crate) const INSTANCE_SECRET: &str = "moqui.server.instance.AppInstanceSecret";
pub(crate) const INSTANCE_DEPENDENCY: &str = "moqui.server.instance.AppInstanceDependency";
pub(crate) const INSTANCE_VOLUME: &str = "moqui.server.instance.AppInstanceVolume";
pub(crate) const INSTANCE_HOST_CONFIG: &str = "moqui.server.instance.AppInstanceHostConfig";
pub(crate) const ACTION: &str = "moqui.server.instance.InstanceAction";
pub(crate) const ENVIRONMENT: &str = "moqui.server.instance.InstanceEnvironment";
pub(crate) const HOST: &str = "moqui.server.instance.InstanceHost";
pub(crate) const HOST_TYPE: &str = "moqui.server.instance.InstanceHostType";
pub(crate) const IMAGE: &str = "moqui.server.instance.InstanceImage";
pub(crate) const KIND: &str = "moqui.server.instance.InstanceKind";
pub(crate) const KIND_ENV: &str = "moqui.server.instance.InstanceKindEnv";
pub(crate) const KIND_VOLUME: &str = "moqui.server.instance.InstanceKindVolume";
pub(crate) const KIND_HOST_CONFIG: &str = "moqui.server.instance.InstanceKindHostConfig";
pub(crate) const KIND_SECRET: &str = "moqui.server.instance.InstanceKindSecret";
pub(crate) const KIND_DEPENDENCY: &str = "moqui.server.instance.InstanceKindDependency";
pub(crate) const DATABASE_HOST: &str = "moqui.server.instance.DatabaseHost";

/// The permission every instance service asks, whatever calls it (plans/Security-Review.md SEC-TLS-02).
pub(crate) const INSTANCE_ADMIN: &str = "INSTANCE_ADMIN";
/// The package a host type's services must be of: a row names the service, so it cannot name any other.
pub(crate) const SERVICE_PACKAGE: &str = "org.moqui.dev.InstanceServices.";

pub(crate) fn fail(text: impl Into<String>) -> LibError {
    LibError::new(text)
}

/// Authorization off while a service reads and writes the instance rows: each service checks its caller itself.
pub(crate) fn authz_off() -> Option<AuthzGuard> {
    ExecutionContext::with_current(ExecutionContext::disable_authz)
}

/// INSTANCE_ADMIN, or a caller that runs with authorization disabled (a job, the framework's own call).
pub(crate) fn require_instance_admin() -> Result<(), LibError> {
    let disabled = ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false);
    if disabled || moqui_core::user::has_permission(INSTANCE_ADMIN).unwrap_or(false) {
        return Ok(());
    }
    Err(fail(format!("Managed instances need the {INSTANCE_ADMIN} permission")))
}

pub(crate) fn find(facade: &EntityFacade, entity: &str) -> Result<EntityFind, LibError> {
    Ok(facade.find(entity)?.disable_authz())
}

/// The row of `entity` whose single key field `key` is `id`.
pub(crate) fn one(facade: &EntityFacade, entity: &str, key: &str, id: &str) -> Result<Option<EntityValue>, LibError> {
    if id.is_empty() {
        return Ok(None);
    }
    find(facade, entity)?.eq(key, Value::string(id)).one()
}

/// The rows of `entity` whose `key` is `id`.
pub(crate) fn rows(facade: &EntityFacade, entity: &str, key: &str, id: &str) -> Result<Vec<EntityValue>, LibError> {
    Ok(find(facade, entity)?.eq(key, Value::string(id)).list()?.into_values())
}

pub(crate) fn field(row: &EntityValue, name: &str) -> String {
    row.get(name)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .unwrap_or_default()
}

pub(crate) fn text(params: &Params, name: &str) -> String {
    params
        .get(name)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub(crate) fn required(params: &Params, name: &str) -> Result<String, LibError> {
    let value = text(params, name);
    if value.is_empty() {
        return Err(fail(format!("{name} is required")));
    }
    Ok(value)
}

pub(crate) fn now() -> Value {
    moqui_core::user::now_value()
}

pub(crate) fn out<const N: usize>(pairs: [(&str, Value); N]) -> BTreeMap<String, Value> {
    pairs
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect()
}

/// A credential the manager makes: 32 letters and digits from the OS's randomness, about 190 bits, with no quote or
/// backslash (MySQL's default mode reads a backslash as an escape) and nothing a shell or a URL would read. Zeroized
/// when dropped.
pub(crate) fn new_credential() -> Zeroizing<String> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut out = Zeroizing::new(String::with_capacity(32));
    let mut bytes = Zeroizing::new([0_u8; 64]);
    OsRng.fill(&mut bytes[..]);
    // rejection sampling: 62 letters, so a byte of 248 or more is skipped and the choice stays uniform
    for byte in bytes.iter() {
        if out.len() == 32 {
            break;
        }
        if *byte < 248 {
            out.push(char::from(ALPHABET[usize::from(*byte % 62)]));
        }
    }
    while out.len() < 32 {
        let mut one = [0_u8; 1];
        OsRng.fill(&mut one);
        if one[0] < 248 {
            out.push(char::from(ALPHABET[usize::from(one[0] % 62)]));
        }
    }
    out
}

/// A new vault item id for a deposit: random, as a client's are.
pub(crate) fn new_item_id() -> String {
    let mut bytes = [0_u8; 10];
    OsRng.fill(&mut bytes);
    format!("item-{}", moqui_vault::text::hex(&bytes))
}

/// The value of a field of the item bound to `binding` in the lease the thread runs under (`ec.vault.leased`); each use
/// is a `VeLeaseUsed` event, written before the value comes back. Zeroized when dropped.
pub(crate) fn leased(binding: &str, field_name: &str) -> Result<Zeroizing<String>, LibError> {
    let value = moqui_core::vault_hooks::leased(binding, Some(field_name))?;
    Ok(Zeroizing::new(to_plain_string(&value)))
}

/// The binding of a host's TLS client key.
pub(crate) fn host_key_binding(host_id: &str) -> String {
    format!("InstanceHost:{host_id}:tlsClientKey")
}

/// Whether a host is reached with a client key, which is then a vault item: https with a client certificate.
pub(crate) fn host_needs_key(host: &EntityValue) -> bool {
    field(host, "hostProtocol").eq_ignore_ascii_case("https") && !field(host, "tlsClientCert").is_empty()
}

/// The client of an `InstanceHost` row; its client key, when it has one, from the thread's lease.
pub(crate) fn connect_host(host: &EntityValue) -> Result<DockerHost, LibError> {
    let opt = |name: &str| Some(field(host, name)).filter(|value| !value.is_empty());
    let key = if host_needs_key(host) {
        Some(leased(&host_key_binding(&field(host, "instanceHostId")), "privateKey")?)
    } else {
        None
    };
    let address = HostAddress {
        protocol: opt("hostProtocol"),
        address: opt("hostAddress"),
        port: opt("adminPort"),
        tls_ca_cert: opt("tlsCaCert"),
        tls_client_cert: opt("tlsClientCert"),
        tls_client_key: key.as_ref().map(|key| key.to_string()),
    };
    DockerHost::connect(&address, Some(2375))
}

/// Whether a host is this machine (a unix socket, or plain http to loopback), where a published port is bound to
/// loopback and reached there.
pub(crate) fn host_is_local(host: &EntityValue) -> bool {
    let protocol = field(host, "hostProtocol").to_ascii_lowercase();
    protocol == "unix" || (protocol != "https" && moqui_lib::net::loopback_host(&field(host, "hostAddress")))
}
