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

//! `EmailClientServices.filter#ContactAddresses`: which senders are a user's contacts, for an automatic reply to
//! contacts only (`send#OutOfOfficeReplies`). The framework has no contacts of its own (they are the app's, such as
//! Mantle's parties), so it asks a hook service, as the shell counts do: the one the conf's `email-client@contact-service`
//! names, else the theme value `STRT_EMAIL_CONTACT_SERVICE` of the user's qapps theme. It is called with `userId` and
//! `emailAddressList` and answers `contactAddressList`, the addresses of those that are a contact the user can see. No
//! hook: `contactAddressList` is null, and no sender is known as a contact (an automatic reply to contacts then goes to
//! internal senders only). It is core's, not moqui-mail's, so it answers with or without mail built in.
//!
//! `ChatPeopleServices.find#ContactPeople` asks a second hook the same way (`chat@contact-service`, else the
//! theme value `STRT_CHAT_CONTACT_SERVICE`): which contacts the user can see have one of a list of addresses (`tel:`,
//! `email:`, `user:` keys), each with its own addresses, so chat can group one person's rooms across networks
//! (plans/Comms-Messaging.md §11.1, phase 22). The answer is normalized here: valid keys only, bounded.

use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value, ValueMap};
use moqui_lib::sync::lock;

use crate::context::ExecutionContext;
use crate::entity::EntityFacade;
use crate::screen::sri::{theme_resource_values, user_theme_id, DEFAULT_QUASAR_THEME, QUASAR_THEME_TYPE};
use crate::service::rust_services::RustServiceFn;

/// The conf attribute that names the hook; it wins over the theme value.
const SETTING: &str = "email-client@contact-service";
/// The theme value that names the hook.
const THEME_VALUE: &str = "STRT_EMAIL_CONTACT_SERVICE";
/// The conf attribute and theme value that name the people hook.
const PEOPLE_SETTING: &str = "chat@contact-service";
const PEOPLE_THEME_VALUE: &str = "STRT_CHAT_CONTACT_SERVICE";
/// At most this many addresses are asked about, contacts answered, and addresses kept of one contact.
const PEOPLE_ASKED_MAX: usize = 300;
const PEOPLE_CONTACTS_MAX: usize = 100;
const PEOPLE_ADDRESSES_MAX: usize = 10;

pub(super) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, filter_contact_addresses)?;
    rust_service!(map, find_contact_people)
}

/// The hooks that are missing or failed, each logged once while the server runs.
#[derive(Default)]
struct Logged(Mutex<HashSet<String>>);

fn first_time(key: &str) -> bool {
    ExecutionContext::with_current(|ec| {
        let logged = ec.factory().extension(|_| Logged::default());
        let first = lock(&logged.0).insert(key.to_string());
        first
    })
    .unwrap_or(true)
}

/// The lower-case addresses of a list, or of a comma separated text, without blanks and repeats.
fn addresses(value: Option<&Value>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |text: String| {
        let address = text.trim().to_lowercase();
        if !address.is_empty() && !out.contains(&address) {
            out.push(address);
        }
    };
    match value {
        Some(Value::List(items)) => {
            for item in items {
                if !is_empty(item) {
                    add(to_plain_string(item));
                }
            }
        }
        Some(other) if !is_empty(other) => {
            for part in to_plain_string(other).split(',') {
                add(part.to_string());
            }
        }
        _ => {}
    }
    out
}

/// The hook service for `user_id`: the conf attribute `setting` names, else the user's qapps theme value.
fn hook_service(user_id: &str, setting: &str, theme_value: &str) -> Option<String> {
    let named = crate::context::conf_env::setting(setting).trim().to_string();
    if !named.is_empty() {
        return Some(named);
    }
    let theme = user_theme_id(Some(user_id), QUASAR_THEME_TYPE, DEFAULT_QUASAR_THEME);
    match theme_resource_values(&theme, theme_value) {
        Ok(values) => values
            .into_iter()
            .map(|value| value.trim().to_string())
            .find(|value| !value.is_empty()),
        Err(err) => {
            tracing::error!("Could not read {theme_value} of theme {theme}: {err}");
            None
        }
    }
}

/// The user a hook is asked for: `userId` when given, else the current user. Another user's only with authorization
/// disabled: otherwise an error is added and Err answered. Ok(None) when there is no user at all.
fn hook_user(parameters: &BTreeMap<String, Value>, service: &str) -> Result<Option<String>, ()> {
    let current = crate::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty());
    let given = crate::service::util::param_text(parameters, "userId");
    let Some(user_id) = Some(given).filter(|id| !id.is_empty()).or_else(|| current.clone()) else {
        return Ok(None);
    };
    let internal = ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(true);
    if current.as_deref() != Some(user_id.as_str()) && !internal {
        ExecutionContext::with_current(|ec| {
            ec.message().add_error(format!(
                "{service} reads another user's contacts only with authorization disabled"
            ))
        });
        return Err(());
    }
    Ok(Some(user_id))
}

/// Call the hook as internal work (it decides for the user it is given), in a transaction of its own, so one that fails
/// leaves the caller's transaction as it was; an error it records is its failure, taken back out of the messages.
fn call_hook(
    name: &str,
    parameters: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, crate::context::Error> {
    let mark = ExecutionContext::with_current(|ec| ec.message().error_mark());
    let options = crate::service::call::CallOptions {
        disable_authz: true,
        require_new_transaction: Some(true),
        ..crate::service::call::CallOptions::default()
    };
    let result = crate::service::call::sync_with(name, parameters, &options);
    let errors: Vec<String> = mark
        .and_then(|mark| ExecutionContext::with_current(|ec| ec.message().take_errors_since(mark)))
        .unwrap_or_default();
    match result {
        Err(err) => Err(err),
        Ok(_) if !errors.is_empty() => Err(crate::context::Error::new(errors.join("; "))),
        Ok(out) => Ok(out),
    }
}

/// `filter#ContactAddresses`: of `emailAddressList`, those that are contacts of `userId` (the current user by default;
/// another user's only with authorization disabled). Out: `contactAddressList` (lower case; null when no hook is
/// configured), `hookService`, and `hookFailed` when the hook failed or answered no list, which is logged once by name
/// (then the list is empty: no address counts as a contact).
fn filter_contact_addresses(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let answer = |list: Option<Vec<String>>, service: Option<&str>, failed: bool| {
        BTreeMap::from([
            (
                "contactAddressList".to_string(),
                list.map_or(Value::Null, |list| {
                    Value::List(list.into_iter().map(Value::String).collect())
                }),
            ),
            ("hookService".to_string(), service.map_or(Value::Null, Value::string)),
            ("hookFailed".to_string(), Value::Bool(failed)),
        ])
    };
    let user_id = match hook_user(parameters, "filter#ContactAddresses") {
        Err(()) => return Ok(BTreeMap::new()),
        Ok(None) => return Ok(answer(None, None, false)),
        Ok(Some(user_id)) => user_id,
    };
    let Some(service) = hook_service(&user_id, SETTING, THEME_VALUE) else {
        if first_time("none") {
            tracing::info!(
                "No {SETTING} setting or {THEME_VALUE} theme value: no sender counts as a contact, so an automatic reply to contacts goes to internal senders only"
            );
        }
        return Ok(answer(None, None, false));
    };
    let wanted = addresses(parameters.get("emailAddressList"));
    if wanted.is_empty() {
        return Ok(answer(Some(Vec::new()), Some(&service), false));
    }
    let call = BTreeMap::from([
        ("userId".to_string(), Value::string(&user_id)),
        (
            "emailAddressList".to_string(),
            Value::List(wanted.iter().map(Value::string).collect()),
        ),
    ]);
    let failed = |problem: String| {
        if first_time(&service) {
            tracing::error!("The contact service {service} {problem}; no sender counts as a contact");
        }
        Ok(answer(Some(Vec::new()), Some(&service), true))
    };
    match call_hook(&service, &call) {
        Ok(out) => match out.get("contactAddressList") {
            Some(list @ Value::List(_)) => {
                let known: HashSet<String> = addresses(Some(list)).into_iter().collect();
                // only the addresses asked about, in their order
                let found = wanted.into_iter().filter(|address| known.contains(address)).collect();
                Ok(answer(Some(found), Some(&service), false))
            }
            _ => failed("answered no contactAddressList".to_string()),
        },
        Err(err) => failed(format!("failed: {err}")),
    }
}

/// A person's address key as chat writes it (plans/Comms-Messaging.md §11.1): `tel:` a number in E.164, `email:` a mail
/// address in lower case, `user:` a user id. None for anything else: `ext:` is chat's own, never a hook's.
fn person_key(text: &str) -> Option<String> {
    let (kind, rest) = text.trim().split_once(':')?;
    let rest = rest.trim();
    match kind.to_ascii_lowercase().as_str() {
        "tel" => {
            let digits = rest.strip_prefix('+')?;
            (digits.len() >= 5 && digits.len() <= 15 && digits.bytes().all(|b| b.is_ascii_digit()))
                .then(|| format!("tel:+{digits}"))
        }
        "email" => {
            let address = rest.to_lowercase();
            let (local, domain) = address.split_once('@')?;
            let plain = !address
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, ',' | ';' | '<' | '>' | '"'));
            (!local.is_empty() && domain.contains('.') && !domain.contains('@') && address.len() <= 254 && plain)
                .then(|| format!("email:{address}"))
        }
        "user" => (!rest.is_empty()
            && rest.len() <= 64
            && rest
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '@')))
        .then(|| format!("user:{rest}")),
        _ => None,
    }
}

/// The valid person keys of a list or a comma separated text, in their order, without repeats, at most `max`.
fn person_keys(value: Option<&Value>, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |text: &str| {
        if let Some(key) = person_key(text) {
            if out.len() < max && !out.contains(&key) {
                out.push(key);
            }
        }
    };
    match value {
        Some(Value::List(items)) => {
            for item in items.iter().filter(|item| !is_empty(item)) {
                add(&to_plain_string(item));
            }
        }
        Some(other) if !is_empty(other) => {
            for part in to_plain_string(other).split(',') {
                add(part);
            }
        }
        _ => {}
    }
    out
}

/// A text field of a hook's contact, trimmed and cut to `max` characters; None when empty.
fn contact_text(contact: &ValueMap, name: &str, max: usize) -> Option<String> {
    let text = contact
        .get(name)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)?;
    let text: String = text.trim().chars().take(max).collect();
    (!text.is_empty()).then_some(text)
}

/// Whether a url is a path on this server ([`moqui_lib::string::local_path`]).
fn local_path(url: &str) -> bool {
    moqui_lib::string::local_path(url)
}

/// The hook's contactList normalized: maps only, each `{contactRef, name, url, addressList}` with valid keys (at most
/// PEOPLE_ADDRESSES_MAX), a contact with none left out, at most PEOPLE_CONTACTS_MAX contacts. A url is kept only when it is
/// a path on this server (one leading /), so a hook cannot hand the page an outside link.
fn contact_people(value: &Value) -> Vec<Value> {
    let Value::List(items) = value else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        if out.len() >= PEOPLE_CONTACTS_MAX {
            break;
        }
        let Value::Map(contact) = item else { continue };
        let keys = person_keys(contact.get("addressList"), PEOPLE_ADDRESSES_MAX);
        if keys.is_empty() {
            continue;
        }
        let url = contact_text(contact, "url", 500).filter(|url| local_path(url));
        let text = |field: Option<String>| field.map_or(Value::Null, Value::String);
        let mut map = ValueMap::default();
        map.insert("contactRef".to_string(), text(contact_text(contact, "contactRef", 100)));
        map.insert("name".to_string(), text(contact_text(contact, "name", 200)));
        map.insert("url".to_string(), text(url));
        map.insert(
            "addressList".to_string(),
            Value::List(keys.into_iter().map(Value::String).collect()),
        );
        out.push(Value::Map(map));
    }
    out
}

/// `find#ContactPeople`: of the contacts `userId` (the current user by default; another user's only with authorization
/// disabled) can see, those with one of `addressList` (`tel:`, `email:`, `user:` keys, at most PEOPLE_ASKED_MAX), each with
/// its own addresses. Out: `contactList` (null when no hook is configured), `hookService`, and `hookFailed` when the hook
/// failed or answered no list, which is logged once by name (then the list is empty).
fn find_contact_people(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let answer = |list: Option<Vec<Value>>, service: Option<&str>, failed: bool| {
        BTreeMap::from([
            ("contactList".to_string(), list.map_or(Value::Null, Value::List)),
            ("hookService".to_string(), service.map_or(Value::Null, Value::string)),
            ("hookFailed".to_string(), Value::Bool(failed)),
        ])
    };
    let user_id = match hook_user(parameters, "find#ContactPeople") {
        Err(()) => return Ok(BTreeMap::new()),
        Ok(None) => return Ok(answer(None, None, false)),
        Ok(Some(user_id)) => user_id,
    };
    let Some(service) = hook_service(&user_id, PEOPLE_SETTING, PEOPLE_THEME_VALUE) else {
        if first_time("people-none") {
            tracing::info!(
                "No {PEOPLE_SETTING} setting or {PEOPLE_THEME_VALUE} theme value: chat groups a person's rooms from the user's own rooms and links only"
            );
        }
        return Ok(answer(None, None, false));
    };
    let wanted = person_keys(parameters.get("addressList"), PEOPLE_ASKED_MAX);
    if wanted.is_empty() {
        return Ok(answer(Some(Vec::new()), Some(&service), false));
    }
    let call = BTreeMap::from([
        ("userId".to_string(), Value::string(&user_id)),
        (
            "addressList".to_string(),
            Value::List(wanted.into_iter().map(Value::String).collect()),
        ),
    ]);
    let failed = |problem: String| {
        if first_time(&format!("people:{service}")) {
            tracing::error!("The contact people service {service} {problem}; chat groups people without it");
        }
        Ok(answer(Some(Vec::new()), Some(&service), true))
    };
    match call_hook(&service, &call) {
        Ok(out) => match out.get("contactList") {
            Some(list @ Value::List(_)) => Ok(answer(Some(contact_people(list)), Some(&service), false)),
            _ => failed("answered no contactList".to_string()),
        },
        Err(err) => failed(format!("failed: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{addresses, contact_people, local_path, person_key, person_keys};

    #[test]
    fn a_contact_url_is_a_path_on_this_server_only() {
        assert!(local_path("/qapps/People/Contact?partyId=P1"));
        for url in [
            "//evil.example/",
            "/\\evil.example/",
            "/\t/evil.example",
            "/a b",
            "https://evil.example/",
            "/x\u{0}y",
        ] {
            assert!(!local_path(url), "{url:?}");
        }
    }
    use moqui_lib::object::{Value, ValueMap};

    #[test]
    fn addresses_are_lower_case_without_blanks_or_repeats() {
        assert_eq!(
            addresses(Some(&Value::string(
                "Ada@Example.org, ,bob@example.com,ada@example.org"
            ))),
            vec!["ada@example.org".to_string(), "bob@example.com".to_string()]
        );
        assert_eq!(
            addresses(Some(&Value::List(vec![Value::string(" X@y.z "), Value::Null]))),
            vec!["x@y.z".to_string()]
        );
        assert!(addresses(None).is_empty());
    }

    #[test]
    fn person_keys_are_valid_forms_only() {
        assert_eq!(person_key(" tel:+15551234567 ").as_deref(), Some("tel:+15551234567"));
        assert_eq!(person_key("TEL:+1555"), None);
        assert_eq!(person_key("tel:5551234567"), None);
        assert_eq!(
            person_key("email:Ada@Example.org").as_deref(),
            Some("email:ada@example.org")
        );
        assert_eq!(person_key("email:a@b"), None);
        assert_eq!(person_key("email:a@b@c.org"), None);
        assert_eq!(person_key("email:Ada <a@b.org>"), None);
        assert_eq!(person_key("user:EX_JOHN_DOE").as_deref(), Some("user:EX_JOHN_DOE"));
        assert_eq!(person_key("user:a,b"), None);
        assert_eq!(person_key("ext:XA100"), None);
        assert_eq!(
            person_keys(
                Some(&Value::string("tel:+15551234567, user:u1,tel:+15551234567,junk")),
                10
            ),
            vec!["tel:+15551234567".to_string(), "user:u1".to_string()]
        );
        assert_eq!(person_keys(Some(&Value::string("user:a,user:b,user:c")), 2).len(), 2);
    }

    #[test]
    fn contact_people_keeps_valid_contacts_only() {
        let contact = |name: &str, url: &str, keys: &[&str]| {
            let mut map = ValueMap::default();
            map.insert("contactRef".to_string(), Value::string("P1"));
            map.insert("name".to_string(), Value::string(name));
            map.insert("url".to_string(), Value::string(url));
            map.insert(
                "addressList".to_string(),
                Value::List(keys.iter().map(|key| Value::string(*key)).collect()),
            );
            Value::Map(map)
        };
        let out = contact_people(&Value::List(vec![
            contact(
                " Jane ",
                "/qapps/People/Contact?partyId=P1",
                &["tel:+15551234567", "ext:XA1", "email:J@X.org"],
            ),
            contact("Nobody", "/x", &["junk"]),
            contact("Outside", "https://evil.example/", &["user:u2"]),
            Value::string("not a map"),
        ]));
        assert_eq!(out.len(), 2);
        let Value::Map(first) = &out[0] else { panic!("a map") };
        assert_eq!(first.get("name"), Some(&Value::string("Jane")));
        assert_eq!(
            first.get("addressList"),
            Some(&Value::List(vec![
                Value::string("tel:+15551234567"),
                Value::string("email:j@x.org")
            ]))
        );
        let Value::Map(second) = &out[1] else { panic!("a map") };
        assert_eq!(second.get("url"), Some(&Value::Null));
        assert!(contact_people(&Value::Null).is_empty());
    }
}
