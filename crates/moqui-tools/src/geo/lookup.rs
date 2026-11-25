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

//! `org.moqui.maps.GeoDataServices.normalize#PostalCode` (plans/Maps.md §6.3): a typed postal code read by the same
//! country rules the importer applies to its rows (`moqui_lib::geo::postal`), for the postal lookup of
//! `org.moqui.maps.MapServices.get#PostalLocation` (MapLookup.js), so a code is found by the normalized form it was
//! stored under. A pure function of its input: it reads and writes nothing.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::geo::postal::lookup_code;
use moqui_lib::object::Value;

use moqui_core::entity::EntityFacade;

use crate::group_db::{text, Out};

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    use moqui_core::rust_service;
    rust_service!(map, normalize_postal_code)
}

/// `normalize#PostalCode`: `postalCode` as a code of `country` (ISO alpha-2 or alpha-3). Out: `normalizedCode`,
/// `displayCode`, `granularityEnumId`, the declared prefix (`prefixCode`, `prefixDisplay`) of a full Canadian, British
/// or Dutch code, `zipPlus4` (a US ZIP+4 read as its ZIP code); or `reason` alone when the text is not a code of the
/// country, which is an answer, not an error.
pub(crate) fn normalize_postal_code(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = Out::new();
    match lookup_code(&text(parameters, "country"), &text(parameters, "postalCode")) {
        Ok(found) => {
            out.insert("normalizedCode".to_string(), Value::string(&found.code.normalized));
            out.insert("displayCode".to_string(), Value::string(&found.code.display));
            out.insert(
                "granularityEnumId".to_string(),
                Value::string(found.code.granularity.enum_id()),
            );
            if let Some(prefix) = &found.prefix {
                out.insert("prefixCode".to_string(), Value::string(&prefix.normalized));
                out.insert("prefixDisplay".to_string(), Value::string(&prefix.display));
            }
            out.insert("zipPlus4".to_string(), Value::Bool(found.zip_plus4));
        }
        Err(reason) => {
            out.insert("reason".to_string(), Value::string(reason));
        }
    }
    Ok(out)
}
