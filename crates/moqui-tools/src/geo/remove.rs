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
//! not of a form the importer computes. The rows are removed a batch of Geo IDs at a time, each batch in its own
//! transaction, so a large country is never held whole and a removal stopped part way is finished by another.

use std::collections::{BTreeSet, HashMap, HashSet};

use moqui_lib::error::Error as LibError;
use moqui_lib::geo::{ids, Format};
use moqui_lib::object::{to_plain_string, Value};

use moqui_core::entity::{Condition, EntityFacade, EntityValue};
use moqui_core::transaction::TransactionAttribute;

use super::load::{
    rows_in, uncached, Country, DataSet, GEO, GEO_ASSOC, GEO_POINT, IN_CHUNK, POINT_ASSOC, POSTAL_CODE, POSTAL_PLACE,
};

/// What a removal came to: rows deleted and rows end-dated, the batches that failed and why it stopped early.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Removed {
    pub deleted: u64,
    pub retired: u64,
    /// Each batch that failed, rolled back while the others went on: its Geo IDs and the error
    pub failed: Vec<String>,
    /// Why the removal stopped before its end (an import of the scope started), when it did
    pub stopped: Option<String>,
}

/// What a removal asks before each batch: `Err` with the reason stops it there (an import of the scope started). It
/// also keeps the removal's mark on its data set fresh.
pub type BeforeBatch<'a> = dyn FnMut() -> Result<(), String> + 'a;

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

/// The importer's own `GAT_REGIONS` links of the ZCTA-to-county set: county (`USA_C_`) to postal code (`USA_Z_`),
/// [`REMOVE_BATCH`] at a time, each batch in its own transaction. A batch that fails stops the removal (the next would
/// read the same links); the batches before it stay removed.
fn remove_zcta_counties(facade: &EntityFacade, before_batch: &mut BeforeBatch<'_>) -> Result<Removed, LibError> {
    let tx = facade.transactions_for(GEO)?;
    let mut removed = Removed::default();
    loop {
        if let Err(reason) = before_batch() {
            removed.stopped = Some(reason);
            break;
        }
        let (read, deleted) = tx.run(TransactionAttribute::ForceNew, None, || {
            let links = uncached(facade, GEO_ASSOC)?
                .eq("geoAssocTypeEnumId", Value::string("GAT_REGIONS"))
                .condition(Condition::Like("geoId".into(), Value::string("USA_C_%")))
                .condition(Condition::Like("toGeoId".into(), Value::string("USA_Z_%")))
                .limit(REMOVE_BATCH as i64)
                .list()?;
            let mut deleted = 0_u64;
            for link in links.iter() {
                // LIKE's _ matches any character: only the importer's own IDs
                if text_of(link, "geoId").starts_with("USA_C_") && text_of(link, "toGeoId").starts_with("USA_Z_") {
                    link.delete(facade)?;
                    deleted += 1;
                }
            }
            Ok::<_, LibError>((links.len(), deleted))
        })?;
        removed.deleted += deleted;
        if read < REMOVE_BATCH || deleted == 0 {
            break;
        }
    }
    Ok(removed)
}

/// The rows of each kind (place names, codes, point associations) whose Geo IDs bound a batch of a removal: about this
/// many Geo IDs a batch.
pub(crate) const REMOVE_BATCH: usize = 2000;

/// What a removal reads, in a scope: the source's rows, the relationships that hold a row in use.
struct Scope<'a> {
    set: &'a DataSet,
    country: Option<&'a Country>,
    prefix: Option<String>,
    point_refs: Vec<(String, String)>,
    geo_refs: Vec<(String, String)>,
    code_refs: Vec<(String, String)>,
}

impl Scope<'_> {
    fn in_scope(&self, geo_id: &str) -> bool {
        self.prefix.as_deref().is_none_or(|prefix| geo_id.starts_with(prefix))
    }

    /// `find` narrowed to the scope's country (by its Geo ID prefix) and to Geo IDs after `after` and up to `through`.
    fn narrowed(
        &self,
        mut find: moqui_core::entity::EntityFind,
        after: Option<&str>,
        through: Option<&str>,
    ) -> moqui_core::entity::EntityFind {
        if let Some(prefix) = &self.prefix {
            find = find.condition(Condition::Like("geoId".into(), Value::string(format!("{prefix}%"))));
        }
        if let Some(after) = after {
            find = find.condition(Condition::Greater("geoId".into(), Value::string(after)));
        }
        if let Some(through) = through {
            find = find.condition(Condition::LessEqual("geoId".into(), Value::string(through)));
        }
        find
    }

    /// The source's place names, codes and point associations, each to be narrowed by `narrowed`
    fn places(&self, facade: &EntityFacade) -> Result<moqui_core::entity::EntityFind, LibError> {
        Ok(uncached(facade, POSTAL_PLACE)?.eq("sourceKey", Value::string(&self.set.source_key)))
    }

    fn codes(&self, facade: &EntityFacade) -> Result<moqui_core::entity::EntityFind, LibError> {
        let mut find = uncached(facade, POSTAL_CODE)?.eq("sourceKey", Value::string(&self.set.source_key));
        if let Some(country) = self.country {
            find = find.eq("countryGeoId", Value::string(&country.geo_id));
        }
        Ok(find)
    }

    /// None when the set owns no data set's point associations
    fn assocs(&self, facade: &EntityFacade) -> Result<Option<moqui_core::entity::EntityFind>, LibError> {
        if self.set.owned_set_ids.is_empty() {
            return Ok(None);
        }
        Ok(Some(uncached(facade, POINT_ASSOC)?.condition(Condition::In(
            "dataSetId".into(),
            self.set.owned_set_ids.iter().map(Value::string).collect(),
        ))))
    }
}

/// What bounds the next batch, from each kind of row's next page of Geo IDs (in the database's order, at most `batch`
/// rows each, so a Geo ID with several rows counts more than once): the last IDs of the full pages, of which the least
/// ends the batch, as the rows past each may be more (true); else, every page short, the last IDs of all, of which the
/// greatest ends it, as nothing is left past them (false). None when every page is empty: the removal is done.
pub(crate) fn bound_candidates(pages: &[Vec<String>], batch: usize) -> Option<(Vec<String>, bool)> {
    if pages.iter().all(Vec::is_empty) {
        return None;
    }
    let full: Vec<String> = pages
        .iter()
        .filter(|page| page.len() >= batch)
        .filter_map(|page| page.last().cloned())
        .collect();
    if full.is_empty() {
        Some((pages.iter().filter_map(|page| page.last().cloned()).collect(), false))
    } else {
        Some((full, true))
    }
}

/// The last Geo ID of the next batch ([`bound_candidates`]), as the database orders them: the least (or greatest) of
/// the candidates' Geo rows, else, with none found, the least (or greatest) as Rust compares them. The bound only sizes
/// the batch: a batch takes every row after the one before up to it, so every row falls in some batch either way.
fn batch_bound(facade: &EntityFacade, pages: &[Vec<String>], batch: usize) -> Option<String> {
    let (ids, least) = bound_candidates(pages, batch)?;
    let ordered = uncached(facade, GEO).and_then(|find| {
        find.condition(Condition::In("geoId".into(), ids.iter().map(Value::string).collect()))
            .select_field("geoId")
            .order_by("geoId", !least)
            .limit(1)
            .list()
    });
    let picked = match ordered {
        Ok(rows) => rows.iter().next().map(|row| text_of(row, "geoId")),
        Err(err) => {
            tracing::warn!(error = %err, "Could not order a Geo data removal's batch bound in the database");
            None
        }
    };
    picked.or_else(|| if least { ids.iter().min() } else { ids.iter().max() }.cloned())
}

/// Remove what `set`'s source key owns, in `country`'s scope (every country when none), a batch of Geo IDs at a time
/// (about [`REMOVE_BATCH`]), each batch in its own transaction: a batch's place names, codes, point associations,
/// points and Geos are removed together, so a removal stopped part way leaves whole batches removed and the rest as it
/// was, and a Remove run again finishes it. A batch that fails is rolled back, logged and counted in `failed`, and the
/// removal goes on with the next; `before_batch` is asked before each one ([`BeforeBatch`]).
///
/// # Errors
///
/// A set whose rows are shared (countries, first- and second-level divisions: found by lookup and linked to, never the
/// importer's alone), nothing removed; or a database error reading the next batch's Geo IDs, the batches before it
/// removed.
pub fn remove(
    facade: &EntityFacade,
    set: &DataSet,
    country: Option<&Country>,
    before_batch: &mut BeforeBatch<'_>,
) -> Result<Removed, LibError> {
    match set.format {
        Format::CensusZctaCounty => return remove_zcta_counties(facade, before_batch),
        Format::GeoNamesCountries | Format::GeoNamesAdmin1 | Format::GeoNamesAdmin2 => {
            return Err(LibError::new(format!(
                "Geo data set {} has nothing of its own to remove: countries and divisions are found by lookup and \
                 shared with other records",
                set.data_set_id
            )));
        }
        _ => {}
    }
    remove_in_batches(facade, set, country, REMOVE_BATCH, None, before_batch)
}

/// [`remove`]'s batches, bounded by `batch` rows of each kind; with `max_batches`, stopping after that many,
/// as a removal stopped part way would.
pub(crate) fn remove_in_batches(
    facade: &EntityFacade,
    set: &DataSet,
    country: Option<&Country>,
    batch: usize,
    max_batches: Option<u64>,
    before_batch: &mut BeforeBatch<'_>,
) -> Result<Removed, LibError> {
    let scope = Scope {
        set,
        country,
        prefix: country.map(|country| format!("{}_", country.geo_id)),
        point_refs: referring(
            facade,
            GEO_POINT,
            &[(POINT_ASSOC, "geoPointId"), (POSTAL_PLACE, "geoPointId")],
        ),
        geo_refs: referring(
            facade,
            GEO,
            &[
                (POSTAL_CODE, "geoId"),
                (POSTAL_CODE, "countryGeoId"),
                (GEO_ASSOC, "geoId"),
                (GEO_ASSOC, "toGeoId"),
            ],
        ),
        // what refers to the code row: another source's place names (keyed by the code), and any other entity
        code_refs: referring(facade, POSTAL_CODE, &[]),
    };
    let tx = facade.transactions_for(GEO)?;
    let mut removed = Removed::default();
    let mut after: Option<String> = None;
    let mut batches = 0_u64;
    loop {
        if max_batches.is_some_and(|max| batches >= max) {
            break;
        }
        let pages = [
            Some(scope.places(facade)?),
            Some(scope.codes(facade)?),
            scope.assocs(facade)?,
        ]
        .into_iter()
        .flatten()
        .map(|find| {
            let rows = scope
                .narrowed(find, after.as_deref(), None)
                .select_field("geoId")
                .order_by("geoId", false)
                .limit(batch as i64)
                .list()?;
            Ok(rows.iter().map(|row| text_of(row, "geoId")).collect::<Vec<String>>())
        })
        .collect::<Result<Vec<_>, LibError>>()?;
        let Some(through) = batch_bound(facade, &pages, batch) else {
            break;
        };
        if let Err(reason) = before_batch() {
            removed.stopped = Some(reason);
            break;
        }
        match tx.run(TransactionAttribute::ForceNew, None, || {
            remove_batch(facade, &scope, after.as_deref(), &through)
        }) {
            Ok(done) => {
                removed.deleted += done.deleted;
                removed.retired += done.retired;
            }
            Err(err) => {
                let range = match &after {
                    Some(after) => format!("after {after} through {through}"),
                    None => format!("through {through}"),
                };
                tracing::error!(data_set = %set.data_set_id, range = %range, error = %err, "Could not remove a batch of Geo data; going on with the next");
                removed.failed.push(format!("Geo IDs {range}: {err}"));
            }
        }
        batches += 1;
        if batches.is_multiple_of(20) {
            tracing::info!(data_set = %set.data_set_id, batches, deleted = removed.deleted, retired = removed.retired, through = %through, "Removing Geo data");
        }
        after = Some(through);
    }
    Ok(removed)
}

/// Remove the source's rows of the Geo IDs after `after` up to `through`, in the transaction the caller began.
fn remove_batch(
    facade: &EntityFacade,
    scope: &Scope<'_>,
    after: Option<&str>,
    through: &str,
) -> Result<Removed, LibError> {
    let source_key = scope.set.source_key.as_str();
    let mut removed = Removed::default();
    let mut points: BTreeSet<String> = BTreeSet::new();
    let mut candidates: BTreeSet<String> = BTreeSet::new();
    // the source's place names
    let places = scope.narrowed(scope.places(facade)?, after, Some(through)).list()?;
    for place in places.iter() {
        let geo_id = text_of(place, "geoId");
        if !scope.in_scope(&geo_id) {
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
    let codes: Vec<EntityValue> = scope
        .narrowed(scope.codes(facade)?, after, Some(through))
        .list()?
        .into_values()
        .into_iter()
        .filter(|code| scope.in_scope(&text_of(code, "geoId")))
        .collect();
    candidates.extend(codes.iter().map(|code| text_of(code, "geoId")));
    // the source's point associations; one a user confirmed stays, with its point, and keeps its Geo
    let mut assocs = Vec::new();
    let mut confirmed: HashSet<String> = HashSet::new();
    let found = match scope.assocs(facade)? {
        Some(find) => scope.narrowed(find, after, Some(through)).list()?.into_values(),
        None => Vec::new(),
    };
    for assoc in found {
        if !scope.in_scope(&text_of(&assoc, "geoId")) {
            continue;
        }
        if text_of(&assoc, "precisionEnumId") == "GPP_USER_CONFIRMED" {
            confirmed.insert(text_of(&assoc, "geoPointId"));
        } else {
            assocs.push(assoc);
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
    let mut held_points = referenced(facade, &scope.point_refs, &point_ids);
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
    // Geos: deleted where nothing refers to them (another source's points and names count), else the code end-dated
    let candidates: Vec<String> = candidates.into_iter().collect();
    let mut held = referenced(facade, &scope.geo_refs, &candidates);
    held.extend(referenced(facade, &scope.code_refs, &candidates));
    // a Geo in a group or other association than the importer's GAT_REGIONS is in use
    let mut links = rows_in(facade, GEO_ASSOC, "geoId", &candidates, &[])?;
    links.extend(rows_in(facade, GEO_ASSOC, "toGeoId", &candidates, &[])?);
    for link in &links {
        if text_of(link, "geoAssocTypeEnumId") != "GAT_REGIONS" {
            held.insert(text_of(link, "geoId"));
            held.insert(text_of(link, "toGeoId"));
        }
    }
    // each candidate's links, by either end
    let mut links_of: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, link) in links.iter().enumerate() {
        links_of.entry(text_of(link, "geoId")).or_default().push(index);
        links_of.entry(text_of(link, "toGeoId")).or_default().push(index);
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
        for &index in links_of.get(geo_id).map(Vec::as_slice).unwrap_or_default() {
            let link = &links[index];
            if deleted_links.insert((text_of(link, "geoId"), text_of(link, "toGeoId"))) {
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
}

#[cfg(test)]
#[path = "remove_tests.rs"]
mod tests;
