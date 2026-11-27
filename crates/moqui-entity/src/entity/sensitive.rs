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

//! The sensitive entities and `UserAccount` fields of Java's `WebUtilities`: what generic entity REST, Service REST
//! entity resources, the generic tools and the data loader keep away from a request. [`generic_access`] is the one
//! rule they share for an entity: whether generic access reads it, writes it, and which of its fields are secret (the
//! password fields, by name or origin) or encrypted, which no generic answer, condition, order or export holds.

use std::collections::BTreeMap;

use moqui_lib::object::Value;

/// `UserAccount` fields that are password material. Left out of REST JSON and not taken on generic entity writes.
pub const USER_ACCOUNT_SECRET_FIELDS: &[&str] = &[
    "currentPassword",
    "resetPassword",
    "passwordSalt",
    "passwordHashType",
    "passwordBase64",
];

/// `UserAccount` fields that control authentication and identity: the sign-in name, where a reset is mailed, the
/// provider link (`externalUserId`, `externalAuthFlowId`) and the key a key sign-in is checked by (`publicKey`), the
/// flags and the counters, and what ends, marks or recovers an account (`terminateDate`, which a cleared one reopens an
/// ended account by; `displayAccount`; `passwordHint`; `resetPasswordSetDate`; SEC-ADM-89). No generic write takes them,
/// a Service REST entity resource's included, an ADMIN's too: `update#UserAccount` changes them, with its rules.
pub const USER_ACCOUNT_IDENTITY_FIELDS: &[&str] = &[
    "disabled",
    "username",
    "emailAddress",
    "ipAllowed",
    "externalUserId",
    "externalAuthFlowId",
    "publicKey",
    "requirePasswordChange",
    "hasLoggedOut",
    "disabledDateTime",
    "successiveFailedLogins",
    "terminateDate",
    "displayAccount",
    "passwordHint",
    "resetPasswordSetDate",
];

/// Identity-admin entities (not `UserAccount`), by simple name or short alias in lower case.
const IDENTITY_ADMIN_ENTITY_KEYS: &[&str] = &[
    "userloginkey",
    "usergroupmember",
    "usergrouppermission",
    "userpermission",
    "artifactauthz",
    "artifactgroup",
    "artifactgroupmember",
    "artifacttarpit",
    "usergrouppermissions",
    "userpermissions",
    "artifactgroups",
    "userauthcfactor",
    "userpasswordhistory",
    // an account's own sign-in credentials, made through UserServices' key, app password and device services (which ask
    // whose account it is and what it holds, heldBeyond): a generic write could plant one on another account
    // (plans/Security-Review-Medium.md SEC-ADM-81)
    "usersshkey",
    "userapppassword",
    "devicecredential",
];

/// Secret-config entities: `EmailServer` (host and password) and `SystemMessageRemote` (HMAC secret).
const SECRET_CONFIG_ENTITY_KEYS: &[&str] = &["emailserver", "systemmessageremote"];

/// Entities only their own services read and write, never the generic tools, REST or request loads, an ADMIN's
/// included: `DatabaseAudit`, what ran outside the entity facade (§14.2); `SqlRunToken`, the one-time confirmations of
/// SQL runs with changes (§8.2), which a generic write could forge; and `DatabaseConnection`, an external database's
/// host and password (§4.3), which a generic write could point at another host to have the password sent there; and
/// the ETL mappings, value maps and runs (§10), whose `expression` fields run as the mapping's user, so a generic write
/// could plant a script in another user's mapping or take it over; and the database jobs and their statements (§6), whose
/// text `run#DatabaseJob` executes as written, so a generic write would run any SQL past the SQL runner's permission
/// and its confirmation (plans/Security-Review-Medium.md SEC-TLS-88); and a person's shared live locations
/// (plans/Location-Sharing.md §1), which no grant reads for anyone but the owner and the share's audience. No
/// entity-auto service writes them either.
const AUDIT_ENTITY_KEYS: &[&str] = &[
    "databaseaudit",
    // the replay keys of signed requests: a generic delete would let a captured message through again (SEC-DAT-90)
    "signaturereplay",
    "sqlruntoken",
    "databaseconnection",
    "databasejob",
    "databasejobstatement",
    "etlmapping",
    "etlmappingfield",
    "etlvaluemap",
    "etlvaluemapentry",
    "etlrun",
    "etlrunerror",
    // a person's shared live location (plans/Location-Sharing.md §1, "No other reader"): its shares, their audiences and
    // link digests, the latest position and the trail, which only org.moqui.maps.LocationServices reads and writes, for
    // the owner and the current audience; no generic tool, REST resource or agent reads them, an ADMIN's included
    "locationshare",
    "locationshareaudience",
    "locationshareaudiencemember",
    "locationshareowner",
    "locationlatest",
    "locationtrailpoint",
];

/// Managed-instance configuration (`moqui.server.instance`, plans/archive/Vault-Instances.md §4.1): a host type names the
/// services that run, a kind's templates and secrets say what goes into a container and which vault items it binds,
/// and a host, an image or a database host says where a leased login goes; so these rows are not written through the
/// generic tools, REST or request loads, where an `ADMIN` without the sealed `INSTANCE_ADMIN` could point a holder's
/// action at a service or a server of theirs (plans/Security-Review.md SEC-TLS-02). The environments and instances are only their services' to write
/// ([`SERVICE_WRITTEN_ENTITY_KEYS`]).
const HOST_CONFIG_ENTITY_KEYS: &[&str] = &[
    "instancehost",
    "instancehosttype",
    "instanceimage",
    "instancekind",
    "instancekindenv",
    "instancekindvolume",
    "instancekindhostconfig",
    "instancekindsecret",
    "instancekinddependency",
    "databasehost",
];

/// Entities the generic tools, REST and request loads read but never write, an ADMIN's included: the entity audit log
/// (`EntityAuditLog`), the evidence of every audited change, which the framework and the services that record their own
/// changes write (SEC-RTE-03); and Monitor's recovery actions, adapters, policies and emergency stop, its instance drains
/// and its ingest keys, whose `MONITOR_*` permissions only their own services check, so a generic write of a row would
/// run a recovery action, plant a working ingest key or lift the stop without them (SEC-TLS-11); and the scrape targets
/// and synthetic checks, whose `allowPrivateNetwork` only a holder of `MONITOR_PRIVATE_NETWORK` sets (SEC-OUT-03); and the
/// mail OAuth providers, whose token endpoint is posted the client secret and every mailbox's refresh token, so a generic
/// write could point it at another host (SEC-OUT-82): seed data, the conf and Admin's mail sign-in provider kind set them;
/// and the ACME names, challenges, certificates and order locks, which the TLS listener answers and each member writes to
/// `runtime/cert` as an order made them (SEC-OUT-81); and the network printers, whose server is sent each document printed
/// and whose `allowPrivateNetwork` and test-print gate `store#Connection` keeps (SEC-RTE-83); and the
/// service jobs, their parameters and their users, which the job runner calls with authorization off as the service a row
/// names, so a generic write would schedule any service, with any parameters, as any of its users (SEC-TLS-85). System ›
/// Operations, ETL's Schedule and the services that keep their own jobs write them through their own calls.
const SERVICE_WRITTEN_ENTITY_KEYS: &[&str] = &[
    "entityauditlog",
    "recoveryaction",
    "recoveryadapter",
    "recoverypolicy",
    "recoverycontrol",
    "instancedrain",
    "ingestkey",
    // a managed instance's environment (and so its machine access), dependencies and secret bindings decide which vault
    // items its provision opens, and its actions are its record (plans/archive/Vault-Instances.md §4.1)
    "instanceenvironment",
    "instancecluster",
    "appinstance",
    "appinstanceenv",
    "appinstancesecret",
    "appinstancedependency",
    "appinstancevolume",
    "appinstancehostconfig",
    "instanceaction",
    // the allowPrivateNetwork flag of a scrape target or synthetic check is set in Monitor/Checks, with its permission
    "scrapetarget",
    "syntheticcheck",
    // the token endpoint of a mail OAuth provider receives the client secret and the refresh tokens
    "emailoauthprovider",
    // a network printer's server is sent every document printed on it, and its allowPrivateNetwork is store#Connection's
    "networkprinter",
    // a sign-in provider (store#IdentityProvider, ConfigurationProviders.js, its only writer): the OAuth and OpenID
    // Connect flows, whose token endpoint is sent the client secret, which it clears when an endpoint changes; the SAML
    // flow, whose metadata location says whose assertions are believed; and the flow itself and its role mappings, whose
    // enabling, default group, account linking and groups need ADMIN_ACCESS_MANAGE there
    "authflow",
    "authflowrolemap",
    "oauthflow",
    "oidcflow",
    "samlflow",
    // a SIP trunk's stored password goes to its host: store#VoiceTrunk clears it when the endpoint changes
    "voicetrunk",
    // ACME: a challenge row is answered on the TLS listener and a certificate row written to runtime/cert, so only an
    // order (setup#AcmeName and the renewal job) writes them
    "acmename",
    "acmechallenge",
    "acmecertificate",
    "acmeorderlock",
    // a job's serviceName and parameters run with authorization off; its users decide whom it runs as
    "servicejob",
    "servicejobparameter",
    "servicejobuser",
    // a cluster member's row names where every member sends cluster messages, the keys it is pinned by and the
    // addresses let in to the cluster port and login; only the members' own heartbeat writes it (SEC-CLU-89), and a
    // cluster message is the members' own
    "clustermember",
    "clustermessage",
];

/// The text after the last `.`, in lower case.
fn simple_entity_key(entity_name: &str) -> String {
    let name = entity_name.trim();
    let simple = match name.rsplit_once('.') {
        Some((_, simple)) if !simple.is_empty() => simple,
        _ => name,
    };
    simple.to_ascii_lowercase()
}

/// `UserAccount` by full name, `users` alias, or any package.
pub fn is_user_account_entity(entity_name: &str) -> bool {
    entity_name == "moqui.security.UserAccount" || entity_name == "users" || entity_name.ends_with(".UserAccount")
}

/// Login keys, group membership and permissions, artifact authz, MFA factors and password history. Not `UserAccount`.
pub fn is_identity_admin_entity(entity_name: &str) -> bool {
    IDENTITY_ADMIN_ENTITY_KEYS.contains(&simple_entity_key(entity_name).as_str())
}

/// `EmailServer` and `SystemMessageRemote`.
pub fn is_secret_config_entity(entity_name: &str) -> bool {
    SECRET_CONFIG_ENTITY_KEYS.contains(&simple_entity_key(entity_name).as_str())
}

/// `DatabaseAudit`, `SqlRunToken`, `DatabaseConnection`, the ETL mappings and runs, the database jobs and the shared
/// live locations ([`AUDIT_ENTITY_KEYS`]): read and written only by their own services, never through generic entity
/// reads, writes or loads.
pub fn is_audit_entity(entity_name: &str) -> bool {
    AUDIT_ENTITY_KEYS.contains(&simple_entity_key(entity_name).as_str())
}

/// Entities the generic tools and loads do not take from a request entity name: identity-admin, secret-config and
/// audit. `UserAccount` stays available there.
pub fn is_restricted_generic_entity(entity_name: &str) -> bool {
    is_identity_admin_entity(entity_name)
        || is_secret_config_entity(entity_name)
        || is_audit_entity(entity_name)
        || is_host_config_entity(entity_name)
}

/// The entities only their own services write, though the generic tools read them ([`SERVICE_WRITTEN_ENTITY_KEYS`]).
pub fn is_service_written_entity(entity_name: &str) -> bool {
    SERVICE_WRITTEN_ENTITY_KEYS.contains(&simple_entity_key(entity_name).as_str())
}

/// Entities the generic tools, REST and request loads do not write: the restricted ones
/// ([`is_restricted_generic_entity`]) and those only their own services write ([`is_service_written_entity`]).
pub fn is_generic_write_refused(entity_name: &str) -> bool {
    is_restricted_generic_entity(entity_name) || is_service_written_entity(entity_name)
}

/// The hosted-instance configuration entities that name a command, a script or a service ([`HOST_CONFIG_ENTITY_KEYS`]).
pub fn is_host_config_entity(entity_name: &str) -> bool {
    HOST_CONFIG_ENTITY_KEYS.contains(&simple_entity_key(entity_name).as_str())
}

/// Whether `entity_name` is an `EmailServer` and `row` names one that a mail client account owns (an `EmailAccount` with
/// `ownsEmailServer` Y): a user's own mailbox server, which is changed through the account's settings and nothing else, so
/// no generic write takes it, an ADMIN's included. A row with no `emailServerId` names none.
pub fn account_owned_email_server(
    facade: &crate::entity::EntityFacade,
    entity_name: &str,
    row: &BTreeMap<String, Value>,
) -> Result<bool, moqui_lib::error::Error> {
    if simple_entity_key(entity_name) != "emailserver" {
        return Ok(false);
    }
    let server_id = row
        .get("emailServerId")
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default();
    if server_id.is_empty() {
        return Ok(false);
    }
    let owners = facade
        .find("moqui.basic.email.EmailAccount")?
        .eq("emailServerId", Value::string(server_id))
        .eq("ownsEmailServer", Value::string("Y"))
        .use_cache(false)
        .disable_authz()
        .count()?;
    Ok(owners > 0)
}

/// How deep [`secret_fields`], [`identity_fields`] and [`is_restricted_entity_or_view`] follow views of views.
const MAX_VIEW_DEPTH: u32 = 8;

fn definition_of(facade: &crate::entity::EntityFacade, name: &str) -> Option<moqui_db::model::EntityDefinition> {
    facade.with_entity(name, |entity, _, _| Ok(entity.clone())).ok()
}

/// Whether raw SQL text (a `complex-alias` expression, a `pq-alias`) names one of `fields` or its column.
fn sql_names(text: &str, fields: &[&str]) -> bool {
    let lower = text.to_ascii_lowercase();
    fields.iter().any(|field| {
        let column: String = field
            .chars()
            .flat_map(|ch| {
                if ch.is_ascii_uppercase() {
                    vec!['_', ch.to_ascii_lowercase()]
                } else {
                    vec![ch]
                }
            })
            .collect();
        lower.contains(&field.to_ascii_lowercase()) || lower.contains(&column)
    })
}

/// Which fields [`fields_by_origin`] looks for: `UserAccount`'s password fields, by name on any entity and in any
/// alias name, or its identity fields, only where they are `UserAccount`'s own.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Secret,
    Identity,
}

impl Kind {
    /// A field of an entity that is not a view.
    fn is_field(self, entity: &moqui_db::model::EntityDefinition, field: &str) -> bool {
        match self {
            Self::Secret => USER_ACCOUNT_SECRET_FIELDS.contains(&field),
            Self::Identity => {
                is_user_account_entity(&entity.full_name) && USER_ACCOUNT_IDENTITY_FIELDS.contains(&field)
            }
        }
    }

    /// A view's alias by its own name, whatever it reads.
    fn is_alias_name(self, name: &str) -> bool {
        self == Self::Secret && USER_ACCOUNT_SECRET_FIELDS.contains(&name)
    }

    /// Raw SQL of a view that names one; for identity fields, of a view that reads `UserAccount`.
    fn sql_names(self, text: &str, reads_accounts: bool) -> bool {
        match self {
            Self::Secret => sql_names(text, USER_ACCOUNT_SECRET_FIELDS),
            Self::Identity => reads_accounts && sql_names(text, USER_ACCOUNT_IDENTITY_FIELDS),
        }
    }
}

/// Whether `entity` is `UserAccount` or a view any of whose members (through views of views) is.
fn reads_user_accounts(
    facade: &crate::entity::EntityFacade,
    entity: &moqui_db::model::EntityDefinition,
    depth: u32,
) -> bool {
    if is_user_account_entity(&entity.full_name) {
        return true;
    }
    entity.is_view
        && (depth > MAX_VIEW_DEPTH
            || entity.member_entities.iter().any(|member| {
                is_user_account_entity(&member.entity_name)
                    || definition_of(facade, &member.entity_name)
                        .is_some_and(|definition| reads_user_accounts(facade, &definition, depth + 1))
            }))
}

/// The fields of `entity` that are of `kind`: on an entity, by name ([`Kind::is_field`]); on a view (a view entity, a
/// `DbViewEntity`, a `DataDocument`) each alias that reads one from a member, through views of views, in a complex or
/// case alias too, or whose raw SQL names one.
fn fields_by_origin(
    facade: &crate::entity::EntityFacade,
    entity: &moqui_db::model::EntityDefinition,
    depth: u32,
    kind: Kind,
) -> Vec<String> {
    use moqui_db::model::AliasExpr;
    if !entity.is_view {
        return entity
            .fields
            .iter()
            .filter(|field| kind.is_field(entity, &field.name))
            .map(|field| field.name.clone())
            .collect();
    }
    if depth > MAX_VIEW_DEPTH {
        // too deep to follow: every alias is treated as one
        return entity.aliases.iter().map(|alias| alias.name.clone()).collect();
    }
    let reads_accounts = kind == Kind::Identity && reads_user_accounts(facade, entity, depth);
    let member_name = |entity_alias: &str| {
        entity
            .member_entities
            .iter()
            .find(|member| member.entity_alias == entity_alias)
            .map(|member| member.entity_name.clone())
    };
    let mut member_fields: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut from_origin = |member: Option<String>, field: &str| {
        let Some(member) = member.filter(|member| !member.is_empty()) else {
            return kind.is_alias_name(field);
        };
        let found = member_fields.entry(member.clone()).or_insert_with(|| {
            definition_of(facade, &member)
                .map(|definition| fields_by_origin(facade, &definition, depth + 1, kind))
                .unwrap_or_default()
        });
        kind.is_alias_name(field) || found.iter().any(|name| name == field)
    };
    let mut out = Vec::new();
    for alias in &entity.aliases {
        let found = kind.is_alias_name(&alias.name)
            || match &alias.expr {
                AliasExpr::Column => {
                    let member = alias
                        .member_entity
                        .clone()
                        .or_else(|| alias.entity_alias.as_deref().and_then(member_name));
                    from_origin(member, alias.field_name())
                }
                AliasExpr::Complex(complex) => {
                    let mut fields = Vec::new();
                    complex.visit_fields(&mut |field| fields.push((field.entity_alias.clone(), field.field.clone())));
                    complex
                        .expression
                        .as_deref()
                        .is_some_and(|text| kind.sql_names(text, reads_accounts))
                        || fields
                            .into_iter()
                            .any(|(member, field)| from_origin(member_name(&member), &field))
                }
                AliasExpr::Case(case) => {
                    let mut fields = Vec::new();
                    for (_, then) in &case.whens {
                        then.visit_fields(&mut |field| fields.push((field.entity_alias.clone(), field.field.clone())));
                    }
                    if let Some(otherwise) = &case.otherwise {
                        otherwise
                            .visit_fields(&mut |field| fields.push((field.entity_alias.clone(), field.field.clone())));
                    }
                    case.expression
                        .as_deref()
                        .is_some_and(|text| kind.sql_names(text, reads_accounts))
                        || fields
                            .into_iter()
                            .any(|(member, field)| from_origin(member_name(&member), &field))
                }
            };
        if found {
            out.push(alias.name.clone());
        }
    }
    for alias in &entity.pq_aliases {
        if kind.is_alias_name(&alias.name) || kind.sql_names(&alias.expression, reads_accounts) {
            out.push(alias.name.clone());
        }
    }
    out
}

/// The fields of `entity_name` whose value is a `UserAccount` password field, whatever their name: by name on any
/// entity, and in a view (a view entity, a `DbViewEntity`, a `DataDocument`) each alias that reads one from a member,
/// through views of views, in a complex or case alias too, or whose raw SQL names one. An entity that does not resolve
/// has none here; its find fails on its own.
#[must_use]
pub fn secret_fields(facade: &crate::entity::EntityFacade, entity_name: &str) -> Vec<String> {
    match definition_of(facade, entity_name) {
        Some(entity) => fields_by_origin(facade, &entity, 0, Kind::Secret),
        None => Vec::new(),
    }
}

/// The fields of `entity_name` whose value is a `UserAccount` identity field ([`USER_ACCOUNT_IDENTITY_FIELDS`]),
/// whatever their name: `UserAccount`'s own, and in a view each alias that reads one from a `UserAccount` member,
/// through views of views, in a complex or case alias too, or whose raw SQL names one in a view that reads
/// `UserAccount`. An entity that does not resolve has none here.
#[must_use]
pub fn identity_fields(facade: &crate::entity::EntityFacade, entity_name: &str) -> Vec<String> {
    match definition_of(facade, entity_name) {
        Some(entity) => fields_by_origin(facade, &entity, 0, Kind::Identity),
        None => Vec::new(),
    }
}

/// The fields of `entity_name` stored encrypted (`encrypt="true"`): on a view, each column alias of an encrypted member
/// field, which the view inherits.
#[must_use]
pub fn encrypted_fields(facade: &crate::entity::EntityFacade, entity_name: &str) -> Vec<String> {
    let Some(entity) = definition_of(facade, entity_name) else {
        return Vec::new();
    };
    if entity.is_view {
        entity
            .aliases
            .iter()
            .filter(|alias| alias.encrypt)
            .map(|alias| alias.name.clone())
            .collect()
    } else {
        entity
            .fields
            .iter()
            .filter(|field| field.encrypt)
            .map(|field| field.name.clone())
            .collect()
    }
}

/// What generic entity access (entity REST, Service REST entity resources, search forms, data documents, the data
/// writer, Dev AIDE's export) may do with one entity: the one rule they share.
#[derive(Debug, Clone, Default)]
pub struct GenericAccess {
    /// Not restricted, nor a view over a restricted entity ([`is_restricted_entity_or_view`]).
    pub read: bool,
    /// Neither restricted nor written only by its own services ([`is_generic_write_refused`]).
    pub write: bool,
    /// The password fields, by name or by origin through a view ([`secret_fields`]): never answered, never a condition.
    pub secret: Vec<String>,
    /// The encrypted fields ([`encrypted_fields`]): answered only as whether one is set, never a condition.
    pub encrypted: Vec<String>,
}

impl GenericAccess {
    /// A field generic access never answers in clear or takes a condition or order on: a password field (by name on
    /// any entity, or by origin through a view) or an encrypted one.
    #[must_use]
    pub fn hidden(&self, field: &str) -> bool {
        USER_ACCOUNT_SECRET_FIELDS.contains(&field)
            || self.secret.iter().any(|name| name == field)
            || self.encrypted.iter().any(|name| name == field)
    }

    /// A password field, by name on any entity or by origin through a view.
    #[must_use]
    pub fn is_secret(&self, field: &str) -> bool {
        USER_ACCOUNT_SECRET_FIELDS.contains(&field) || self.secret.iter().any(|name| name == field)
    }

    /// The fields of one record as generic access answers them: the password fields left out, and each encrypted
    /// field replaced by `<field>IsSet`, true when it has a value.
    #[must_use]
    pub fn conceal(&self, fields: Vec<(String, Value)>) -> Vec<(String, Value)> {
        let mut out = Vec::with_capacity(fields.len());
        for (name, value) in fields {
            if self.encrypted.contains(&name) {
                out.push((
                    format!("{name}IsSet"),
                    Value::Bool(!moqui_lib::object::is_empty(&value)),
                ));
            } else if !self.hidden(&name) {
                out.push((name, value));
            }
        }
        out
    }

    /// The fields of one record as a file of records (an export) holds them: the password and encrypted fields left
    /// out.
    #[must_use]
    pub fn drop_hidden(&self, fields: Vec<(String, Value)>) -> Vec<(String, Value)> {
        fields.into_iter().filter(|(name, _)| !self.hidden(name)).collect()
    }
}

/// [`GenericAccess`] of `entity_name`.
#[must_use]
pub fn generic_access(facade: &crate::entity::EntityFacade, entity_name: &str) -> GenericAccess {
    let full_name =
        definition_of(facade, entity_name).map_or_else(|| entity_name.to_string(), |entity| entity.full_name);
    let read = !is_restricted_entity_or_view(facade, entity_name);
    GenericAccess {
        read,
        write: read && !is_generic_write_refused(entity_name) && !is_generic_write_refused(&full_name),
        secret: secret_fields(facade, entity_name),
        encrypted: encrypted_fields(facade, entity_name),
    }
}

fn restricted_definition(
    facade: &crate::entity::EntityFacade,
    entity: &moqui_db::model::EntityDefinition,
    depth: u32,
) -> bool {
    if is_restricted_generic_entity(&entity.full_name) || is_restricted_generic_entity(&entity.entity_name) {
        return true;
    }
    if !entity.is_view {
        return false;
    }
    if depth > MAX_VIEW_DEPTH {
        return true;
    }
    entity.member_entities.iter().any(|member| {
        is_restricted_generic_entity(&member.entity_name)
            || definition_of(facade, &member.entity_name)
                .is_some_and(|definition| restricted_definition(facade, &definition, depth + 1))
    })
}

/// [`is_restricted_generic_entity`], and a view any of whose members (through views of views) is restricted: a view
/// over `UserGroupMember` (`UserGroupMemberUser`) reads what the entity refuses.
#[must_use]
pub fn is_restricted_entity_or_view(facade: &crate::entity::EntityFacade, entity_name: &str) -> bool {
    if is_restricted_generic_entity(entity_name) {
        return true;
    }
    definition_of(facade, entity_name).is_some_and(|entity| restricted_definition(facade, &entity, 0))
}

/// `value` with the fields named in `names` left out of every map, at any depth: [`secret_fields`] of a view, whose
/// aliases may rename a password field.
pub fn strip_fields(value: Value, names: &[String]) -> Value {
    match value {
        Value::Map(map) => Value::Map(
            map.into_iter()
                .filter(|(key, _)| !names.contains(key))
                .map(|(key, value)| (key, strip_fields(value, names)))
                .collect(),
        ),
        Value::List(items) => Value::List(items.into_iter().map(|item| strip_fields(item, names)).collect()),
        other => other,
    }
}

/// Drop the password fields from a write's parameters.
pub fn remove_user_account_secrets(parameters: &mut BTreeMap<String, Value>) {
    for field in USER_ACCOUNT_SECRET_FIELDS {
        parameters.remove(*field);
    }
}

/// Drop the identity-control fields from a write's parameters.
pub fn remove_user_account_identity(parameters: &mut BTreeMap<String, Value>) {
    for field in USER_ACCOUNT_IDENTITY_FIELDS {
        parameters.remove(*field);
    }
}

/// `value` with the `UserAccount` password fields left out of every map, at any depth.
pub fn strip_user_account_secrets(value: Value) -> Value {
    match value {
        Value::Map(map) => Value::Map(
            map.into_iter()
                .filter(|(key, _)| !USER_ACCOUNT_SECRET_FIELDS.contains(&key.as_str()))
                .map(|(key, value)| (key, strip_user_account_secrets(value)))
                .collect(),
        ),
        Value::List(items) => Value::List(items.into_iter().map(strip_user_account_secrets).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use moqui_lib::object::Value;

    use super::{
        is_audit_entity, is_generic_write_refused, is_host_config_entity, is_identity_admin_entity,
        is_restricted_generic_entity, is_secret_config_entity, is_service_written_entity, is_user_account_entity,
        simple_entity_key, strip_user_account_secrets,
    };

    #[test]
    fn the_simple_key_is_the_name_after_the_last_dot() {
        assert_eq!(simple_entity_key(" moqui.security.UserLoginKey "), "userloginkey");
        assert_eq!(simple_entity_key("UserLoginKey"), "userloginkey");
        assert_eq!(simple_entity_key("moqui.Été"), "Été".to_ascii_lowercase());
        // A trailing dot leaves the whole name.
        assert_eq!(simple_entity_key("moqui.security."), "moqui.security.");
    }

    /// Java's `SecurityMisconfigTests` names that this port still classifies, plus the other keys
    /// `is_identity_admin_entity` lists. `UserAccount`, `users`, `Enumeration` and an empty name stay unrestricted.
    #[test]
    fn entity_names_are_classified_as_java_does() {
        for (name, identity, secret) in [
            ("moqui.security.UserLoginKey", true, false),
            ("UserLoginKey", true, false),
            ("UserGroupMember", true, false),
            ("moqui.security.UserGroupPermission", true, false),
            ("UserPermission", true, false),
            ("moqui.security.ArtifactAuthz", true, false),
            ("ArtifactGroup", true, false),
            ("moqui.security.ArtifactGroupMember", true, false),
            ("ArtifactTarpit", true, false),
            ("usergrouppermissions", true, false),
            ("userpermissions", true, false),
            ("artifactgroups", true, false),
            ("userauthcfactor", true, false),
            ("moqui.security.UserAuthcFactor", true, false),
            ("moqui.security.UserPasswordHistory", true, false),
            ("moqui.security.UserSshKey", true, false),
            ("UserAppPassword", true, false),
            ("moqui.security.DeviceCredential", true, false),
            ("moqui.basic.email.EmailServer", false, true),
            ("moqui.service.message.SystemMessageRemote", false, true),
            ("moqui.security.UserAccount", false, false),
            ("users", false, false),
            ("moqui.basic.Enumeration", false, false),
            ("", false, false),
        ] {
            assert_eq!(is_identity_admin_entity(name), identity, "{name}");
            assert_eq!(is_secret_config_entity(name), secret, "{name}");
            assert_eq!(is_restricted_generic_entity(name), identity || secret, "{name}");
        }
        // the database audit: restricted, and no one writes it generically
        assert!(is_audit_entity("moqui.database.DatabaseAudit"));
        assert!(is_restricted_generic_entity("moqui.database.DatabaseAudit"));
        assert!(is_audit_entity("moqui.database.SqlRunToken"));
        assert!(is_audit_entity("moqui.database.DatabaseConnection"));
        assert!(!is_secret_config_entity("moqui.database.DatabaseConnection"));
        assert!(is_restricted_generic_entity("moqui.database.DatabaseConnection"));
        assert!(!is_audit_entity("moqui.entity.EntityAuditLog"));
        // shared live locations: only their own services read them, never a generic read, an ADMIN's included
        for name in [
            "moqui.maps.LocationShare",
            "moqui.maps.LocationShareAudience",
            "moqui.maps.LocationShareAudienceMember",
            "moqui.maps.LocationShareOwner",
            "moqui.maps.LocationLatest",
            "moqui.maps.LocationTrailPoint",
        ] {
            assert!(is_audit_entity(name), "{name}");
            assert!(is_restricted_generic_entity(name), "{name}");
        }
        // a database job's statements run as written: no generic read or write, whose text would run past the SQL
        // runner's permission (SEC-TLS-88)
        for name in [
            "moqui.database.DatabaseJob",
            "moqui.database.DatabaseJobStatement",
            "DatabaseJobStatement",
        ] {
            assert!(is_audit_entity(name), "{name}");
            assert!(is_generic_write_refused(name), "{name}");
            assert!(is_restricted_generic_entity(name), "{name}");
        }
        // read by the generic tools, written only by their own services (SEC-RTE-03, SEC-TLS-11, SEC-OUT-82)
        for name in [
            "moqui.security.sso.AuthFlow",
            "moqui.security.sso.AuthFlowRoleMap",
            "moqui.security.sso.OauthFlow",
            "moqui.security.sso.OidcFlow",
            "moqui.security.sso.SamlFlow",
            "moqui.call.VoiceTrunk",
            "moqui.basic.email.EmailOAuthProvider",
            "moqui.basic.print.NetworkPrinter",
            "moqui.server.AcmeName",
            "moqui.server.AcmeChallenge",
            "moqui.server.AcmeCertificate",
            "moqui.server.AcmeOrderLock",
            "moqui.entity.EntityAuditLog",
            "moqui.monitor.RecoveryAction",
            "moqui.monitor.RecoveryAdapter",
            "moqui.monitor.RecoveryPolicy",
            "moqui.monitor.RecoveryControl",
            "moqui.monitor.InstanceDrain",
            "moqui.monitor.IngestKey",
            "moqui.monitor.ScrapeTarget",
            "moqui.monitor.SyntheticCheck",
            // the members' own rows and messages (SEC-CLU-89)
            "moqui.server.ClusterMember",
            "moqui.server.ClusterMessage",
        ] {
            assert!(!is_restricted_generic_entity(name), "{name}");
            assert!(
                is_service_written_entity(name) && is_generic_write_refused(name),
                "{name}"
            );
        }
        assert!(is_generic_write_refused("moqui.security.UserLoginKey"));
        assert!(!is_generic_write_refused("moqui.monitor.MonitorIncident"));
        assert!(!is_generic_write_refused("moqui.basic.Enumeration"));
        // the ETL mappings and runs: an expression planted in one runs as its user
        for name in [
            "moqui.etl.EtlMapping",
            "moqui.etl.EtlMappingField",
            "moqui.etl.EtlValueMap",
            "moqui.etl.EtlValueMapEntry",
            "moqui.etl.EtlRun",
            "moqui.etl.EtlRunError",
        ] {
            assert!(is_audit_entity(name) && is_restricted_generic_entity(name), "{name}");
        }
        // the hosted-instance configuration: a command, a script or a service name runs from it (SEC-TLS-02)
        for name in [
            "moqui.server.instance.InstanceImage",
            "moqui.server.instance.InstanceHostType",
            "moqui.server.instance.InstanceKindSecret",
        ] {
            assert!(
                is_host_config_entity(name) && is_restricted_generic_entity(name),
                "{name}"
            );
        }
        // an instance's environment and secret bindings: read by the tools, written only by the instance services
        for name in [
            "moqui.server.instance.InstanceEnvironment",
            "moqui.server.instance.AppInstanceSecret",
        ] {
            assert!(
                !is_restricted_generic_entity(name) && is_generic_write_refused(name),
                "{name}"
            );
        }
        assert!(is_user_account_entity("moqui.security.UserAccount"));
        assert!(is_user_account_entity("users"));
        assert!(!is_user_account_entity("moqui.security.UserAccountX"));
    }

    #[test]
    fn secrets_are_left_out_of_nested_maps() {
        let nested = Value::Map(
            [
                ("userId".to_string(), Value::string("U1")),
                ("currentPassword".to_string(), Value::string("hash")),
                ("passwordSalt".to_string(), Value::string("salt")),
            ]
            .into_iter()
            .collect(),
        );
        let out = strip_user_account_secrets(Value::Map(
            [
                ("users".to_string(), Value::List(vec![nested])),
                ("passwordHashType".to_string(), Value::string("SHA")),
            ]
            .into_iter()
            .collect(),
        ));
        let Value::Map(out) = out else { panic!("a map") };
        assert!(!out.contains_key("passwordHashType"));
        let Some(Value::List(users)) = out.get("users") else {
            panic!("users")
        };
        let Value::Map(user) = &users[0] else { panic!("a user") };
        assert!(user.contains_key("userId"));
        assert!(!user.contains_key("currentPassword") && !user.contains_key("passwordSalt"));
    }

    /// Nested hash fields are dropped, `emailAddress` stays, and the map passed in still has its secrets.
    #[test]
    fn nested_hash_fields_are_dropped_email_stays_and_the_input_is_unchanged() {
        let nested = Value::Map(
            [
                ("currentPassword".to_string(), Value::string("hash")),
                ("username".to_string(), Value::string("sec.none.only")),
                (
                    "child".to_string(),
                    Value::Map(
                        [
                            ("resetPassword".to_string(), Value::string("r")),
                            ("passwordSalt".to_string(), Value::string("s")),
                            ("emailAddress".to_string(), Value::string("a@b.c")),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let input = Value::Map(
            [
                ("users".to_string(), Value::List(vec![nested])),
                ("passwordHashType".to_string(), Value::string("SHA")),
            ]
            .into_iter()
            .collect(),
        );
        let out = strip_user_account_secrets(input.clone());
        let Value::Map(input) = input else { panic!("a map") };
        let Value::List(users) = input.get("users").expect("users") else {
            panic!("users")
        };
        let Value::Map(user) = &users[0] else { panic!("a user") };
        assert_eq!(user.get("currentPassword").and_then(Value::as_str), Some("hash"));
        assert!(input.contains_key("passwordHashType"));

        let Value::Map(out) = out else { panic!("a map") };
        assert!(!out.contains_key("passwordHashType"));
        let Value::List(users) = out.get("users").expect("users") else {
            panic!("users")
        };
        let Value::Map(user) = &users[0] else { panic!("a user") };
        assert_eq!(user.get("username").and_then(Value::as_str), Some("sec.none.only"));
        assert!(!user.contains_key("currentPassword"));
        let Value::Map(child) = user.get("child").expect("child") else {
            panic!("child")
        };
        assert_eq!(child.get("emailAddress").and_then(Value::as_str), Some("a@b.c"));
        assert!(!child.contains_key("resetPassword"));
        assert!(!child.contains_key("passwordSalt"));
    }
}
