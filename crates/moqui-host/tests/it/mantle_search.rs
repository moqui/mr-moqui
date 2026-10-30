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

//! Mantle's search callers on SearchFacade, over Mantle's data and a small sales fixture: the feed indexes every
//! Mantle document type, including the wiki pages; the sales report services and projected inventory aggregate
//! the fixture's orders and invoices; party search, the general search, and the universal search run; and a user
//! limited to their organizations sees nothing owned by another, in results, totals, or facets.
//!
//! The test needs `runtime/component/mr-mantle`, which is not part of this repository, and it skips without it. The load takes about half a minute.

#![cfg(all(unix, feature = "embedded-postgres"))]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::driver::DbDriver;
use moqui_core::transaction::PoolConfig;
use moqui_host::db::embedded::EmbeddedCluster;
use moqui_host::{load_data, LoadFlags};
use moqui_lib::object::{to_plain_string, Value};

/// The fixture: two customers (one wholesale), three orders from the retail org (one cancelled), two invoices, and a
/// payment applied before the first invoice's date; and a user employed by the retail org whose group may run the
/// general search with the user organization filter, which administrators may run without it.
fn fixture(day: &dyn Fn(i64) -> String) -> String {
    let (d8, d9, d15, d30) = (day(8), day(9), day(15), day(3650));
    format!(
        r#"<entity-facade-xml type="demo">
    <mantle.party.Party partyId="SF_CUST_A" partyTypeEnumId="PtyPerson" ownerPartyId="ORG_ZIZI_RETAIL">
        <mantle.party.Person firstName="Sally" lastName="Fixture"/>
        <mantle.party.PartyRole roleTypeId="Customer"/>
    </mantle.party.Party>
    <mantle.party.Party partyId="SF_CUST_B" partyTypeEnumId="PtyOrganization" ownerPartyId="ORG_ZIZI_RETAIL">
        <mantle.party.Organization organizationName="Fixture Wholesale"/>
        <mantle.party.PartyRole roleTypeId="Customer"/>
        <mantle.party.PartyClassificationAppl partyClassificationId="CustWholesale" fromDate="{d30}"/>
    </mantle.party.Party>

    <mantle.order.OrderHeader orderId="SF_ORD_1" entryDate="{d8}" placedDate="{d8}" statusId="OrderCompleted" currencyUomId="USD" productStoreId="POPC_DEFAULT">
        <mantle.order.OrderPart orderPartSeqId="01" vendorPartyId="ORG_ZIZI_RETAIL" customerPartyId="SF_CUST_A" statusId="OrderCompleted" isNewCustomer="Y"/>
        <mantle.order.OrderItem orderItemSeqId="01" orderPartSeqId="01" itemTypeEnumId="ItemProduct" productId="DEMO_1_1" quantity="2" unitAmount="10.00"/>
        <mantle.order.OrderItem orderItemSeqId="02" orderPartSeqId="01" itemTypeEnumId="ItemDiscount" quantity="1" unitAmount="-2.00"/>
    </mantle.order.OrderHeader>
    <mantle.order.OrderHeader orderId="SF_ORD_2" entryDate="{d15}" placedDate="{d15}" statusId="OrderApproved" currencyUomId="USD" productStoreId="POPC_DEFAULT">
        <mantle.order.OrderPart orderPartSeqId="01" vendorPartyId="ORG_ZIZI_RETAIL" customerPartyId="SF_CUST_B" statusId="OrderApproved" isNewCustomer="N"/>
        <mantle.order.OrderItem orderItemSeqId="01" orderPartSeqId="01" itemTypeEnumId="ItemProduct" productId="DEMO_2_1" quantity="3" unitAmount="5.00"/>
    </mantle.order.OrderHeader>
    <mantle.order.OrderHeader orderId="SF_ORD_3" entryDate="{d8}" placedDate="{d8}" statusId="OrderCancelled" currencyUomId="USD" productStoreId="POPC_DEFAULT">
        <mantle.order.OrderPart orderPartSeqId="01" vendorPartyId="ORG_ZIZI_RETAIL" customerPartyId="SF_CUST_A" statusId="OrderCancelled"/>
        <mantle.order.OrderItem orderItemSeqId="01" orderPartSeqId="01" itemTypeEnumId="ItemProduct" productId="DEMO_1_1" quantity="100" unitAmount="10.00"/>
    </mantle.order.OrderHeader>

    <mantle.account.invoice.Invoice invoiceId="SF_INV_1" invoiceTypeEnumId="InvoiceSales" fromPartyId="ORG_ZIZI_RETAIL" toPartyId="SF_CUST_A"
            statusId="InvoicePmtRecvd" invoiceDate="{d8}" currencyUomId="USD" invoiceTotal="20.00" appliedPaymentsTotal="20.00" unpaidTotal="0">
        <mantle.account.invoice.InvoiceItem invoiceItemSeqId="01" itemTypeEnumId="ItemProduct" productId="DEMO_1_1" quantity="2" amount="10.00"/>
    </mantle.account.invoice.Invoice>
    <mantle.account.invoice.Invoice invoiceId="SF_INV_2" invoiceTypeEnumId="InvoiceSales" fromPartyId="ORG_ZIZI_RETAIL" toPartyId="SF_CUST_B"
            statusId="InvoiceFinalized" invoiceDate="{d15}" currencyUomId="USD" invoiceTotal="15.00" appliedPaymentsTotal="0" unpaidTotal="15.00">
        <mantle.account.invoice.InvoiceItem invoiceItemSeqId="01" itemTypeEnumId="ItemProduct" productId="DEMO_2_1" quantity="3" amount="5.00"/>
    </mantle.account.invoice.Invoice>
    <mantle.account.payment.Payment paymentId="SF_PAY_1" paymentTypeEnumId="PtInvoicePayment" fromPartyId="SF_CUST_A" toPartyId="ORG_ZIZI_RETAIL"
            statusId="PmntDelivered" effectiveDate="{d9}" amount="20.00" amountUomId="USD"/>
    <mantle.account.payment.PaymentApplication paymentApplicationId="SF_PA_1" paymentId="SF_PAY_1" invoiceId="SF_INV_1" amountApplied="20.00" appliedDate="{d8}"/>

    <mantle.party.Party partyId="SF_EMP" partyTypeEnumId="PtyPerson">
        <mantle.party.Person firstName="Olga" lastName="Orgonly"/>
        <mantle.party.PartyRole roleTypeId="Employee"/>
    </mantle.party.Party>
    <mantle.party.PartyRelationship partyRelationshipId="SF_EMP_REL" relationshipTypeEnumId="PrtEmployee" fromPartyId="SF_EMP"
            fromRoleTypeId="Employee" toPartyId="ORG_ZIZI_RETAIL" toRoleTypeId="OrgInternal" fromDate="{d30}"/>
    <moqui.security.UserAccount userId="SF_ORG_USER" username="sf.org.user" partyId="SF_EMP" userFullName="Olga Orgonly"/>
    <moqui.security.UserGroup userGroupId="SF_ORG_GROUP" description="Search fixture org users"/>
    <moqui.security.UserGroupMember userGroupId="SF_ORG_GROUP" userId="SF_ORG_USER" fromDate="{d30}"/>
    <moqui.security.ArtifactGroup artifactGroupId="SF_SEARCH" description="General search, limited to the user's organizations">
        <artifacts artifactName="mantle.GeneralServices.search#MantleFiltered" artifactTypeEnumId="AT_SERVICE" inheritAuthz="Y"/>
        <artifacts artifactName="mantle.party.PartyServices.setup#UserOrganizationInfo" artifactTypeEnumId="AT_SERVICE" inheritAuthz="Y"/>
        <artifacts artifactName="mantle.search.UniversalSearchServices.search#Universal" artifactTypeEnumId="AT_SERVICE" inheritAuthz="Y"/>
        <authz artifactAuthzId="SF_SEARCH_ORG" userGroupId="SF_ORG_GROUP" authzTypeEnumId="AUTHZT_ALWAYS" authzActionEnumId="AUTHZA_ALL">
            <filters entityFilterSetId="MANTLE_USER_ORG"/></authz>
        <authz artifactAuthzId="SF_SEARCH_ADMIN" userGroupId="ADMIN" authzTypeEnumId="AUTHZT_ALWAYS" authzActionEnumId="AUTHZA_ALL"/>
    </moqui.security.ArtifactGroup>
</entity-facade-xml>"#
    )
}

#[test]
fn mantle_search_callers_on_the_search_facade() {
    if !crate::common::own_process() {
        return;
    }
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let components = repo.join("runtime/component");
    if !components.join("mr-mantle").is_dir() {
        eprintln!("skipping: runtime/component/mr-mantle is not present");
        return;
    }
    let framework = repo.join("runtime/base-component/framework");
    // Short paths: the Postgres socket path must stay under 108 bytes.
    let runtime = temp_dir("msr");
    let cluster_dir = runtime.join("db");
    let _guard = DirGuard(runtime.clone());
    std::fs::create_dir_all(runtime.join("component")).unwrap();
    std::fs::create_dir_all(&cluster_dir).unwrap();
    copy_dir(&repo.join("runtime/conf"), &runtime.join("conf"));
    std::os::unix::fs::symlink(repo.join("runtime/base-component"), runtime.join("base-component")).unwrap();
    std::os::unix::fs::symlink(components.join("mr-mantle"), runtime.join("component/mr-mantle")).unwrap();

    // The fixture's dates are days before now at noon, so the weekly periods hold them whatever today is.
    let now = jiff::Zoned::now();
    let noon = now.date().at(12, 0, 0, 0).to_zoned(now.time_zone().clone()).unwrap();
    let at = |days: i64| noon.checked_sub(jiff::Span::new().days(days)).unwrap();
    let day = |days: i64| at(days).strftime("%Y-%m-%d %H:%M:%S").to_string();
    let fixture_dir = runtime.join("component/zz-search-fixture");
    std::fs::create_dir_all(fixture_dir.join("data")).unwrap();
    std::fs::write(
        fixture_dir.join("component.xml"),
        r#"<component name="zz-search-fixture"><depends-on name="mr-mantle"/></component>"#,
    )
    .unwrap();
    std::fs::write(fixture_dir.join("data/SearchFixtureData.xml"), fixture(&day)).unwrap();

    let flags = LoadFlags {
        data_types: Some(vec!["all".to_string()]),
        ..LoadFlags::default()
    };
    load_data(&runtime, &framework, &cluster_dir, &flags).unwrap();

    let (cluster, driver) = EmbeddedCluster::start(&cluster_dir, 30).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let factory = ExecutionContextFactory::boot_with_framework(&runtime, &framework).unwrap();
    let entity_dirs = moqui_host::db::entity_facade::component_entity_dirs(&factory);
    let facade = moqui_host::db::entity_facade::open_entity_facade_components(
        &framework,
        &entity_dirs,
        Arc::clone(&driver),
        pool(),
        "postgres",
    )
    .unwrap();
    moqui_host::db::entity_facade::attach(&factory, facade.clone());
    let store = moqui_host::search::attach_search(&factory, &facade, true).unwrap();

    {
        let context = factory.new_context();
        let _entered = context.enter();
        {
            let _authz = context.disable_authz();
            the_feed_indexed_everything(&facade);
            sales_reports(&day(1), &at(8));
            projected_inventory();
            party_search();
        }
        universal_search_results();
        shipped_sales_dashboard(&factory, &day(40), &day(-1));
        organization_limit();
    }
    drop(store);
    drop(factory);
    drop(facade);
    drop(driver);
    drop(cluster);
}

/// The component's actual shipped definitions run over the fixture's generated Mantle
/// documents. Scope the existing dashboard to the two completed/approved fixture orders
/// so unrelated demo orders and the deliberately cancelled order do not change its totals.
fn shipped_sales_dashboard(factory: &Arc<ExecutionContextFactory>, from: &str, thru: &str) {
    use serde_json::json;
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let service = moqui_core::native::ServiceInfo {
        name: "org.moqui.report.ReportServices.test#MantleFixture",
        service_type: "rust",
        location: "test",
        method: "",
    };
    let parameters = moqui_core::native::Parameters::from([("definitionId".into(), Value::string("mantle-sales"))]);
    let saved = moqui_report::get_definition(&context, &service, &parameters).unwrap();
    let saved = moqui_lib::json::to_json(&Value::Map(saved.into_iter().collect()));
    let mut definition = saved["definition"].clone();
    assert_eq!(
        definition["readOnly"], true,
        "sales must load from the real component file"
    );
    for panel in definition["body"]["panels"].as_object_mut().unwrap().values_mut() {
        for query in panel["queries"].as_array_mut().unwrap() {
            query["filters"] = json!([{"field":{"name":"orderId","op":"in","values":["SF_ORD_1","SF_ORD_2"]}}]);
            for agg in query["aggregations"].as_array_mut().unwrap() {
                if agg["type"] == "date_histogram" {
                    agg["params"]["drop_partial"] = json!(false);
                }
            }
        }
    }
    let from = from.replace(' ', "T") + "Z";
    let thru = thru.replace(' ', "T") + "Z";
    let rendered = moqui_report::dashboard_frames(&context, &definition, &json!({}), &from, &thru).unwrap();
    let total = |panel: &str, field: &str| -> f64 {
        let panel = &rendered["panels"][panel];
        assert!(
            panel["notices"]
                .as_array()
                .unwrap()
                .iter()
                .all(|notice| notice["complete"] != false),
            "{panel}"
        );
        panel["frames"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|frame| frame["fields"].as_array().unwrap())
            .filter(|candidate| candidate["name"] == field)
            .flat_map(|field| field["values"].as_array().unwrap())
            .map(|value| {
                value
                    .as_f64()
                    .unwrap_or_else(|| value.as_str().unwrap().parse().unwrap())
            })
            .sum()
    };
    assert_eq!(
        total("monthly", "sales"),
        33.0,
        "the shared native monthly panel includes product sales less discount"
    );
    assert_eq!(total("status", "orders"), 2.0);
    assert_eq!(total("products", "sales"), 35.0);
    assert_eq!(total("products", "quantity"), 5.0);
    // The datasets' field defaults: entity lookups and status labels for ids, and the amount's unit from currencyUomId.
    let field = |panel: &str, name: &str| -> serde_json::Value {
        rendered["panels"][panel]["frames"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|frame| frame["fields"].as_array().unwrap())
            .find(|field| field["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("{panel}.{name}: {}", rendered["panels"][panel]))
    };
    let label = |field: &serde_json::Value, value: &str| -> String {
        field["config"]["mappings"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|mapping| mapping["value"] == value)
            .and_then(|mapping| mapping["text"].as_str())
            .unwrap_or_else(|| panic!("no label for {value}: {field}"))
            .to_string()
    };
    let product = field("products", "product");
    assert!(
        !label(&product, "DEMO_1_1").is_empty() && label(&product, "DEMO_1_1") != "DEMO_1_1",
        "{product}"
    );
    let status = field("status", "status");
    assert_eq!(label(&status, "OrderCompleted"), "Completed", "{status}");
    let sales = field("products", "sales");
    assert_eq!(sales["config"]["unitField"], "currencyUomId", "{sales}");
    assert_eq!(
        sales["unit"], "USD",
        "the unit comes from the orders' currency: {sales}"
    );
    assert!(rendered["panels"]["monthly"]["frames"]
        .as_array()
        .unwrap()
        .iter()
        .any(|frame| frame["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field["name"] == "month" && field["type"] == "time")));
}

/// No pending feed rows, and every Mantle document type that has records is in its store; the wiki pages carry
/// their page text from the manual data service.
fn the_feed_indexed_everything(facade: &moqui_core::entity::EntityFacade) {
    let pending = facade
        .find("moqui.entity.feed.DataFeedPending")
        .unwrap()
        .count()
        .unwrap();
    assert_eq!(pending, 0, "feed rows left pending");
    let count = |document_type: &str| {
        let out = call(
            "org.moqui.search.SearchServices.search#DataDocuments",
            &[
                ("indexName", Value::string("mantle")),
                ("documentType", Value::string(document_type)),
                ("queryString", Value::string("*:*")),
                ("pageSize", Value::I32(100)),
            ],
        );
        number(out.get("documentListCount"))
    };
    for document_type in ["MantleParty", "MantleProduct", "MantleFacility", "MantleWikiPage"] {
        assert!(count(document_type) > 0.0, "{document_type} has no documents");
    }
    let wiki = call(
        "org.moqui.search.SearchServices.search#DataDocuments",
        &[
            ("indexName", Value::string("mantle")),
            ("documentType", Value::string("MantleWikiPage")),
            ("queryString", Value::string("\"About Us\"")),
        ],
    );
    assert_eq!(
        number(wiki.get("documentListCount")),
        1.0,
        "the about page by its text: {wiki:?}"
    );
}

/// The four sales report services over the fixture. Totals are summed over every period line so the result does
/// not depend on where today falls in the week.
fn sales_reports(yesterday: &str, order_one: &jiff::Zoned) {
    let orders = call(
        "mantle.sales.SalesReportServices.get#SalesOrderSummary",
        &[("interval", Value::string("w"))],
    );
    let lines = periods(&orders, "salesOrderSummaryList");
    assert_eq!(sum(&lines, "orderCount"), 2.0, "the cancelled order is out: {lines:?}");
    assert_eq!(sum(&lines, "newCustomerOrderCount"), 1.0);
    assert_eq!(sum(&lines, "productSaleTotal"), 35.0);
    assert_eq!(sum(&lines, "discountTotal"), -2.0);
    assert_eq!(sum(&lines, "totalSales"), 33.0);
    assert_eq!(sum(&lines, "netSales"), 33.0);
    assert_eq!(sum(&lines, "productQuantityTotal"), 5.0);

    let invoices = call(
        "mantle.sales.SalesReportServices.get#SalesInvoiceSummary",
        &[("interval", Value::string("w"))],
    );
    let lines = periods(&invoices, "salesInvoiceSummaryList");
    assert_eq!(sum(&lines, "invoiceCount"), 2.0, "{lines:?}");
    assert_eq!(sum(&lines, "invoiceTotal"), 35.0);
    assert_eq!(sum(&lines, "invoicePaymentsApplied"), 20.0);
    assert_eq!(
        sum(&lines, "prePaymentsTotal"),
        20.0,
        "paid the day before the invoice date"
    );
    assert_eq!(sum(&lines, "productSaleTotal"), 35.0);

    // The week before the one seven days after the first order is that order's week.
    let thru = order_one
        .checked_add(jiff::Span::new().days(7))
        .unwrap()
        .strftime("%Y-%m-%d")
        .to_string();
    let top = call(
        "mantle.sales.SalesReportServices.get#SalesTopProduct",
        &[("interval", Value::string("w")), ("thruDate", Value::string(&thru))],
    );
    let products = list(top.get("topProductList"));
    assert_eq!(products.len(), 1, "{top:?}");
    assert_eq!(text(products[0].get("productId")), "DEMO_1_1");
    assert_eq!(number(products[0].get("totalSales")), 20.0);
    assert_eq!(number(products[0].get("totalQuantity")), 2.0);

    let classes = call(
        "mantle.sales.SalesReportServices.get#InvoicesByCustomerClass",
        &[
            ("interval", Value::string("M")),
            ("poffset", Value::I32(0)),
            ("thruDate", Value::string(yesterday)),
        ],
    );
    let by_class: BTreeMap<String, f64> = list(classes.get("topCustomerClassList"))
        .iter()
        .filter(|line| line.get("customerClassId").is_some())
        .map(|line| (text(line.get("customerClassId")), number(line.get("invoiceTotal"))))
        .collect();
    // Both invoices fall in this month only when the month is old enough; either way each class has its own total.
    for (class, total) in &by_class {
        match class.as_str() {
            "CustConsumer" => assert_eq!(*total, 20.0),
            "CustWholesale" => assert_eq!(*total, 15.0),
            other => panic!("unexpected customer class {other}: {classes:?}"),
        }
    }
}

/// Open order quantity per product from the order items, and the available quantity from the assets.
fn projected_inventory() {
    let out = call(
        "mantle.product.InventoryReportServices.get#InventoryProjectedInfo",
        &[(
            "productIdList",
            Value::List(vec![Value::string("DEMO_1_1"), Value::string("DEMO_2_1")]),
        )],
    );
    let info = map(out.get("infoByProductId"));
    let demo_2 = map(info.get("DEMO_2_1"));
    assert_eq!(
        number(demo_2.get("orderQuantitySum")),
        3.0,
        "the approved order: {out:?}"
    );
    let demo_1 = map(info.get("DEMO_1_1"));
    assert_eq!(
        number(demo_1.get("orderQuantitySum")),
        0.0,
        "completed and cancelled orders are out"
    );
    assert!(number(demo_1.get("availableToPromiseTotalSum")) > 0.0);
}

/// Party search by any field, with highlights.
fn party_search() {
    let out = call(
        "mantle.party.PartyServices.search#Party",
        &[("anyField", Value::string("Fixture"))],
    );
    let ids: Vec<String> = list(out.get("documentList"))
        .iter()
        .map(|doc| text(doc.get("partyId")))
        .collect();
    assert!(
        ids.contains(&"SF_CUST_A".to_string()) && ids.contains(&"SF_CUST_B".to_string()),
        "{ids:?}"
    );
}

/// The universal search as an administrator: typed results with titles, links, and the facility found by its
/// title prefix; a mode the backend lacks is a message, not a silent fallback.
fn universal_search_results() {
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let out = call(
        "mantle.search.UniversalSearchServices.search#Universal",
        &[
            ("q", Value::string("fixture")),
            ("entityType", Value::string("MantleOrder,MantleInvoice")),
        ],
    );
    let titles: Vec<String> = list(out.get("results"))
        .iter()
        .map(|result| text(result.get("title")))
        .collect();
    assert_eq!(
        number(out.get("total")),
        5.0,
        "three orders and two invoices for the fixture customers: {titles:?}"
    );
    for result in list(out.get("results")) {
        assert!(text(result.get("url")).contains("AutoEditMaster"), "{result:?}");
    }
    let suggest = call(
        "mantle.search.UniversalSearchServices.search#Universal",
        &[
            ("q", Value::string("ziziwork ret")),
            ("mode", Value::string("autocomplete")),
        ],
    );
    let suggested: Vec<String> = list(suggest.get("results"))
        .iter()
        .map(|result| text(result.get("title")))
        .collect();
    assert!(
        suggested.contains(&"Ziziwork Retail Warehouse".to_string()),
        "{suggested:?}"
    );
    let parameters: BTreeMap<String, Value> = [("q", Value::string("pump")), ("mode", Value::string("semantic"))]
        .iter()
        .map(|(key, value)| (key.to_string(), value.clone()))
        .collect();
    let _ = moqui_core::service::sync("mantle.search.UniversalSearchServices.search#Universal", &parameters);
    let errors = moqui_core::context::ExecutionContext::with_current(|ec| {
        let errors = ec.message().errors().to_vec();
        ec.message().clear_errors();
        errors
    })
    .unwrap_or_default();
    assert!(
        errors.iter().any(|error| error.contains("semantic capability")),
        "{errors:?}"
    );
    moqui_core::user::logout().unwrap();
}

/// The general search as a user of the retail org: everything found is owned by one of the user's organizations,
/// and less is found than without the limit.
fn organization_limit() {
    let search = || {
        call(
            "mantle.GeneralServices.search#MantleFiltered",
            &[
                ("indexName", Value::string("mantle")),
                ("queryString", Value::string("Ziziwork")),
                ("pageSize", Value::I32(100)),
            ],
        )
    };
    // an administrator with no active organization has no limit
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let everything = number(search().get("documentListCount"));
    let universal_all = universal_search();
    moqui_core::user::logout().unwrap();

    moqui_core::user::internal_login_user("sf.org.user").unwrap();
    let orgs = call("mantle.party.PartyServices.setup#UserOrganizationInfo", &[]);
    let allowed: Vec<String> = match orgs.get("filterOrgIds") {
        Some(Value::List(ids)) => ids.iter().map(to_plain_string).collect(),
        _ => Vec::new(),
    };
    assert!(allowed.contains(&"ORG_ZIZI_RETAIL".to_string()), "{orgs:?}");
    let limited = search();
    let documents = list(limited.get("documentList"));
    assert!(
        !documents.is_empty(),
        "the user's own organization's records: {limited:?}"
    );
    for document in &documents {
        let owner = text(document.get("ownerPartyId"));
        assert!(allowed.contains(&owner), "{owner} is not one of {allowed:?}");
    }
    assert!(
        number(limited.get("documentListCount")) < everything,
        "the limit removes other organizations' records"
    );

    // the universal search under the same limit: only the user's organizations, and no type without an organization
    let universal = universal_search();
    let all_total = number(universal_all.get("total"));
    let total = number(universal.get("total"));
    assert!(total > 0.0 && total < all_total, "{total} of {all_total}");
    let type_counts: f64 = list(map(universal.get("facetCounts")).get("entityType"))
        .iter()
        .map(|facet| number(facet.get("count")))
        .sum();
    assert_eq!(
        type_counts, total,
        "the type facet counts the same authorized matches as the total"
    );
    for result in list(universal.get("results")) {
        let entity_type = text(result.get("entityType"));
        assert_ne!(entity_type, "MantleCommunicationEvent");
        let attributes = map(result.get("attributes"));
        let key = map(result.get("key"));
        let owners: Vec<String> = ["ownerPartyId", "fromPartyId", "toPartyId"]
            .iter()
            .filter_map(|name| {
                attributes
                    .get(*name)
                    .or_else(|| key.get(*name))
                    .map(|value| text(Some(value)))
            })
            .collect();
        if entity_type == "MantleParty" || entity_type == "MantleFacility" || entity_type == "MantleAsset" {
            assert!(
                owners.iter().any(|owner| allowed.contains(owner) || owner == "_NA_"),
                "{entity_type} {owners:?} {result:?}"
            );
        }
    }
    moqui_core::user::logout().unwrap();
}

/// The universal search for a name every organization in the demo data has, with the type facet.
fn universal_search() -> BTreeMap<String, Value> {
    call(
        "mantle.search.UniversalSearchServices.search#Universal",
        &[
            ("q", Value::string("ziziwork")),
            ("limit", Value::I32(100)),
            ("facets", Value::List(vec![Value::string("entityType")])),
        ],
    )
}

fn call(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| (key.to_string(), value.clone()))
        .collect();
    let out = moqui_core::service::sync(name, &parameters).unwrap_or_else(|err| panic!("{name}: {err}"));
    let errors =
        moqui_core::context::ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap_or_default();
    assert!(errors.is_empty(), "{name}: {errors:?}");
    out
}

/// The period lines of a summary, without the average and total lines at the end.
fn periods(out: &BTreeMap<String, Value>, name: &str) -> Vec<BTreeMap<String, Value>> {
    let lines = list(out.get(name));
    lines[..lines.len().saturating_sub(2)].to_vec()
}

fn sum(lines: &[BTreeMap<String, Value>], field: &str) -> f64 {
    let total: f64 = lines.iter().map(|line| number(line.get(field))).sum();
    (total * 1000.0).round() / 1000.0
}

fn list(value: Option<&Value>) -> Vec<BTreeMap<String, Value>> {
    match value {
        Some(Value::List(items)) => items.iter().map(|item| map(Some(item))).collect(),
        _ => Vec::new(),
    }
}

fn map(value: Option<&Value>) -> BTreeMap<String, Value> {
    match value {
        Some(Value::Map(entries)) => entries
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        _ => BTreeMap::new(),
    }
}

fn number(value: Option<&Value>) -> f64 {
    value
        .map(to_plain_string)
        .and_then(|text| text.parse().ok())
        .unwrap_or(0.0)
}

fn text(value: Option<&Value>) -> String {
    value.map(to_plain_string).unwrap_or_default()
}

fn pool() -> PoolConfig {
    PoolConfig {
        min: 0,
        max: 12,
        reserve: 2,
        acquire_timeout: Duration::from_secs(30),
        ..Default::default()
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    }
}

/// A path under `/tmp`. The runtimes share one Postgres install in `target/<profile>/moqui-test-install`, rather than
/// each unpacking its own.
fn temp_dir(prefix: &str) -> PathBuf {
    static SHARED: std::sync::Once = std::sync::Once::new();
    SHARED.call_once(|| {
        let exe = std::env::current_exe().unwrap();
        let profile_dir = exe.ancestors().nth(2).expect("target directory");
        moqui_lib::env::set_property(
            "MOQUI_INSTALL",
            profile_dir.join("moqui-test-install").display().to_string(),
        );
    });
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let name = format!("{prefix}-{}-{nanos}", std::process::id());
    if Path::new("/tmp").is_dir() {
        PathBuf::from("/tmp").join(name)
    } else {
        std::env::temp_dir().join(name)
    }
}

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
