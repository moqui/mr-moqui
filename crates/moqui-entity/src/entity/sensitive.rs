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
//! entity resources, the generic tools and the data loader keep away from a request.

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

/// `UserAccount` fields that control authentication and identity. Service REST drops them unless the caller is ADMIN.
pub const USER_ACCOUNT_IDENTITY_FIELDS: &[&str] = &[
    "disabled",
    "username",
    "emailAddress",
    "ipAllowed",
    "externalAuthId",
    "requirePasswordChange",
    "hasLoggedOut",
    "disabledDateTime",
    "successiveFailedLogins",
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
];

/// Secret-config entities: `EmailServer` (host and password) and `SystemMessageRemote` (HMAC secret).
const SECRET_CONFIG_ENTITY_KEYS: &[&str] = &["emailserver", "systemmessageremote"];

/// Entities only their own services read and write, never the generic tools, REST or request loads, an ADMIN's
/// included: `DatabaseAudit`, what ran outside the entity facade (§14.2); `SqlRunToken`, the one-time confirmations of
/// SQL runs with changes (§8.2), which a generic write could forge; and `DatabaseConnection`, an external database's
/// host and password (§4.3), which a generic write could point at another host to have the password sent there; and
/// the ETL mappings, value maps and runs (§10), whose `expression` fields run as the mapping's user, so a generic write
/// could plant a script in another user's mapping or take it over. No entity-auto service writes them either.
const AUDIT_ENTITY_KEYS: &[&str] = &[
    "databaseaudit",
    "sqlruntoken",
    "databaseconnection",
    "etlmapping",
    "etlmappingfield",
    "etlvaluemap",
    "etlvaluemapentry",
    "etlrun",
    "etlrunerror",
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

/// `DatabaseAudit`, `SqlRunToken` and `DatabaseConnection`: read and written only by their own services, never through
/// generic entity reads, writes or loads.
pub fn is_audit_entity(entity_name: &str) -> bool {
    AUDIT_ENTITY_KEYS.contains(&simple_entity_key(entity_name).as_str())
}

/// Entities the generic tools and loads do not take from a request entity name: identity-admin, secret-config and
/// audit. `UserAccount` stays available there.
pub fn is_restricted_generic_entity(entity_name: &str) -> bool {
    is_identity_admin_entity(entity_name) || is_secret_config_entity(entity_name) || is_audit_entity(entity_name)
}

/// How deep [`secret_fields`] and [`is_restricted_entity_or_view`] follow views of views.
const MAX_VIEW_DEPTH: u32 = 8;

fn definition_of(facade: &crate::entity::EntityFacade, name: &str) -> Option<moqui_db::model::EntityDefinition> {
    facade.with_entity(name, |entity, _, _| Ok(entity.clone())).ok()
}

/// Whether raw SQL text (a `complex-alias` expression, a `pq-alias`) names a password field or its column.
fn sql_names_secret(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    USER_ACCOUNT_SECRET_FIELDS.iter().any(|field| {
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

fn view_secret_fields(
    facade: &crate::entity::EntityFacade,
    entity: &moqui_db::model::EntityDefinition,
    depth: u32,
) -> Vec<String> {
    use moqui_db::model::AliasExpr;
    let by_name = |name: &str| USER_ACCOUNT_SECRET_FIELDS.contains(&name);
    if !entity.is_view {
        return entity
            .fields
            .iter()
            .filter(|field| by_name(&field.name))
            .map(|field| field.name.clone())
            .collect();
    }
    if depth > MAX_VIEW_DEPTH {
        // too deep to follow: every alias is treated as secret
        return entity.aliases.iter().map(|alias| alias.name.clone()).collect();
    }
    let member_name = |entity_alias: &str| {
        entity
            .member_entities
            .iter()
            .find(|member| member.entity_alias == entity_alias)
            .map(|member| member.entity_name.clone())
    };
    let mut member_secrets: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut origin_secret = |member: Option<String>, field: &str| {
        let Some(member) = member.filter(|member| !member.is_empty()) else {
            return by_name(field);
        };
        let secrets = member_secrets.entry(member.clone()).or_insert_with(|| {
            definition_of(facade, &member)
                .map(|definition| view_secret_fields(facade, &definition, depth + 1))
                .unwrap_or_default()
        });
        by_name(field) || secrets.iter().any(|secret| secret == field)
    };
    let mut out = Vec::new();
    for alias in &entity.aliases {
        let secret = by_name(&alias.name)
            || match &alias.expr {
                AliasExpr::Column => {
                    let member = alias
                        .member_entity
                        .clone()
                        .or_else(|| alias.entity_alias.as_deref().and_then(member_name));
                    origin_secret(member, alias.field_name())
                }
                AliasExpr::Complex(complex) => {
                    let mut fields = Vec::new();
                    complex.visit_fields(&mut |field| fields.push((field.entity_alias.clone(), field.field.clone())));
                    complex.expression.as_deref().is_some_and(sql_names_secret)
                        || fields
                            .into_iter()
                            .any(|(member, field)| origin_secret(member_name(&member), &field))
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
                    case.expression.as_deref().is_some_and(sql_names_secret)
                        || fields
                            .into_iter()
                            .any(|(member, field)| origin_secret(member_name(&member), &field))
                }
            };
        if secret {
            out.push(alias.name.clone());
        }
    }
    for alias in &entity.pq_aliases {
        if by_name(&alias.name) || sql_names_secret(&alias.expression) {
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
        Some(entity) => view_secret_fields(facade, &entity, 0),
        None => Vec::new(),
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
        is_audit_entity, is_identity_admin_entity, is_restricted_generic_entity, is_secret_config_entity,
        is_user_account_entity, simple_entity_key, strip_user_account_secrets,
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
