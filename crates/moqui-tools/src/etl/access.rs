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

//! What an ETL run and the grid's file source may reach (plans/Tools-Database.md §10, §14.1):
//!
//! - **Files** ([`readable`], [`read_bytes`]): the caller's own uploads under
//!   `dbresource://moqui/database/upload/<userId>/`, the import root `dbresource://moqui/etl/`, and `runtime://import/`;
//!   with `DATABASE_ADMIN` any `dbresource://` location. Never `component://`, `file:`, another `runtime://` place or
//!   another user's uploads. A refusal is one vague error, `Not available`. Bytes sent with the call (`contentBytes`)
//!   need no location.
//! - **`${}` in a location** ([`expand_location`]): plain text substitution of `${date}`, `${yesterday}`, `${now}` and
//!   `${nowMillis}` only.
//! - **Expressions** ([`may_run_scripts`]): the sealed `GROOVY_SHELL_WEB` permission, the "may run scripts" grant, to
//!   save, check or run a mapping that has one, and again when the run evaluates it.
//! - **Entities** ([`refuse_restricted`], [`is_secret_field`]): no identity-admin, secret-config or audit entity as a
//!   source, target or lookup, and no `UserAccount` password field (by name, on any entity) as a target field or a
//!   lookup's match or result field; an entity source's rows leave those fields out.
//! - **The run itself** ([`run_as_user`]): authorization on, as the user, even on a worker or in a ServiceJob, whose
//!   call is made with authorization disabled; the Tools app's authorization for all actions is checked as the user and
//!   pushed as the run's frame, so the run's writes and service calls inherit what they would in the Tools app, and the
//!   security rules (privileged groups, sealed permissions) run.

use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};

use moqui_core::context::{Authz, ExecutionContext, Tarpit};

/// The caller's uploads: `<root>/<userId>/…`.
pub const UPLOAD_ROOT: &str = "dbresource://moqui/database/upload";
/// Where ETL source files are kept, and the runs' error files.
pub const IMPORT_ROOT: &str = "dbresource://moqui/etl/";
/// The import directory of the runtime.
pub const RUNTIME_IMPORT_ROOT: &str = "runtime://import/";

/// The tokens `${}` takes in a location.
pub const LOCATION_TOKENS: [&str; 4] = ["date", "yesterday", "now", "nowMillis"];

fn current_user() -> Option<String> {
    moqui_core::user::current_user_id()
        .ok()
        .flatten()
        .filter(|user| !user.is_empty() && !user.contains('/'))
}

fn is_database_admin() -> bool {
    moqui_core::user::has_permission("DATABASE_ADMIN").unwrap_or(false)
}

/// Whether the current user may read `location`, by the rules above.
#[must_use]
pub fn location_allowed(location: &str) -> bool {
    let location = location.trim();
    if location.is_empty()
        || location.contains('\\')
        || location.contains('%')
        || location.split('/').any(|part| part == ".." || part == ".")
    {
        return false;
    }
    let under = |prefix: &str| location.starts_with(prefix) && location.len() > prefix.len();
    if under(IMPORT_ROOT) || under(RUNTIME_IMPORT_ROOT) {
        return true;
    }
    if under("dbresource://") && is_database_admin() {
        return true;
    }
    current_user().is_some_and(|user| under(&format!("{UPLOAD_ROOT}/{user}/")))
}

/// Refuse a location the current user may not read, with one vague error.
///
/// # Errors
///
/// `Not available` for a location outside the rules.
pub fn readable(location: &str) -> Result<(), Error> {
    if location_allowed(location) {
        Ok(())
    } else {
        tracing::info!(location, "Refused an ETL or grid file location");
        Err(Error::new(crate::NOT_AVAILABLE))
    }
}

/// The bytes at `location`, once [`readable`] allowed it; read as the framework, since the user's own reach to the
/// resource store is what the rules above decide.
///
/// # Errors
///
/// `Not available` for a location outside the rules, or when there is no file there.
pub fn read_bytes(location: &str) -> Result<Vec<u8>, Error> {
    readable(location)?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    super::source::read_location(location)
}

/// The last-modified time of an allowed `location`, as the framework; none when it is not allowed or not there.
#[must_use]
pub fn last_modified(location: &str) -> Option<i64> {
    if !location_allowed(location) {
        return None;
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    ExecutionContext::with_current(|ec| ec.factory().resource().stat(location))
        .and_then(Result::ok)
        .flatten()
        .and_then(|stat| stat.last_modified)
}

/// `location` with `${date}` (`2026-11-05`), `${yesterday}`, `${now}` (the time stamp) and `${nowMillis}` replaced
/// as text, in the user's time zone, so a nightly file can be named for its day. Nothing else is expanded.
///
/// # Errors
///
/// Any other `${…}`, or one not closed.
pub fn expand_location(location: &str) -> Result<String, Error> {
    if !location.contains("${") {
        return Ok(location.to_string());
    }
    let now = jiff::Zoned::now().with_time_zone(moqui_core::l10n::time_zone());
    let date = now.date();
    let yesterday = date.yesterday().unwrap_or(date);
    let value_of = |token: &str| match token {
        "date" => Some(date.to_string()),
        "yesterday" => Some(yesterday.to_string()),
        "now" => Some(to_plain_string(&Value::Timestamp(now.timestamp()))),
        "nowMillis" => Some(now.timestamp().as_millisecond().to_string()),
        _ => None,
    };
    let mut out = String::new();
    let mut rest = location;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let Some(end) = after.find('}') else {
            return Err(Error::new("A ${ in the location is not closed"));
        };
        match value_of(&after[..end]) {
            Some(value) => out.push_str(&value),
            None => {
                return Err(Error::new(format!(
                    "A location takes only ${{date}}, ${{yesterday}}, ${{now}} and ${{nowMillis}}, not ${{{}}}",
                    &after[..end]
                )))
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Whether the current user may run scripts: the sealed `GROOVY_SHELL_WEB` permission, which a mapping's
/// `expression` fields need.
#[must_use]
pub fn may_run_scripts() -> bool {
    moqui_core::user::has_permission("GROOVY_SHELL_WEB").unwrap_or(false)
}

/// The error of a mapping with an expression, for a user without [`may_run_scripts`].
pub const SCRIPTS_REFUSED: &str = "An expression in an ETL mapping needs the GROOVY_SHELL_WEB permission";

/// Refuse an identity-admin, secret-config or audit entity (`what` names its use: source, target, lookup).
///
/// # Errors
///
/// An error naming the entity's use.
pub fn refuse_restricted(entity_name: &str, what: &str) -> Result<(), Error> {
    if moqui_core::entity::sensitive::is_restricted_generic_entity(entity_name) {
        return Err(Error::new(format!("{entity_name} is not available as an ETL {what}")));
    }
    Ok(())
}

/// A `UserAccount` password field, by name, on any entity.
#[must_use]
pub fn is_secret_field(name: &str) -> bool {
    moqui_core::entity::sensitive::USER_ACCOUNT_SECRET_FIELDS.contains(&name)
}

/// While alive, the run is the user's: authorization required, and the Tools app's frame (all actions) on the stack.
pub struct UserRun {
    _required: Option<moqui_core::context::AuthzRequired>,
    pushed: bool,
}

impl Drop for UserRun {
    fn drop(&mut self) {
        if self.pushed {
            let popped = ExecutionContext::with_current(ExecutionContext::pop);
            if let Some(Err(err)) = popped {
                tracing::warn!(error = %err, "Could not pop the ETL run's Tools frame");
            }
        }
    }
}

/// Make the rest of the call the current user's own, with authorization on (see the module): the Tools app's
/// authorization for all actions checked as the user and pushed. With no context entered (a unit test) nothing is
/// checked.
///
/// # Errors
///
/// The refusal of a user the Tools app does not authorize for all actions.
pub fn run_as_user() -> Result<UserRun, Error> {
    if !ExecutionContext::is_entered() {
        return Ok(UserRun {
            _required: None,
            pushed: false,
        });
    }
    let required = ExecutionContext::with_current(ExecutionContext::require_authz);
    let pushed = ExecutionContext::with_current(|ec| {
        ec.push_checked(
            "AT_XML_SCREEN",
            crate::service::TOOLS_SCREEN,
            "all",
            Authz::Required,
            Tarpit::Skip,
            None,
        )
    })
    .unwrap_or(Ok(()));
    match pushed {
        Ok(()) => Ok(UserRun {
            _required: required,
            pushed: true,
        }),
        Err(err) => {
            tracing::info!(error = %err, "Refused an ETL run");
            Err(Error::new(
                "An ETL run needs authorization for all actions on the Tools app",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_location_expands_only_its_date_tokens() {
        let expanded = expand_location("dbresource://moqui/etl/in-${date}.csv").unwrap();
        assert!(expanded.starts_with("dbresource://moqui/etl/in-20"), "{expanded}");
        assert!(!expanded.contains("${"));
        assert!(expand_location("x/${nowMillis}").unwrap()[2..].parse::<i64>().is_ok());
        assert_eq!(expand_location("plain").unwrap(), "plain");
        for refused in ["x/${ec.user.userId}", "x/${date", "x/${1+1}"] {
            assert!(expand_location(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn only_the_import_places_are_readable_without_a_user() {
        for allowed in ["dbresource://moqui/etl/in.csv", "runtime://import/a/b.xlsx"] {
            assert!(location_allowed(allowed), "{allowed}");
        }
        for refused in [
            "component://tools/screen/Tools.xml",
            "runtime://conf/MoquiDevConf.xml",
            "file:/etc/passwd",
            "dbresource://moqui/database/upload/other/1/a.csv",
            "dbresource://moqui/etl/../database/upload/other/a.csv",
            "dbresource://moqui/etl/",
            "runtime://import/%2e%2e/conf/x",
        ] {
            assert!(!location_allowed(refused), "{refused}");
        }
    }
}
