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

//! Search form input to find conditions. A port of `EntityFindBase.searchFormMap`.
//!
//! A search form posts one value per field (`description`) and optional modifiers beside it:
//! `description_op` (`equals`, `like`, `contains`, `begins`, `empty`, `in`), `_not`, `_ic` (ignore
//! case), a `_period` with `_poffset` and `_pdate`, or `_from` and `_thru` for a range.

use std::collections::{BTreeMap, BTreeSet};

use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::object::{basic_convert, is_empty, to_plain_string, Value};

use super::find::EntityFind;
use super::params::{entity_columns, view_columns};

const DEFAULT_PAGE_SIZE: i64 = 20;

/// The find with the form's conditions, and the names the form puts back in the script context.
pub struct SearchFormResult {
    pub find: EntityFind,
    /// The default parameters when the input made no condition, and `orderByField` when one was
    /// used. Java puts these on the context stack so the form shows what was applied.
    pub context: BTreeMap<String, Value>,
}

/// A field a search can name: its name, Java type, and whether it is an aggregate of a view.
struct SearchField {
    name: String,
    java_type: String,
    aggregate: bool,
}

impl EntityFind {
    /// Add the conditions, order, and paging the form input asks for.
    ///
    /// `defaults` apply only when the input made no condition. `skip_fields` is a comma list of
    /// fields to leave alone. `default_order_by` is used when the input has no `orderByField`.
    /// With `always_paginate`, or when the input has `pageIndex` or `pageSize`, the find is paged
    /// (`pageSize` defaults to the find's limit, or 20), and `pageNoLimit=true` removes paging.
    ///
    /// # Errors
    ///
    /// Returns an error when the entity is unknown or a period or date in the input is not valid.
    pub fn search_form_map(
        mut self,
        input: &BTreeMap<String, Value>,
        defaults: &BTreeMap<String, Value>,
        skip_fields: &str,
        default_order_by: &str,
        always_paginate: bool,
    ) -> Result<SearchFormResult, Error> {
        // As Java: a find that did not choose is not read from the cache, which would ignore paging.
        if self.use_cache.is_none() {
            self.use_cache = Some(false);
        }
        let skip: BTreeSet<String> = skip_fields
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect();
        let fields = self.search_fields()?;
        let mut context = BTreeMap::new();
        let mut added = false;
        if !input.is_empty() {
            added = self.process_input(input, &skip, &fields)?;
        }
        self.has_search_parameters = added;
        if !added && !defaults.is_empty() {
            self.process_input(defaults, &skip, &fields)?;
            context.extend(defaults.clone());
        }
        let order_by = input
            .get("orderByField")
            .map(to_plain_string)
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| default_order_by.to_string());
        if !order_by.is_empty() {
            context.insert("orderByField".to_string(), Value::string(&order_by));
            self = self.order_by_list(&order_by);
        }
        let given = |name: &str| input.get(name).is_some_and(|value| !is_empty(value));
        // A find that asked for the cache is not paged, as Java's `!shouldCache()`, so the cached list answers it.
        if (always_paginate || given("pageIndex") || given("pageSize")) && !self.should_cache()? {
            let index = whole(input.get("pageIndex")).unwrap_or(0).max(0);
            let size = whole(input.get("pageSize"))
                .or(self.limit)
                .unwrap_or(DEFAULT_PAGE_SIZE)
                .max(0);
            self = self.offset(index.saturating_mul(size)).limit(size);
        }
        // Java reads only `true` or the text "true" here.
        if input
            .get("pageNoLimit")
            .is_some_and(|value| matches!(value, Value::Bool(true)) || to_plain_string(value) == "true")
        {
            self.offset = None;
            self.limit = None;
        }
        Ok(SearchFormResult { find: self, context })
    }

    /// `name`, `-name` (descending), `+name`, with `^` for upper case, comma separated.
    fn order_by_list(mut self, text: &str) -> Self {
        for part in text.split(',') {
            self = self.order_by_text(part);
        }
        self
    }

    fn search_fields(&self) -> Result<Vec<SearchField>, Error> {
        self.facade.with_entity(self.entity_name(), |entity, _, model| {
            if entity.is_view {
                let aggregates: BTreeSet<&str> = entity
                    .aliases
                    .iter()
                    .filter(|alias| {
                        matches!(
                            alias.function.as_deref(),
                            Some("min" | "max" | "sum" | "avg" | "count" | "count-distinct")
                        )
                    })
                    .map(|alias| alias.name.as_str())
                    .collect();
                return Ok(view_columns(entity)
                    .into_iter()
                    .map(|(name, java_type)| SearchField {
                        aggregate: aggregates.contains(name.as_str()),
                        name,
                        java_type,
                    })
                    .collect());
            }
            Ok(entity_columns(entity, model)
                .into_iter()
                .map(|(name, java_type)| SearchField {
                    name,
                    java_type,
                    aggregate: false,
                })
                .collect())
        })
    }

    /// Java's `shouldCache` once `search_form_map` has set `useCache`: `useCache(true)` on an entity not marked
    /// `cache="never"`, with no having condition, paging or lock.
    fn should_cache(&self) -> Result<bool, Error> {
        if self.use_cache != Some(true)
            || !self.having.is_empty()
            || self.limit.is_some()
            || self.offset.is_some()
            || self.for_update
        {
            return Ok(false);
        }
        self.facade
            .with_entity(self.entity_name(), |entity, _, _| Ok(entity.cache != "never"))
    }

    fn add(&mut self, field: &SearchField, condition: Condition) {
        if field.aggregate {
            self.having.push(condition);
        } else {
            super::find::add_condition(&mut self.conditions, condition);
        }
    }

    fn process_input(
        &mut self,
        input: &BTreeMap<String, Value>,
        skip: &BTreeSet<String>,
        fields: &[SearchField],
    ) -> Result<bool, Error> {
        let mut added = false;
        for field in fields {
            let name = field.name.as_str();
            if skip.contains(name) {
                continue;
            }
            let op_key = format!("{name}_op");
            if input.contains_key(name) || input.contains_key(&op_key) {
                if let Some(condition) = text_condition(name, field, input) {
                    self.add(field, condition);
                    added = true;
                }
            } else if input.get(&format!("{name}_period")).is_some_and(truthy) {
                let text = |suffix: &str| {
                    input
                        .get(&format!("{name}_{suffix}"))
                        .map(to_plain_string)
                        .unwrap_or_default()
                };
                let (from, thru) = crate::user::period_range(&text("period"), &text("poffset"), &text("pdate"))
                    .map_err(|err| Error::new(err.message().to_string()))?;
                self.add(field, Condition::greater_equal(name, from));
                self.add(field, Condition::less(name, thru));
                added = true;
            } else {
                added |= self.range_conditions(name, field, input)?;
            }
        }
        Ok(added)
    }

    fn range_conditions(
        &mut self,
        name: &str,
        field: &SearchField,
        input: &BTreeMap<String, Value>,
    ) -> Result<bool, Error> {
        let mut added = false;
        if let Some(from) = bound(input.get(&format!("{name}_from")), field, " 00:00:00.000") {
            self.add(field, Condition::greater_equal(name, from));
            added = true;
        }
        if let Some(thru) = bound(input.get(&format!("{name}_thru")), field, " 23:59:59.999") {
            self.add(field, Condition::less_equal(name, thru));
            added = true;
        }
        Ok(added)
    }
}

/// The condition for `name`, `name_op`, `name_not`, and `name_ic`. `None` when the value is empty.
fn text_condition(name: &str, field: &SearchField, input: &BTreeMap<String, Value>) -> Option<Condition> {
    let value = input.get(name).cloned().unwrap_or(Value::Null);
    let value_empty = is_empty(&value);
    let op = input
        .get(&format!("{name}_op"))
        .map(to_plain_string)
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "equals".to_string());
    let flag = |suffix: &str| {
        input
            .get(&format!("{name}_{suffix}"))
            .map(to_plain_string)
            .is_some_and(|text| text == "Y" || text == "true")
    };
    let (not, ignore_case) = (flag("not"), flag("ic"));
    // Java attaches `_ic` to `equals`, `like`, `contains` and `begins` only.
    let case = |condition: Condition| {
        if ignore_case {
            Condition::ignore_case(condition)
        } else {
            condition
        }
    };
    let text = to_plain_string(&value);
    let condition = match op.as_str() {
        "equals" if !value_empty => {
            let typed = match &value {
                Value::String(raw) => convert(raw, &field.java_type),
                other => other.clone(),
            };
            if not {
                // Java's `orNull`: "not" also finds the rows with no value. Java throws on `_ic` here (ignore case on
                // the OR); this compares the value ignoring case.
                or_null(name, case(Condition::not_equals(name, typed)))
            } else {
                case(Condition::equals(name, typed))
            }
        }
        "like" if !value_empty => case(like(name, Value::string(&text), not)),
        "contains" if !value_empty => case(like(name, Value::string(format!("%{text}%")), not)),
        "begins" if !value_empty => case(like(name, Value::string(format!("{text}%")), not)),
        "empty" => {
            // Null or the empty string; "not" is neither.
            if not {
                return Some(Condition::and(vec![
                    Condition::is_not_null(name),
                    Condition::not_equals(name, Value::string("")),
                ]));
            }
            return Some(Condition::or(vec![
                Condition::is_null(name),
                Condition::equals(name, Value::string("")),
            ]));
        }
        "in" if !value_empty => {
            let items: Vec<Value> = match &value {
                Value::List(items) => items.clone(),
                Value::String(raw) => raw.split(',').map(Value::string).collect(),
                _ => return None,
            };
            if items.is_empty() {
                return None;
            }
            if not {
                or_null(name, Condition::not_in(name, items))
            } else {
                Condition::is_in(name, items)
            }
        }
        _ => return None,
    };
    Some(condition)
}

/// `condition OR name IS NULL`, Java's `makeCondition(field, operator, value, orNull)`.
fn or_null(name: &str, condition: Condition) -> Condition {
    Condition::or(vec![condition, Condition::is_null(name)])
}

fn like(name: &str, pattern: Value, not: bool) -> Condition {
    if not {
        Condition::not_like(name, pattern)
    } else {
        Condition::like(name, pattern)
    }
}

/// Text from a form becomes the field's type. Text that does not convert is kept, and the database
/// reports it, as a lone bad value in a search is the user's to see.
fn convert(text: &str, java_type: &str) -> Value {
    let converted = moqui_lib::object::convert_checked(&Value::string(text), java_type);
    converted.unwrap_or_else(|| Value::string(text))
}

/// A range bound. A date typed into a date-time field covers the whole day: the start of the day
/// for `from`, the last millisecond for `thru`.
fn bound(raw: Option<&Value>, field: &SearchField, day_end: &str) -> Option<Value> {
    let raw = raw?;
    if is_empty(raw) {
        return None;
    }
    let Value::String(text) = raw else {
        return Some(raw.clone());
    };
    let stamp = field.java_type.ends_with("Timestamp");
    if stamp && text.len() < 12 {
        let widened = basic_convert(&Value::string(format!("{text}{day_end}")), "java.sql.Timestamp");
        return Some(widened.value);
    }
    Some(convert(text, &field.java_type))
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        other => {
            let text = to_plain_string(other);
            !text.is_empty() && text != "false"
        }
    }
}

fn whole(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if is_empty(value) {
        return None;
    }
    match basic_convert(value, "Long").value {
        Value::I64(number) => Some(number),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use moqui_db::sql::Condition;
    use moqui_lib::object::Value;

    fn input(pairs: &[(&str, &str)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), Value::string(*v)))
            .collect()
    }

    fn find() -> super::EntityFind {
        let facade = crate::testing::open();
        facade.find("moqui.test.TestEntity").unwrap()
    }

    fn conditions(result: &super::SearchFormResult) -> Vec<Condition> {
        result.find.conditions.clone()
    }

    fn search(pairs: &[(&str, &str)]) -> super::SearchFormResult {
        find()
            .search_form_map(&input(pairs), &BTreeMap::new(), "", "", false)
            .unwrap()
    }

    #[test]
    fn operators_build_the_conditions_java_builds() {
        assert_eq!(
            conditions(&search(&[("testMedium", "alp"), ("testMedium_op", "contains")])),
            [Condition::like("testMedium", Value::string("%alp%"))]
        );
        assert_eq!(
            conditions(&search(&[
                ("testMedium", "alp"),
                ("testMedium_op", "begins"),
                ("testMedium_not", "Y")
            ])),
            [Condition::not_like("testMedium", Value::string("alp%"))]
        );
        assert_eq!(
            conditions(&search(&[("testId", "A,B"), ("testId_op", "in")])),
            [Condition::is_in("testId", vec![Value::string("A"), Value::string("B")])]
        );
        assert_eq!(
            conditions(&search(&[("testId", "A"), ("testId_ic", "true")])),
            [Condition::ignore_case(Condition::equals("testId", Value::string("A")))]
        );
        // "empty" needs no value: null or the empty string, and "not" is neither.
        assert_eq!(
            conditions(&search(&[("testMedium_op", "empty")])),
            [Condition::or(vec![
                Condition::is_null("testMedium"),
                Condition::equals("testMedium", Value::string(""))
            ])]
        );
        // An empty value makes no condition, and so does a field the form did not send.
        let empty = search(&[("testMedium", ""), ("other", "x")]);
        assert!(conditions(&empty).is_empty());
        assert!(!empty.find.has_search_parameters);
    }

    #[test]
    fn ignore_case_is_not_for_in_and_not_also_finds_the_empty_rows() {
        // Java attaches `_ic` to equals, like, contains and begins only.
        assert_eq!(
            conditions(&search(&[
                ("testId", "A,B"),
                ("testId_op", "in"),
                ("testId_ic", "true")
            ])),
            [Condition::is_in("testId", vec![Value::string("A"), Value::string("B")])]
        );
        assert_eq!(
            conditions(&search(&[
                ("testMedium", "alp"),
                ("testMedium_op", "begins"),
                ("testMedium_ic", "Y")
            ])),
            [Condition::ignore_case(Condition::like(
                "testMedium",
                Value::string("alp%")
            ))]
        );
        // Java's `orNull`: "not" on equals and in also finds the rows with no value.
        assert_eq!(
            conditions(&search(&[("testId", "A"), ("testId_not", "Y")])),
            [Condition::or(vec![
                Condition::not_equals("testId", Value::string("A")),
                Condition::is_null("testId")
            ])]
        );
        assert_eq!(
            conditions(&search(&[
                ("testId", "A,B"),
                ("testId_op", "in"),
                ("testId_not", "true")
            ])),
            [Condition::or(vec![
                Condition::not_in("testId", vec![Value::string("A"), Value::string("B")]),
                Condition::is_null("testId")
            ])]
        );
    }

    #[test]
    fn a_form_value_replaces_the_finds_own_and_the_cache_is_off_unless_asked_for() {
        let replaced = find()
            .eq("testId", Value::string("A"))
            .search_form_map(&input(&[("testId", "B")]), &BTreeMap::new(), "", "", false)
            .unwrap();
        assert_eq!(conditions(&replaced), [Condition::equals("testId", Value::string("B"))]);
        // Java's `useCache(false)` when the find did not choose, so a paged form never reads a cached list.
        assert_eq!(replaced.find.use_cache, Some(false));
        // A find that asked for the cache is not paged, so the cached list answers it.
        let cached = find()
            .use_cache(true)
            .search_form_map(&input(&[("pageIndex", "1")]), &BTreeMap::new(), "", "", true)
            .unwrap();
        assert_eq!((cached.find.offset, cached.find.limit), (None, None));
        assert_eq!(cached.find.use_cache, Some(true));
        // Only `true` removes paging, as Java.
        let paged = find()
            .search_form_map(&input(&[("pageNoLimit", "Y")]), &BTreeMap::new(), "", "", true)
            .unwrap();
        assert_eq!((paged.find.offset, paged.find.limit), (Some(0), Some(20)));
    }

    #[test]
    fn a_negative_page_gives_no_negative_offset() {
        let page = search(&[("pageIndex", "-3"), ("pageSize", "10")]);
        assert_eq!((page.find.offset_value(), page.find.limit_value()), (Some(0), Some(10)));
        let page = search(&[("pageIndex", "2"), ("pageSize", "-10")]);
        assert_eq!((page.find.offset_value(), page.find.limit_value()), (Some(0), Some(0)));
        let page = search(&[("pageIndex", "2"), ("pageSize", "10")]);
        assert_eq!(
            (page.find.offset_value(), page.find.limit_value()),
            (Some(20), Some(10))
        );
    }

    #[test]
    fn text_becomes_the_field_type_and_a_date_widens_to_the_whole_day() {
        let typed = search(&[("testNumberInteger", "7")]);
        assert_eq!(
            conditions(&typed),
            [Condition::equals("testNumberInteger", Value::I64(7))]
        );
        let range = search(&[("testDateTime_from", "2026-10-01"), ("testDateTime_thru", "2026-10-02")]);
        let found = conditions(&range);
        assert_eq!(found.len(), 2);
        let Condition::GreaterEqual(_, Value::Timestamp(from)) = &found[0] else {
            panic!("from bound {found:?}");
        };
        let Condition::LessEqual(_, Value::Timestamp(thru)) = &found[1] else {
            panic!("thru bound {found:?}");
        };
        // From the start of 10-01 to the last millisecond of 10-02, both whole days.
        assert_eq!(thru.as_millisecond() - from.as_millisecond(), 2 * 86_400_000 - 1);
    }

    #[test]
    fn a_period_makes_a_half_open_range() {
        let found = conditions(&search(&[
            ("testDateTime_period", "month"),
            ("testDateTime_poffset", "0"),
            ("testDateTime_pdate", "2026-10-14"),
        ]));
        assert!(matches!(&found[0], Condition::GreaterEqual(..)), "{found:?}");
        assert!(matches!(&found[1], Condition::Less(..)), "{found:?}");
    }

    #[test]
    fn defaults_apply_only_without_input_and_order_and_paging_are_read() {
        let defaults = input(&[("testMedium", "alpha")]);
        let none = find()
            .search_form_map(&BTreeMap::new(), &defaults, "", "-testId", false)
            .unwrap();
        assert_eq!(
            conditions(&none),
            [Condition::equals("testMedium", Value::string("alpha"))]
        );
        assert_eq!(none.context.get("testMedium").and_then(Value::as_str), Some("alpha"));
        assert_eq!(
            none.context.get("orderByField").and_then(Value::as_str),
            Some("-testId")
        );
        assert!(!none.find.has_search_parameters, "defaults are not search parameters");
        let given = find()
            .search_form_map(&input(&[("testId", "A")]), &defaults, "", "", false)
            .unwrap();
        assert_eq!(conditions(&given), [Condition::equals("testId", Value::string("A"))]);

        let paged = find()
            .search_form_map(
                &input(&[("pageIndex", "2"), ("pageSize", "10")]),
                &BTreeMap::new(),
                "",
                "",
                false,
            )
            .unwrap();
        assert_eq!((paged.find.offset, paged.find.limit), (Some(20), Some(10)));
        let always = find()
            .search_form_map(&BTreeMap::new(), &BTreeMap::new(), "", "", true)
            .unwrap();
        assert_eq!((always.find.offset, always.find.limit), (Some(0), Some(20)));
        let unlimited = find()
            .search_form_map(&input(&[("pageNoLimit", "true")]), &BTreeMap::new(), "", "", true)
            .unwrap();
        assert_eq!((unlimited.find.offset, unlimited.find.limit), (None, None));
        let skipped = find()
            .search_form_map(&input(&[("testId", "A")]), &BTreeMap::new(), "testId", "", false)
            .unwrap();
        assert!(conditions(&skipped).is_empty());
    }

    #[test]
    fn a_form_with_nothing_to_search_returns_no_rows_when_parameters_are_required() {
        let facade = crate::testing::open();
        let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
        row.set("testId", Value::string("A"));
        row.create(&facade).unwrap();
        let all = facade.find("moqui.test.TestEntity").unwrap();
        let result = all
            .require_search_form_parameters(true)
            .search_form_map(&BTreeMap::new(), &BTreeMap::new(), "", "", false)
            .unwrap();
        assert_eq!(result.find.list().unwrap().len(), 0);
        let searched = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .require_search_form_parameters(true)
            .search_form_map(&input(&[("testId", "A")]), &BTreeMap::new(), "", "", false)
            .unwrap();
        assert_eq!(searched.find.list().unwrap().len(), 1);
    }
}
