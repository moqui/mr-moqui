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

use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::EntityFacade;
use moqui_lib::geo::Format;
use moqui_lib::object::Value;

use super::super::load::{Country, DataSet};
use super::{bound_candidates, remove_in_batches, Removed};

#[test]
fn a_removal_batch_ends_at_the_least_last_id_of_a_full_page() {
    let page = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<Vec<_>>();
    // every page empty: done
    assert_eq!(bound_candidates(&[Vec::new(), Vec::new()], 2), None);
    // the full pages' last IDs, of which the least ends the batch, as more may follow them
    assert_eq!(
        bound_candidates(&[page(&["A", "C"]), page(&["B"]), page(&["A", "B"])], 2),
        Some((page(&["C", "B"]), true))
    );
    // every page short: all their last IDs, of which the greatest ends it
    assert_eq!(
        bound_candidates(&[page(&["A"]), page(&["B", "D"]), Vec::new()], 3),
        Some((page(&["A", "D"]), false))
    );
}

fn create(facade: &EntityFacade, entity: &str, fields: &[(&str, &str)]) {
    let mut row = facade.make_value(entity).unwrap();
    for (name, value) in fields {
        row.set(*name, Value::string(*value));
    }
    row.create(facade).unwrap();
}

fn exists(facade: &EntityFacade, entity: &str, field: &str, id: &str) -> bool {
    facade
        .find(entity)
        .unwrap()
        .use_cache(false)
        .eq(field, Value::string(id))
        .count()
        .unwrap()
        > 0
}

fn ended(facade: &EntityFacade, entity: &str, field: &str, id: &str) -> bool {
    let row = facade
        .find(entity)
        .unwrap()
        .use_cache(false)
        .eq(field, Value::string(id))
        .one()
        .unwrap()
        .unwrap();
    row.get("thruDate").is_some_and(|thru| !matches!(thru, Value::Null))
}

fn code_retired(facade: &EntityFacade, geo_id: &str) -> bool {
    ended(facade, "moqui.basic.GeoPostalCode", "geoId", geo_id)
}

/// A Geo of `source`'s with its own point, associated by `data_set` at `precision`.
fn pointed(facade: &EntityFacade, geo_id: &str, geo_type: &str, source: &str, data_set: &str, precision: &str) {
    let point_id = format!("{geo_id}_{source}");
    create(
        facade,
        "moqui.basic.Geo",
        &[("geoId", geo_id), ("geoTypeEnumId", geo_type)],
    );
    create(
        facade,
        "moqui.basic.GeoPoint",
        &[("geoPointId", &point_id), ("geoPointTypeEnumId", "GPT_POSTAL")],
    );
    create(
        facade,
        "moqui.basic.GeoPointAssoc",
        &[
            ("geoId", geo_id),
            ("geoPointId", &point_id),
            ("dataSetId", data_set),
            ("precisionEnumId", precision),
        ],
    );
}

fn place(facade: &EntityFacade, geo_id: &str, source: &str, name: &str, point_id: &str) {
    create(
        facade,
        "moqui.basic.GeoPostalCodePlace",
        &[
            ("geoId", geo_id),
            ("sourceKey", source),
            ("placeName", name),
            ("geoPointId", point_id),
        ],
    );
}

/// A code of `country` from `source`: its Geo, code row, own point, the point's association (`data_set`'s) and a
/// place name.
fn code(facade: &EntityFacade, country: &str, code: &str, source: &str, data_set: &str, precision: &str) {
    let geo_id = format!("{country}_Z_{code}");
    pointed(facade, &geo_id, "GEOT_POSTAL_CODE", source, data_set, precision);
    create(
        facade,
        "moqui.basic.GeoPostalCode",
        &[
            ("geoId", &geo_id),
            ("countryGeoId", country),
            ("normalizedCode", code),
            ("sourceKey", source),
        ],
    );
    place(facade, &geo_id, source, code, &format!("{geo_id}_{source}"));
}

/// A removal in batches bounded by two rows of each kind, stopped after its first batch, run again while every batch
/// fails, and run a third time: whole batches removed and the rest as they were, a failed batch rolled back and
/// reported while the others go on, then everything of the source and country removed but what a record refers to,
/// end-dated; another source's and another country's rows untouched; and a fourth run changes nothing.
#[test]
fn a_removal_stopped_part_way_or_failing_is_finished_by_another() {
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    );
    let facade = moqui_core::testing::open();
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    // a county of the source's own with a point and no code: only point associations bound its batch
    pointed(
        &facade,
        "SPX_C_001",
        "GEOT_COUNTY",
        "SPECSRC",
        "SPEC_SET",
        "GPP_POSTAL_CENTROID",
    );
    for name in ["A", "B", "C", "D", "E", "G"] {
        let precision = if name == "C" {
            "GPP_USER_CONFIRMED"
        } else {
            "GPP_POSTAL_CENTROID"
        };
        code(&facade, "SPX", name, "SPECSRC", "SPEC_SET", precision);
    }
    // B has more place names than a batch's bound
    place(&facade, "SPX_Z_B", "SPECSRC", "B2", "SPX_Z_B_SPECSRC");
    place(&facade, "SPX_Z_B", "SPECSRC", "B3", "SPX_Z_B_SPECSRC");
    // D is a group's member: in use
    create(
        &facade,
        "moqui.basic.Geo",
        &[("geoId", "SPX_GROUP"), ("geoTypeEnumId", "GEOT_GROUP")],
    );
    create(
        &facade,
        "moqui.basic.GeoAssoc",
        &[
            ("geoId", "SPX_GROUP"),
            ("toGeoId", "SPX_Z_D"),
            ("geoAssocTypeEnumId", "GAT_GROUP_MEMBER"),
        ],
    );
    // another source's place name of G on G's point: the point, and the code, are in use
    place(&facade, "SPX_Z_G", "OTHERSRC", "G elsewhere", "SPX_Z_G_SPECSRC");
    // another source's code of the country, and the source's code of another country
    code(&facade, "SPX", "F", "OTHERSRC", "OTHER_SET", "GPP_POSTAL_CENTROID");
    code(&facade, "SPY", "A", "SPECSRC", "SPEC_SET", "GPP_POSTAL_CENTROID");
    let set = DataSet {
        data_set_id: "SPEC_SET".to_string(),
        source_key: "SPECSRC".to_string(),
        data_source_id: None,
        priority: 0,
        format: Format::GeoNamesPostal,
        owned_set_ids: vec!["SPEC_SET".to_string()],
    };
    let country = Country {
        geo_id: "SPX".to_string(),
        alpha2: "SX".to_string(),
        alpha3: "SPX".to_string(),
    };
    let mut go_on = || Ok(());

    // stopped after its first batch, the county and A (the least last ID of the full pages: the associations' SPX_Z_A),
    // each whole (Geo, association, point; A's code and place name too); the rest untouched
    let first = remove_in_batches(&facade, &set, Some(&country), 2, Some(1), &mut go_on).unwrap();
    assert_eq!(
        first,
        Removed {
            deleted: 3 + 5,
            ..Removed::default()
        }
    );
    for id in ["SPX_C_001", "SPX_Z_A"] {
        assert!(!exists(&facade, "moqui.basic.Geo", "geoId", id), "{id}");
        assert!(!exists(&facade, "moqui.basic.GeoPointAssoc", "geoId", id), "{id}");
        assert!(
            !exists(&facade, "moqui.basic.GeoPoint", "geoPointId", &format!("{id}_SPECSRC")),
            "{id}"
        );
    }
    for id in ["SPX_Z_B", "SPX_Z_C", "SPX_Z_D", "SPX_Z_E", "SPX_Z_G"] {
        assert!(exists(&facade, "moqui.basic.GeoPostalCodePlace", "geoId", id), "{id}");
        assert!(!code_retired(&facade, id), "{id}");
    }

    // every batch fails (its links cannot be read): each rolled back and reported, the removal going on to the end
    let failing = {
        let _fail = moqui_core::testing::FailTable::new("geo_assoc");
        remove_in_batches(&facade, &set, Some(&country), 2, None, &mut go_on).unwrap()
    };
    assert_eq!(
        (failing.deleted, failing.retired, failing.stopped.as_deref()),
        (0, 0, None)
    );
    // B (its three place names past the bound), C to D, E to G
    assert_eq!(failing.failed.len(), 3, "{:?}", failing.failed);
    assert!(
        failing.failed[0].starts_with("Geo IDs through SPX_Z_B: "),
        "{:?}",
        failing.failed
    );
    assert_eq!(
        facade
            .find("moqui.basic.GeoPostalCodePlace")
            .unwrap()
            .use_cache(false)
            .eq("geoId", Value::string("SPX_Z_B"))
            .count()
            .unwrap(),
        3
    );

    // run again: it finishes. B whole (three place names); C its place name (its confirmed association, point and Geo
    // stay, the code end-dated); D its place name, association and point (its Geo stays, the code end-dated); E whole;
    // G its place name (its association end-dated, its point and Geo kept, the code end-dated)
    let second = remove_in_batches(&facade, &set, Some(&country), 2, None, &mut go_on).unwrap();
    assert_eq!(
        second,
        Removed {
            deleted: 7 + 1 + 3 + 5 + 1,
            retired: 1 + 1 + 2,
            ..Removed::default()
        }
    );
    for id in ["SPX_Z_B", "SPX_Z_E"] {
        assert!(!exists(&facade, "moqui.basic.Geo", "geoId", id), "{id}");
    }
    for id in ["SPX_Z_C", "SPX_Z_D", "SPX_Z_G"] {
        assert!(exists(&facade, "moqui.basic.Geo", "geoId", id), "{id}");
        assert!(code_retired(&facade, id), "{id}");
    }
    // the source's place names left are the other country's
    let left: Vec<String> = facade
        .find("moqui.basic.GeoPostalCodePlace")
        .unwrap()
        .use_cache(false)
        .eq("sourceKey", Value::string("SPECSRC"))
        .list()
        .unwrap()
        .iter()
        .map(|row| {
            row.get("geoId")
                .map(moqui_lib::object::to_plain_string)
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(left, vec!["SPY_Z_A".to_string()]);
    assert!(exists(&facade, "moqui.basic.GeoPoint", "geoPointId", "SPX_Z_C_SPECSRC"));
    assert!(!ended(&facade, "moqui.basic.GeoPointAssoc", "geoId", "SPX_Z_C"));
    assert!(!exists(
        &facade,
        "moqui.basic.GeoPoint",
        "geoPointId",
        "SPX_Z_D_SPECSRC"
    ));
    assert!(exists(&facade, "moqui.basic.GeoAssoc", "toGeoId", "SPX_Z_D"));
    assert!(exists(&facade, "moqui.basic.GeoPoint", "geoPointId", "SPX_Z_G_SPECSRC"));
    assert!(ended(&facade, "moqui.basic.GeoPointAssoc", "geoId", "SPX_Z_G"));
    // another source's and another country's rows untouched
    for id in ["SPX_Z_F", "SPY_Z_A"] {
        assert!(exists(&facade, "moqui.basic.GeoPostalCodePlace", "geoId", id), "{id}");
        assert!(!ended(&facade, "moqui.basic.GeoPointAssoc", "geoId", id), "{id}");
        assert!(!code_retired(&facade, id), "{id}");
    }

    // nothing is left to do
    let third = remove_in_batches(&facade, &set, Some(&country), 2, None, &mut go_on).unwrap();
    assert_eq!(third, Removed::default());

    // a removal asked to stop before a batch stops there
    code(&facade, "SPX", "H", "SPECSRC", "SPEC_SET", "GPP_POSTAL_CENTROID");
    let mut stop = || Err("an import started".to_string());
    let stopped = remove_in_batches(&facade, &set, Some(&country), 2, None, &mut stop).unwrap();
    assert_eq!(stopped.stopped.as_deref(), Some("an import started"));
    assert!(exists(&facade, "moqui.basic.Geo", "geoId", "SPX_Z_H"));
}
