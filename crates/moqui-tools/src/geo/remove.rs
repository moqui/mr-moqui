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

//! Removing a source's Geo data (plans/Maps.md §6.4, Settings: Remove): the rows its source key owns in a scope are
//! deleted where nothing refers to them, and end-dated where something does. Its place names and point associations go
//! first; a point, a code or a Geo then goes only when no row of any entity refers to it (found from the entity model:
//! every relationship to `Geo` or `GeoPoint`), another source's points and place names included, so a code another
//! source still has, or an address's postal code, stays. A user-confirmed point is never touched, nor a Geo whose ID is
//! not of a form the importer computes.

use std::collections::{BTreeSet, HashMap, HashSet};

use moqui_lib::error::Error as LibError;
use moqui_lib::geo::{ids, Format};
use moqui_lib::object::{to_plain_string, Value};

use moqui_core::entity::{Condition, EntityFacade, EntityValue};
use moqui_core::transaction::TransactionAttribute;

use super::load::{
    rows_in, Country, DataSet, GEO, GEO_ASSOC, GEO_POINT, IN_CHUNK, POINT_ASSOC, POSTAL_CODE, POSTAL_PLACE,
};

/// What a removal came to: rows deleted and rows end-dated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Removed {
    pub deleted: u64,
    pub retired: u64,
}

fn text_of(row: &EntityValue, field: &str) -> String {
    row.get(field).map(to_plain_string).unwrap_or_default()
}

/// The `(entity, field)` pairs that refer to `target` by a one relationship, from the entity model; views,
/// auto-reverse relationships and `skip` left out.
fn referring(facade: &EntityFacade, target: &str, skip: &[(&str, &str)]) -> Vec<(String, String)> {
    facade.with_model(|model| {
        let mut pairs = Vec::new();
        for entity in model.entities() {
            if entity.is_view {
                continue;
            }
            for relationship in &entity.relationships {
                let to_target = relationship.related_entity_name == target || relationship.related == target;
                // an auto-reverse relationship is the model's mirror of a forward one (Geo's one-to-one back to its
                // GeoPostalCode), not a row that refers
                if !to_target || relationship.is_auto_reverse || !relationship.relationship_type.starts_with("one") {
                    continue;
                }
                let [(field, _)] = relationship.key_pairs.as_slice() else {
                    continue;
                };
                let skipped = skip
                    .iter()
                    .any(|(name, skip_field)| *name == entity.full_name && skip_field == field);
                if !skipped {
                    pairs.push((entity.full_name.clone(), field.clone()));
                }
            }
        }
        pairs.sort();
        pairs.dedup();
        pairs
    })
}

/// Which of `ids` a row of `pairs` refers to. A query that fails (a table not there) is logged and counts every ID as
/// referred to, so nothing is deleted that might be in use.
fn referenced(facade: &EntityFacade, pairs: &[(String, String)], ids: &[String]) -> HashSet<String> {
    let mut found = HashSet::new();
    for (entity, field) in pairs {
        for chunk in ids.chunks(IN_CHUNK) {
            let rows = facade.find(entity).and_then(|find| {
                find.condition(Condition::In(
                    field.as_str().into(),
                    chunk.iter().map(Value::string).collect(),
                ))
                .select_field(field)
                .distinct(true)
                .list()
            });
            match rows {
                Ok(rows) => found.extend(rows.iter().map(|row| text_of(row, field))),
                Err(err) => {
                    tracing::error!(error = %err, entity = %entity, field = %field, "Could not check references to Geo data; keeping the rows");
                    found.extend(chunk.iter().cloned());
                }
            }
        }
    }
    found
}

/// Whether `point_id` is the source's own point of `geo_id`: `{geo}_{key}`, or a place's `{geo}_{key}_{n}`.
fn own_point(geo_id: &str, source_key: &str, point_id: &str) -> bool {
    let Some(rest) = point_id
        .strip_prefix(geo_id)
        .and_then(|rest| rest.strip_prefix('_'))
        .and_then(|rest| rest.strip_prefix(source_key))
    else {
        return false;
    };
    rest.is_empty()
        || rest
            .strip_prefix('_')
            .is_some_and(|sequence| !sequence.is_empty() && sequence.chars().all(|c| c.is_ascii_digit()))
}

/// The importer's own `GAT_REGIONS` links of the ZCTA-to-county set: county (`USA_C_`) to postal code (`USA_Z_`).
fn remove_zcta_counties(facade: &EntityFacade) -> Result<Removed, LibError> {
    let tx = facade.transactions_for(GEO)?;
    tx.run(TransactionAttribute::ForceNew, None, || {
        let links = facade
            .find(GEO_ASSOC)?
            .eq("geoAssocTypeEnumId", Value::string("GAT_REGIONS"))
            .condition(Condition::Like("geoId".into(), Value::string("USA_C_%")))
            .condition(Condition::Like("toGeoId".into(), Value::string("USA_Z_%")))
            .list()?;
        let mut removed = Removed::default();
        for link in links.iter() {
            if text_of(link, "geoId").starts_with("USA_C_") && text_of(link, "toGeoId").starts_with("USA_Z_") {
                link.delete(facade)?;
                removed.deleted += 1;
            }
        }
        Ok(removed)
    })
}

/// Remove what `set`'s source key owns, in `country`'s scope (every country when none), in one transaction.
///
/// # Errors
///
/// A set whose rows are shared (countries, first- and second-level divisions: found by lookup and linked to, never the
/// importer's alone), or a database error: nothing is removed.
pub fn remove(facade: &EntityFacade, set: &DataSet, country: Option<&Country>) -> Result<Removed, LibError> {
    match set.format {
        Format::CensusZctaCounty => return remove_zcta_counties(facade),
        Format::GeoNamesCountries | Format::GeoNamesAdmin1 | Format::GeoNamesAdmin2 => {
            return Err(LibError::new(format!(
                "Geo data set {} has nothing of its own to remove: countries and divisions are found by lookup and \
                 shared with other records",
                set.data_set_id
            )))
        }
        _ => {}
    }
    let prefix = country.map(|country| format!("{}_", country.geo_id));
    let in_scope = |geo_id: &str| prefix.as_deref().is_none_or(|prefix| geo_id.starts_with(prefix));
    let scoped = |find: moqui_core::entity::EntityFind| match &prefix {
        Some(prefix) => find.condition(Condition::Like("geoId".into(), Value::string(format!("{prefix}%")))),
        None => find,
    };
    let source_key = set.source_key.as_str();
    let tx = facade.transactions_for(GEO)?;
    tx.run(TransactionAttribute::ForceNew, None, || {
        let mut removed = Removed::default();
        let mut points: BTreeSet<String> = BTreeSet::new();
        let mut candidates: BTreeSet<String> = BTreeSet::new();
        // the source's place names
        let places = scoped(facade.find(POSTAL_PLACE)?.eq("sourceKey", Value::string(source_key))).list()?;
        for place in places.iter() {
            let geo_id = text_of(place, "geoId");
            if !in_scope(&geo_id) {
                continue;
            }
            let point_id = text_of(place, "geoPointId");
            if own_point(&geo_id, source_key, &point_id) {
                points.insert(point_id);
            }
            place.delete(facade)?;
            removed.deleted += 1;
        }
        // the source's codes: candidates to delete
        let mut codes_find = facade.find(POSTAL_CODE)?.eq("sourceKey", Value::string(source_key));
        if let Some(country) = country {
            codes_find = codes_find.eq("countryGeoId", Value::string(&country.geo_id));
        }
        let codes: Vec<EntityValue> = codes_find
            .list()?
            .into_values()
            .into_iter()
            .filter(|code| in_scope(&text_of(code, "geoId")))
            .collect();
        candidates.extend(codes.iter().map(|code| text_of(code, "geoId")));
        // the source's point associations; one a user confirmed stays, with its point, and keeps its Geo
        let mut assocs = Vec::new();
        let mut confirmed: HashSet<String> = HashSet::new();
        for chunk in set.owned_set_ids.chunks(IN_CHUNK) {
            let find = facade.find(POINT_ASSOC)?.condition(Condition::In(
                "dataSetId".into(),
                chunk.iter().map(Value::string).collect(),
            ));
            for assoc in scoped(find).list()?.into_values() {
                if !in_scope(&text_of(&assoc, "geoId")) {
                    continue;
                }
                if text_of(&assoc, "precisionEnumId") == "GPP_USER_CONFIRMED" {
                    confirmed.insert(text_of(&assoc, "geoPointId"));
                } else {
                    assocs.push(assoc);
                }
            }
        }
        for assoc in &assocs {
            let geo_id = text_of(assoc, "geoId");
            let point_id = text_of(assoc, "geoPointId");
            if own_point(&geo_id, source_key, &point_id) && !confirmed.contains(&point_id) {
                points.insert(point_id);
            }
            // a county, place or city of the source's own: a candidate too; a code is the code owner's
            if ids::is_computed(&geo_id) && !geo_id.contains("_Z_") && !geo_id.contains("_ZP_") {
                candidates.insert(geo_id);
            }
        }
        points.retain(|point_id| !confirmed.contains(point_id));
        // points: deleted with their association where nothing else refers to them, else the association end-dated
        let point_ids: Vec<String> = points.into_iter().collect();
        let point_refs = referring(
            facade,
            GEO_POINT,
            &[(POINT_ASSOC, "geoPointId"), (POSTAL_PLACE, "geoPointId")],
        );
        let mut held_points = referenced(facade, &point_refs, &point_ids);
        // another source's place name on this point (it should have its own; if not, it holds the point)
        held_points.extend(
            rows_in(facade, POSTAL_PLACE, "geoPointId", &point_ids, &[])?
                .iter()
                .map(|row| text_of(row, "geoPointId")),
        );
        let now = Value::Timestamp(jiff::Timestamp::now());
        for assoc in &assocs {
            let point_id = text_of(assoc, "geoPointId");
            if held_points.contains(&point_id) || confirmed.contains(&point_id) {
                if text_of(assoc, "thruDate").is_empty() {
                    let mut assoc = assoc.clone();
                    assoc.set("thruDate", now.clone());
                    assoc.update(facade)?;
                    removed.retired += 1;
                }
            } else {
                assoc.delete(facade)?;
                removed.deleted += 1;
            }
        }
        let deletable: Vec<String> = point_ids
            .iter()
            .filter(|point_id| !held_points.contains(*point_id))
            .cloned()
            .collect();
        for point in rows_in(facade, GEO_POINT, "geoPointId", &deletable, &[])? {
            if text_of(&point, "geoPointTypeEnumId") == "GPT_RECORD_LOCATION" {
                continue;
            }
            point.delete(facade)?;
            removed.deleted += 1;
        }
        // Geos: deleted where nothing refers to them (another source's points and names count), else the code
        // end-dated
        let candidates: Vec<String> = candidates.into_iter().collect();
        let geo_refs = referring(
            facade,
            GEO,
            &[
                (POSTAL_CODE, "geoId"),
                (POSTAL_CODE, "countryGeoId"),
                (GEO_ASSOC, "geoId"),
                (GEO_ASSOC, "toGeoId"),
            ],
        );
        let mut held = referenced(facade, &geo_refs, &candidates);
        // what refers to the code row: another source's place names (keyed by the code), and any other entity
        let code_refs = referring(facade, POSTAL_CODE, &[]);
        held.extend(referenced(facade, &code_refs, &candidates));
        // a Geo in a group or other association than the importer's GAT_REGIONS is in use
        let mut links = rows_in(facade, GEO_ASSOC, "geoId", &candidates, &[])?;
        links.extend(rows_in(facade, GEO_ASSOC, "toGeoId", &candidates, &[])?);
        for link in &links {
            if text_of(link, "geoAssocTypeEnumId") != "GAT_REGIONS" {
                held.insert(text_of(link, "geoId"));
                held.insert(text_of(link, "toGeoId"));
            }
        }
        let codes: HashMap<String, &EntityValue> = codes.iter().map(|code| (text_of(code, "geoId"), code)).collect();
        let geos: HashMap<String, EntityValue> = rows_in(facade, GEO, "geoId", &candidates, &[])?
            .into_iter()
            .map(|geo| (text_of(&geo, "geoId"), geo))
            .collect();
        let mut deleted_links: HashSet<(String, String)> = HashSet::new();
        for geo_id in &candidates {
            let code = codes.get(geo_id).copied();
            if held.contains(geo_id) {
                if let Some(code) = code.filter(|code| text_of(code, "thruDate").is_empty()) {
                    let mut code = code.clone();
                    code.set("thruDate", now.clone());
                    code.update(facade)?;
                    removed.retired += 1;
                }
                continue;
            }
            for link in &links {
                let pair = (text_of(link, "geoId"), text_of(link, "toGeoId"));
                if (pair.0 == *geo_id || pair.1 == *geo_id) && deleted_links.insert(pair) {
                    link.delete(facade)?;
                    removed.deleted += 1;
                }
            }
            if let Some(code) = code {
                code.delete(facade)?;
                removed.deleted += 1;
            }
            if let Some(geo) = geos.get(geo_id) {
                geo.delete(facade)?;
                removed.deleted += 1;
            }
        }
        Ok(removed)
    })
}
