//! The semantic guards of a business dataset's query (BOSS impl-insights P6, INS-03; design-insights §5 Analysis, §6.3;
//! docs/tools/Monitoring.md "Semantic guards"): what a dataset's `fieldDefaults` declare about a measure decides which
//! aggregations of it mean something, and one that does not is refused with its reason rather than shown without a unit.
//!
//! - `unitField` (an amount's currency, a quantity's unit): a sum, average, least or greatest of the field over documents of
//!   several units is refused (`UNIT_MIXED`) until the query groups by the unit field or is filtered to one unit; a measure in
//!   one named unit (a conversion the owner computes, with a fixed `unit`) has no `unitField` and is not held to this.
//! - `ratio` (a stored rate, with its `numerator` and `denominator` fields): never summed or averaged (`RATIO_AGGREGATE`);
//!   the rate of a group is its summed numerator over its summed denominator, a calculate step.
//! - `nonAdditive` (the dimensions a balance or snapshot is not added across, usually its date): a sum needs each of them as
//!   a group or pinned to one value (`NONADDITIVE`).
//! - a document query returns a bounded page: a total, count or group computed from those rows is refused
//!   (`BOUNDED_RESULT`) when more records match than were returned, and the result says how many match.
//! - a dataset whose definition says `rows: "restricted"` gives its aggregates to its readers and its records only to a
//!   holder of `org.moqui.report.rows.<id>`.
//!
//! The guards hold for every caller of the engine (a panel, Explore, a direct `run#Query`, an alert, a schedule), since they
//! are in the query's execution. A dataset that is not a business one is read as before.
use super::{business, definitions, fail, search, text, Dataset, Error, ExecutionContext, Json};
use indexmap::IndexMap;
use moqui_core::search::{query::FieldOp, Query};
use std::collections::BTreeSet;

/// The body of a business dataset's definition, or None for any other dataset (telemetry, an index alias, a document by its
/// id, a technical definition), which the guards leave alone.
pub(super) fn business_body(ec: &ExecutionContext, dataset: &Dataset) -> Result<Option<Json>, Error> {
    if dataset.telemetry || dataset.id.starts_with("index:") || search::telemetry::is_telemetry_index(&dataset.id) {
        return Ok(None);
    }
    Ok(definitions::read_optional(ec, &dataset.id)?
        .map(|definition| definition["body"].clone())
        .filter(business::is_business))
}

/// The fields a query pins to one value: an `eq`, or an `in` of one value, that every matching document must satisfy.
fn pinned(options: &Json) -> Result<BTreeSet<String>, Error> {
    fn walk(query: &Query, out: &mut BTreeSet<String>) {
        match query {
            Query::Field(field) if !field.negate => {
                let one = match field.op {
                    FieldOp::Eq => !field.value.is_null(),
                    FieldOp::In => field.values.len() == 1 || (field.values.is_empty() && !field.value.is_null()),
                    _ => false,
                };
                if one {
                    out.insert(field.name.strip_suffix(".keyword").unwrap_or(&field.name).to_owned());
                }
            }
            Query::Bool { all, .. } => {
                for query in all {
                    walk(query, out);
                }
            }
            _ => {}
        }
    }
    let mut out = BTreeSet::new();
    if let Some(query) = options.get("query").filter(|query| !query.is_null()) {
        walk(&Query::from_value(&moqui_lib::json::from_json(query))?, &mut out);
    }
    if let Some(query) = options["queryString"].as_str().filter(|text| !text.is_empty()) {
        walk(&search::parse::parse_query_string(query)?, &mut out);
    }
    for filter in options["filters"].as_array().into_iter().flatten() {
        if filter["disabled"] == true || filter["negated"] == true {
            continue;
        }
        let query = filter.get("query").unwrap_or(filter);
        walk(&Query::from_value(&moqui_lib::json::from_json(query))?, &mut out);
    }
    Ok(out)
}

/// What a metric aggregation does to its field.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Measure {
    Sum,
    Average,
    Extreme,
    Count,
}
fn measure(agg: &search::Agg) -> Option<(&str, Measure)> {
    match agg {
        search::Agg::Sum { field } => Some((field, Measure::Sum)),
        search::Agg::Extreme { field, .. } => Some((field, Measure::Extreme)),
        search::Agg::CountDistinct { field } => Some((field, Measure::Count)),
        search::Agg::Metric { field, kind } => Some((
            field,
            match kind {
                search::agg::Metric::ValueCount => Measure::Count,
                _ => Measure::Average,
            },
        )),
        _ => None,
    }
}

/// The static guards of an aggregate query over a business dataset: a ratio is not summed or averaged, and a snapshot is
/// summed only within each of its non-additive dimensions.
pub(super) fn check_aggregations(
    body: &Json,
    options: &Json,
    aggs: &IndexMap<String, search::Agg>,
) -> Result<(), Error> {
    let defaults = &body["fieldDefaults"];
    if !defaults.is_object() {
        return Ok(());
    }
    let pinned = pinned(options)?;
    fn walk(
        aggs: &IndexMap<String, search::Agg>,
        defaults: &Json,
        pinned: &BTreeSet<String>,
        groups: &mut Vec<String>,
    ) -> Result<(), Error> {
        for (id, agg) in aggs {
            if let Some((field, kind)) = measure(agg) {
                let config = &defaults[field];
                if matches!(kind, Measure::Sum | Measure::Average)
                    && !config["ratio"].is_null()
                    && config["ratio"] != false
                {
                    let parts = match (
                        config["ratio"]["numerator"].as_str(),
                        config["ratio"]["denominator"].as_str(),
                    ) {
                        (Some(numerator), Some(denominator)) => {
                            format!(": divide the sum of {numerator} by the sum of {denominator}")
                        }
                        _ => String::new(),
                    };
                    return Err(fail(format!(
                        "RATIO_AGGREGATE: {id} adds or averages {field}, a ratio, which is not the ratio of the group{parts}"
                    )));
                }
                if kind == Measure::Sum {
                    for dimension in config["nonAdditive"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Json::as_str)
                    {
                        if !groups.iter().any(|group| group == dimension) && !pinned.contains(dimension) {
                            return Err(fail(format!(
                                "NONADDITIVE: {id} sums {field}, a balance that is not added across {dimension}: group by {dimension} or filter to one"
                            )));
                        }
                    }
                }
                continue;
            }
            let group = match agg {
                search::Agg::Terms(terms) => Some(terms.field.clone()),
                search::Agg::DateHistogram(histogram) => Some(histogram.field.clone()),
                search::Agg::Histogram { field, .. } => Some(field.clone()),
                _ => None,
            };
            if let Some(children) = agg.sub_aggs() {
                if let Some(group) = &group {
                    groups.push(group.strip_suffix(".keyword").unwrap_or(group).to_owned());
                }
                let result = walk(children, defaults, pinned, groups);
                if group.is_some() {
                    groups.pop();
                }
                result?;
            }
        }
        Ok(())
    }
    walk(aggs, defaults, &pinned, &mut Vec::new())
}

/// Whether a query's aggregations hold a top-hits one, which returns records inside an aggregate.
pub(super) fn returns_records(aggs: &IndexMap<String, search::Agg>) -> bool {
    aggs.values()
        .any(|agg| matches!(agg, search::Agg::TopHits { .. }) || agg.sub_aggs().is_some_and(returns_records))
}

/// The unit guard, on the result: an amount whose documents carry several units (the engine found them when it looked for
/// the amount's unit) is refused for a business dataset, where it was shown without a unit.
pub(super) fn check_units(aggs: &IndexMap<String, search::Agg>, frame: &super::frame::Frame) -> Result<(), Error> {
    fn counts(aggs: &IndexMap<String, search::Agg>, out: &mut BTreeSet<String>, groups: &mut BTreeSet<String>) {
        for (id, agg) in aggs {
            if matches!(measure(agg), Some((_, Measure::Count))) {
                out.insert(id.clone());
            }
            if let search::Agg::Terms(terms) = agg {
                groups.insert(terms.field.strip_suffix(".keyword").unwrap_or(&terms.field).to_owned());
            }
            if let Some(children) = agg.sub_aggs() {
                counts(children, out, groups);
            }
        }
    }
    let mut counted = BTreeSet::new();
    // the fields the query groups by: an amount grouped by its unit field is one unit a row, whatever the group is named
    let mut groups = BTreeSet::new();
    counts(aggs, &mut counted, &mut groups);
    for field in &frame.fields {
        let Some(units) = field.config.get("unitMixed").and_then(Json::as_array) else {
            continue;
        };
        // a count of amounts is a count, in no unit
        if counted.contains(&field.name) {
            continue;
        }
        let unit_field = field
            .config
            .get("unitField")
            .and_then(Json::as_str)
            .unwrap_or("its unit");
        if groups.contains(unit_field) {
            continue;
        }
        let units = units.iter().filter_map(Json::as_str).collect::<Vec<_>>().join(", ");
        return Err(fail(format!(
            "UNIT_MIXED: {} covers records in several units ({units}): group by {unit_field}, filter to one, or use a measure converted to one unit",
            field.name
        )));
    }
    Ok(())
}

/// A page of records is a sample when more match: it is labelled, and nothing is totalled, counted or grouped from it.
pub(super) fn check_bounded(options: &Json, returned: usize, total: u64, notices: &mut Json) -> Result<(), Error> {
    if total <= returned as u64 {
        return Ok(());
    }
    let derived =
        options["totals"].as_array().is_some_and(|totals| !totals.is_empty())
            || options["transformations"].as_array().into_iter().flatten().any(|step| {
                step["disabled"] != true && matches!(text(step, "type", ""), "reduce" | "groupBy" | "pivot")
            });
    if derived {
        return Err(fail(format!(
            "BOUNDED_RESULT: {returned} of {total} matching records were read, so a total, count or group of these rows is not one of all records: aggregate in the query"
        )));
    }
    let notice = serde_json::json!({"level":"info","code":"bounded_result","returned":returned,"total":total,
        "message":format!("Showing {returned} of {total} matching records")});
    match notices.as_array_mut() {
        Some(list) => list.push(notice),
        None => *notices = serde_json::json!([notice]),
    }
    Ok(())
}

/// The records of a dataset whose definition says `rows: "restricted"` are read only by a holder of the dataset's rows
/// artifact; its aggregates stay its readers'. Nothing is fetched for a viewer without it.
pub(super) fn check_rows(ec: &ExecutionContext, dataset: &Dataset) -> Result<(), Error> {
    if dataset.telemetry || dataset.id.starts_with("index:") || search::telemetry::is_telemetry_index(&dataset.id) {
        return Ok(());
    }
    let restricted = definitions::read_optional(ec, &dataset.id)?
        .is_some_and(|definition| definition["body"]["rows"] == "restricted");
    if !restricted {
        return Ok(());
    }
    moqui_core::user::authorize_own("AT_SERVICE", &format!("org.moqui.report.rows.{}", dataset.id), "view").map_err(
        |_| {
            fail(format!(
                "ROWS_RESTRICTED: dataset {} gives this viewer its totals, not its records",
                dataset.id
            ))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn aggs(value: Json) -> IndexMap<String, search::Agg> {
        search::Agg::map_from_value(&moqui_lib::json::from_json(&value)).unwrap()
    }

    #[test]
    fn a_ratio_is_not_averaged_and_a_balance_is_summed_only_within_its_dimensions() {
        let body = json!({"fieldDefaults":{
            "rate":{"ratio":{"numerator":"won","denominator":"offered"}},
            "onHand":{"nonAdditive":["asOfDate"]},
            "amount":{"unitField":"currencyUomId"}}});
        let average = aggs(json!({"r":{"avg":{"field":"rate"}}}));
        let refused = check_aggregations(&body, &json!({}), &average).unwrap_err();
        assert!(
            refused.message().starts_with("RATIO_AGGREGATE"),
            "{}",
            refused.message()
        );
        assert!(refused.message().contains("sum of won by the sum of offered"));
        // its least and greatest are still values of the ratio
        assert!(check_aggregations(&body, &json!({}), &aggs(json!({"r":{"max":{"field":"rate"}}}))).is_ok());
        let balance = aggs(json!({"q":{"sum":{"field":"onHand"}}}));
        let refused = check_aggregations(&body, &json!({}), &balance).unwrap_err();
        assert!(refused.message().starts_with("NONADDITIVE"), "{}", refused.message());
        // grouped by the dimension, or pinned to one value of it by a filter that is neither disabled nor negated
        let grouped = aggs(json!({"day":{"terms":{"field":"asOfDate"},"aggs":{"q":{"sum":{"field":"onHand"}}}}}));
        assert!(check_aggregations(&body, &json!({}), &grouped).is_ok());
        let pin = json!({"field":{"name":"asOfDate","op":"eq","value":"2026-11-01"}});
        assert!(check_aggregations(&body, &json!({"filters":[{"query":pin}]}), &balance).is_ok());
        for off in ["disabled", "negated"] {
            let filter = json!({"query":pin,off:true});
            assert!(
                check_aggregations(&body, &json!({"filters":[filter]}), &balance).is_err(),
                "{off}"
            );
        }
        // a dataset that declares nothing is not held to anything
        assert!(check_aggregations(&json!({}), &json!({}), &balance).is_ok());
    }

    #[test]
    fn a_page_of_records_is_labelled_and_never_totalled() {
        let mut notices = json!([]);
        check_bounded(&json!({}), 20, 20, &mut notices).unwrap();
        assert_eq!(notices, json!([]));
        check_bounded(&json!({}), 20, 75, &mut notices).unwrap();
        assert_eq!(notices[0]["code"], "bounded_result");
        assert_eq!(notices[0]["total"], 75);
        let reduce = json!({"transformations":[{"type":"reduce","calculations":["count"]}]});
        let refused = check_bounded(&reduce, 20, 75, &mut notices).unwrap_err();
        assert!(refused.message().starts_with("BOUNDED_RESULT"), "{}", refused.message());
        // the whole population read: its totals are totals
        assert!(check_bounded(&reduce, 20, 20, &mut notices).is_ok());
        let off = json!({"transformations":[{"type":"reduce","disabled":true}]});
        assert!(check_bounded(&off, 20, 75, &mut notices).is_ok());
    }

    #[test]
    fn an_amount_of_several_units_is_refused_unless_grouped_by_its_unit_or_only_counted() {
        let frame = |name: &str| -> crate::frame::Frame {
            serde_json::from_value(json!({"fields":[{"name":name,"type":"number","values":[42],
                "config":{"unitField":"currencyUomId","unitMixed":["EUR","USD"]}}]}))
            .unwrap()
        };
        let total = aggs(json!({"total":{"sum":{"field":"amount"}}}));
        let refused = check_units(&total, &frame("total")).unwrap_err();
        assert!(refused.message().starts_with("UNIT_MIXED"), "{}", refused.message());
        assert!(refused.message().contains("currencyUomId"));
        let grouped =
            aggs(json!({"currency":{"terms":{"field":"currencyUomId"},"aggs":{"total":{"sum":{"field":"amount"}}}}}));
        assert!(check_units(&grouped, &frame("total")).is_ok());
        let counted = aggs(json!({"n":{"value_count":{"field":"amount"}}}));
        assert!(check_units(&counted, &frame("n")).is_ok());
    }
}
