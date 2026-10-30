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

/// The text after the last `.`, in lower case.
fn simple_entity_key(entity_name: &str) -> String {
    let name = entity_name.trim();
    let simple = match name.rfind('.') {
        Some(dot) if dot + 1 < name.len() => &name[dot + 1..],
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

/// Entities the generic tools and loads do not take from a request entity name: identity-admin and secret-config.
/// `UserAccount` stays available there.
pub fn is_restricted_generic_entity(entity_name: &str) -> bool {
    is_identity_admin_entity(entity_name) || is_secret_config_entity(entity_name)
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
        is_identity_admin_entity, is_restricted_generic_entity, is_secret_config_entity, is_user_account_entity,
        strip_user_account_secrets,
    };

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
