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

//! A view read in a sim session, as Java's `EntityFindImpl.copyWorkingSet` and `TransactionCacheDb.copyViewMembers`.
//!
//! The sim's rows are in a schema on the overlay cluster, which production cannot join, so a view whose members the
//! sim changed is read there: the member rows it needs are copied in as replicas, then the view's own SQL runs on the
//! sim schema. The rows copied are those of production's answer to the same find, and the rows that join with the
//! sim's own rows. A plain read here does not copy what it reads, as Java's does, so without that walk a row the sim
//! created would find no production row to join. An aggregate view copies its members whole, as Java's
//! `answerAggregateViewFromProduction`.

use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};

use moqui_db::model::{AliasExpr, EntityDefinition, EntityModel};
use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};

use super::value::FieldMap;
use super::cache;
use super::facade::EntityFacade;
use super::value::EntityValue;

/// Java's `TransactionCacheDb.COPY_CAP`: rows one find copies from production.
pub(super) const COPY_CAP: usize = 10_000;
/// Key tuples per production find of a member's rows.
const MATCH_BATCH: usize = 100;

thread_local! {
    /// Above zero while a working set is built, so its own reads go to production, as Java's `beginBypass`.
    static BYPASS: Cell<u32> = const { Cell::new(0) };
}

/// Keeps this thread's view reads on production while it lives.
pub(super) struct Bypass(());

impl Bypass {
    pub(super) fn enter() -> Self {
        BYPASS.with(|depth| depth.set(depth.get() + 1));
        Self(())
    }
}

impl Drop for Bypass {
    fn drop(&mut self) {
        BYPASS.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// A member entity of the view that is a table.
struct Member {
    alias: String,
    entity_name: String,
    pk_fields: Vec<String>,
    join_from: Option<String>,
    /// Join-from field to this member's field.
    keys: Vec<(String, String)>,
    /// This member's field to the view alias that selects it.
    selected: Vec<(String, String)>,
}

/// The members of a view a sim read copies, and whether it aggregates.
pub(super) struct ViewPlan {
    members: Vec<Member>,
    aggregate: bool,
}

/// What a read of `entity_name` needs from the sim schema: `None` when production answers it, which is when no sim
/// session is open, a working set is being built, it is not a view, or the sim changed none of its members.
pub(super) fn plan_for(facade: &EntityFacade, entity_name: &str) -> Result<Option<ViewPlan>, Error> {
    if BYPASS.with(Cell::get) > 0 || !cache::sim_entered() {
        return Ok(None);
    }
    let plan = facade.with_entity(entity_name, |entity, _, model| {
        if !entity.is_view {
            return Ok(None);
        }
        Ok(Some(view_plan(entity, model)))
    })?;
    Ok(plan.filter(|plan| {
        plan.members
            .iter()
            .any(|member| cache::sim_changed(&member.entity_name))
    }))
}

fn view_plan(view: &EntityDefinition, model: &EntityModel) -> ViewPlan {
    let mut members = Vec::new();
    for member in &view.member_entities {
        let entity = match model.entity(&member.entity_name) {
            Ok(entity) => entity,
            Err(err) => {
                tracing::warn!(error = %err, "Sim read of {} cannot copy member {}", view.full_name, member.entity_alias);
                continue;
            }
        };
        if entity.is_view {
            // As Java: a member that is a view is not copied, so its rows are not in a sim read of this view.
            tracing::warn!(
                "Sim read of {} does not copy member {}, a view",
                view.full_name,
                member.entity_alias
            );
            continue;
        }
        let pk_fields: Vec<String> = entity.pk_field_names().iter().map(|name| (*name).to_string()).collect();
        // As the view SQL joins: a `related` that is not a member field is the member's single key.
        let keys = member
            .key_map
            .iter()
            .map(|key| {
                let mut related = key
                    .related
                    .clone()
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| key.field_name.clone());
                if !entity.has_field(&related) && pk_fields.len() == 1 && member.key_map.len() == 1 {
                    related.clone_from(&pk_fields[0]);
                }
                (key.field_name.clone(), related)
            })
            .collect();
        let selected = view
            .aliases
            .iter()
            .filter(|alias| {
                alias.entity_alias.as_deref() == Some(member.entity_alias.as_str())
                    && alias.function.is_none()
                    && matches!(alias.expr, AliasExpr::Column)
            })
            .map(|alias| (alias.field_name().to_string(), alias.name.clone()))
            .collect();
        members.push(Member {
            alias: member.entity_alias.clone(),
            entity_name: entity.full_name.clone(),
            pk_fields,
            join_from: member.join_from_alias.clone(),
            keys,
            selected,
        });
    }
    let aggregate = view
        .aliases
        .iter()
        .any(|alias| alias.function.is_some() || alias.aggregates());
    ViewPlan { members, aggregate }
}

impl ViewPlan {
    /// Copy what the read needs into the sim schema and return the schema. `view_rows` is production's answer to the
    /// find, read with every alias; an aggregate view does not ask for it, since its rows are sums, not member rows.
    pub(super) fn prepare(
        &self,
        facade: &EntityFacade,
        view_rows: impl FnOnce() -> Result<Vec<EntityValue>, Error>,
    ) -> Result<String, Error> {
        let _bypass = Bypass::enter();
        let schema = cache::sim_schema().ok_or_else(|| Error::new("sim session is missing"))?;
        for member in &self.members {
            cache::ensure_sim_table(facade, &member.entity_name)?;
        }
        let mut copied = 0_usize;
        if self.aggregate {
            for member in &self.members {
                self.copy_whole(facade, member, &mut copied)?;
            }
            return Ok(schema);
        }
        let rows = view_rows()?;
        for member in &self.members {
            self.copy_view_members(facade, member, &rows, &mut copied)?;
        }
        for member in &self.members {
            if !cache::sim_changed(&member.entity_name) {
                continue;
            }
            let own = cache::sim_written_rows(facade, &member.entity_name)?;
            let mut visited = HashSet::from([member.alias.clone()]);
            self.walk(facade, member, own, &mut visited, &mut copied)?;
        }
        Ok(schema)
    }

    /// Copy the member rows of production's view rows, by key when the view selects the member's whole key, else by
    /// the fields it selects, as Java's `copyViewMembers`.
    fn copy_view_members(
        &self,
        facade: &EntityFacade,
        member: &Member,
        rows: &[EntityValue],
        copied: &mut usize,
    ) -> Result<(), Error> {
        // Rows grouped by the member fields they hold, so each group is one batched find.
        let mut groups: BTreeMap<Vec<String>, Vec<Vec<Value>>> = BTreeMap::new();
        for row in rows {
            let held: Vec<(String, Value)> = member
                .selected
                .iter()
                .filter_map(|(field, alias)| {
                    row.get(alias)
                        .filter(|value| !value.is_null())
                        .map(|value| (field.clone(), value.clone()))
                })
                .collect();
            let full_key = !member.pk_fields.is_empty()
                && member
                    .pk_fields
                    .iter()
                    .all(|pk| held.iter().any(|(field, _)| field == pk));
            let (fields, values): (Vec<String>, Vec<Value>) = if full_key {
                member
                    .pk_fields
                    .iter()
                    .filter_map(|pk| held.iter().find(|(field, _)| field == pk).cloned())
                    .unzip()
            } else {
                held.into_iter().unzip()
            };
            if fields.is_empty() {
                continue;
            }
            groups.entry(fields).or_default().push(values);
        }
        for (fields, tuples) in groups {
            let found = matching_rows(facade, &member.entity_name, &fields, tuples)?;
            copy_rows(facade, &member.entity_name, &found, copied)?;
        }
        Ok(())
    }

    /// From rows of `from`, copy the production rows of each member they join with, and on outward through the join
    /// tree.
    fn walk(
        &self,
        facade: &EntityFacade,
        from: &Member,
        rows: Vec<FieldMap>,
        visited: &mut HashSet<String>,
        copied: &mut usize,
    ) -> Result<(), Error> {
        if rows.is_empty() {
            return Ok(());
        }
        for other in &self.members {
            if visited.contains(&other.alias) {
                continue;
            }
            // Pairs of a field of `from` and the field of `other` it joins.
            let pairs: Vec<(String, String)> = if other.join_from.as_deref() == Some(from.alias.as_str()) {
                other.keys.clone()
            } else if from.join_from.as_deref() == Some(other.alias.as_str()) {
                from.keys
                    .iter()
                    .map(|(theirs, mine)| (mine.clone(), theirs.clone()))
                    .collect()
            } else {
                continue;
            };
            if pairs.is_empty() {
                continue;
            }
            visited.insert(other.alias.clone());
            let mut seen = HashSet::new();
            let tuples: Vec<Vec<Value>> = rows
                .iter()
                .filter_map(|row| {
                    pairs
                        .iter()
                        .map(|(mine, _)| row.get(mine.as_str()).filter(|value| !value.is_null()).cloned())
                        .collect::<Option<Vec<Value>>>()
                })
                .filter(|tuple| seen.insert(tuple.iter().map(to_plain_string).collect::<Vec<_>>()))
                .collect();
            let fields: Vec<String> = pairs.into_iter().map(|(_, theirs)| theirs).collect();
            let found = matching_rows(facade, &other.entity_name, &fields, tuples)?;
            copy_rows(facade, &other.entity_name, &found, copied)?;
            let next = found.into_iter().map(|value| value.fields().clone()).collect();
            self.walk(facade, other, next, visited, copied)?;
        }
        Ok(())
    }

    /// Copy every row of a member, for an aggregate view: its sums count production's rows and the sim's together.
    fn copy_whole(&self, facade: &EntityFacade, member: &Member, copied: &mut usize) -> Result<(), Error> {
        let rows = facade
            .find(&member.entity_name)?
            .disable_authz()
            .use_cache(false)
            .limit(i64::try_from(COPY_CAP + 1).unwrap_or(i64::MAX))
            .list()?;
        if rows.len() > COPY_CAP {
            return Err(Error::new(format!(
                "This aggregate view includes {}, which is too large to copy into the sim after the sim changed data. \
                 Use a screen transition or reporting service that returns the data, or query the member entities \
                 directly.",
                member.entity_name
            )));
        }
        copy_rows(facade, &member.entity_name, rows.values(), copied)
    }
}

/// The rows of `entity_name` whose `fields` hold one of `tuples`, in batches. In a sim these finds see the sim's own
/// rows too; [`cache::copy_replica`] leaves those as they are.
fn matching_rows(
    facade: &EntityFacade,
    entity_name: &str,
    fields: &[String],
    tuples: Vec<Vec<Value>>,
) -> Result<Vec<EntityValue>, Error> {
    let mut out = Vec::new();
    for batch in tuples.chunks(MATCH_BATCH) {
        let condition = if fields.len() == 1 {
            Condition::is_in(fields[0].as_str(), batch.iter().map(|tuple| tuple[0].clone()).collect())
        } else {
            Condition::or(
                batch
                    .iter()
                    .map(|tuple| {
                        Condition::and(
                            fields
                                .iter()
                                .zip(tuple)
                                .map(|(field, value)| Condition::equals(field.as_str(), value.clone()))
                                .collect(),
                        )
                    })
                    .collect(),
            )
        };
        let rows = facade
            .find(entity_name)?
            .disable_authz()
            .use_cache(false)
            .condition(condition)
            .limit(i64::try_from(COPY_CAP + 1).unwrap_or(i64::MAX))
            .list()?;
        if rows.len() > COPY_CAP {
            return Err(copy_cap_error(entity_name));
        }
        out.extend(rows.into_values());
    }
    Ok(out)
}

fn copy_rows(facade: &EntityFacade, entity_name: &str, rows: &[EntityValue], copied: &mut usize) -> Result<(), Error> {
    for row in rows {
        if cache::copy_replica(facade, entity_name, row.fields().clone())? {
            *copied += 1;
            if *copied > COPY_CAP {
                return Err(copy_cap_error(entity_name));
            }
        }
    }
    Ok(())
}

fn copy_cap_error(entity_name: &str) -> Error {
    Error::new(format!("Sim copy-on-read exceeded {COPY_CAP} rows for {entity_name}"))
}
