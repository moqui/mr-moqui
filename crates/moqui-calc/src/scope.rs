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

//! The scope of a reader of a document (plans/IronCalc-DB.md, What a scope changes): a fingerprint of everything that
//! decides which rows their data sheets hold. Two people with the same fingerprint read the same rows, so they share a
//! room, and a scheduled export's owner may send their file to a recipient with the owner's fingerprint.
//!
//! It is made of the access (a profile or the Tools app's own), the reader's time zone (a time stamp is a cell in it),
//! and for each data sheet whether the reader may read it, its source, the columns shown and the condition the entity
//! filters add to its finds, with the values the reader's context gives them (the organizations of `sales`).

use std::fmt::Write as _;

use moqui_core::entity::{Condition, EntityFacade};
use moqui_lib::object::to_plain_string;
use sha2::{Digest, Sha256};

use crate::room::DataSheet;
use crate::service::{sheet_access, Access};

/// A condition as text that does not depend on the order its parts or an IN list's values were made in.
fn canonical(condition: &Condition) -> String {
    let sorted = |conditions: &[Condition]| {
        let mut parts: Vec<String> = conditions.iter().map(canonical).collect();
        parts.sort();
        parts.dedup();
        parts.join(",")
    };
    let values = |values: &[moqui_lib::object::Value]| {
        let mut texts: Vec<String> = values.iter().map(to_plain_string).collect();
        texts.sort();
        texts.dedup();
        texts.join("\u{1f}")
    };
    match condition {
        Condition::And(parts) => format!("and[{}]", sorted(parts)),
        Condition::Or(parts) => format!("or[{}]", sorted(parts)),
        Condition::Not(inner) => format!("not[{}]", canonical(inner)),
        Condition::IgnoreCase(inner) => format!("ic[{}]", canonical(inner)),
        Condition::In(field, list) => format!("in[{field:?}:{}]", values(list)),
        Condition::NotIn(field, list) => format!("notin[{field:?}:{}]", values(list)),
        other => format!("{other:?}"),
    }
}

/// The fingerprint of the caller's scope for a document's data sheets under `access`, as a hex hash. Each sheet adds
/// what [`sheet_access`] decides (or that it is not available) and, with the checks still in force (a profile entered),
/// the condition the entity filters add to a find of its entity.
#[must_use]
pub(crate) fn fingerprint(facade: &EntityFacade, sheets: &[DataSheet], access: &Access) -> String {
    let mut text = match access.profile_id() {
        Some(profile) => format!("profile {profile}"),
        None => "tools".to_string(),
    };
    let zone = moqui_core::l10n::time_zone();
    let _ = write!(text, "\u{1e}zone {}", zone.iana_name().unwrap_or("fixed"));
    for sheet in sheets {
        let source = sheet.spec["source"].to_string();
        match sheet_access(facade, sheet, access) {
            Ok(checked) => {
                let filter = checked
                    .entity
                    .as_deref()
                    .and_then(|entity| moqui_core::user::apply_entity_filters(entity, None))
                    .map(|condition| canonical(&condition))
                    .unwrap_or_default();
                let _ = write!(
                    text,
                    "\u{1e}sheet {} source {source} columns [{}] filter [{filter}]",
                    sheet.name,
                    checked.columns.join(",")
                );
            }
            Err(_) => {
                let _ = write!(text, "\u{1e}sheet {} source {source} unavailable", sheet.name);
            }
        }
    }
    Sha256::digest(text.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

#[cfg(test)]
#[path = "scope_tests.rs"]
mod tests;
