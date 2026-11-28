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

use std::io::Read;

use moqui_core::workbook::ironcalc::base::drawings::{ChartKind, DrawingKind};
use serde_json::json;

use super::*;
use crate::fill;

fn data_model() -> Model<'static> {
    let mut model = fill::new_model("Sales").unwrap();
    for (row, (region, amount)) in [("North", "10"), ("South", "7"), ("East", "13")].iter().enumerate() {
        let row = i32::try_from(row).unwrap() + 2;
        model.set_user_input(0, row, 1, (*region).to_string()).unwrap();
        model.set_user_input(0, row, 2, (*amount).to_string()).unwrap();
    }
    model.evaluate();
    model
}

#[test]
fn a_documents_charts_go_on_a_sheet_of_their_own() {
    let mut model = data_model();
    let charts = vec![
        json!({"title": "By region", "type": "bar", "categories": "Sales!A2:A4", "series": [{"name": "Amount", "values": "Sales!B2:B4"}]}),
        json!({"title": "Share", "type": "pie", "categories": "A2:A4", "series": [{"name": "Amount", "values": "B2:B4"}]}),
        json!({"title": "Broken", "type": "line", "series": [{"name": "x", "values": "Nowhere!A1:A2"}]}),
        json!({"title": "Empty", "type": "column", "series": []}),
    ];
    let left_out = add_to_model(&mut model, &charts).unwrap();
    assert_eq!(left_out.len(), 2, "{left_out:?}");
    assert!(left_out[0].starts_with("Chart 3:"), "{left_out:?}");
    assert_eq!(left_out[1], "Chart 4 has no series");

    assert_eq!(model.workbook.get_worksheet_names(), ["Sales", "Charts"]);
    let drawings = model.get_drawings(1).unwrap();
    assert_eq!(drawings.len(), 2);
    let DrawingKind::Chart(bar) = &drawings[0].kind else {
        panic!("a chart");
    };
    assert_eq!(bar.kind, ChartKind::Bar);
    assert_eq!(bar.title.as_deref(), Some("By region"));
    assert_eq!(bar.series[0].values.height, 3);
    assert_eq!(bar.series[0].color.as_deref(), Some("1976D2"));
    let DrawingKind::Chart(pie) = &drawings[1].kind else {
        panic!("a chart");
    };
    assert_eq!(pie.kind, ChartKind::Pie);
    assert!(pie.series[0].color.is_none());

    // a sheet named Charts already: the next name
    let mut again = data_model();
    again.add_sheet("Charts").unwrap();
    add_to_model(&mut again, &charts[..1]).unwrap();
    assert_eq!(again.workbook.get_worksheet_names(), ["Sales", "Charts", "Charts 2"]);
    // none: nothing added
    let mut none = data_model();
    assert!(add_to_model(&mut none, &[]).unwrap().is_empty());
    assert_eq!(none.workbook.worksheets.len(), 1);
}

#[test]
fn the_charts_are_in_the_xlsx() {
    let mut model = data_model();
    add_to_model(&mut model, &[json!({"title": "By region", "type": "column", "categories": "A2:A4", "series": [{"name": "Amount", "values": "B2:B4"}]})])
        .unwrap();
    let bytes = moqui_core::workbook::model_xlsx(&model).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut chart = String::new();
    archive
        .by_name("xl/charts/chart1.xml")
        .unwrap()
        .read_to_string(&mut chart)
        .unwrap();
    assert!(chart.contains("<c:barDir val=\"col\"/>"), "{chart}");
    assert!(chart.contains("Sales!$B$2:$B$4"), "{chart}");
}

/// A charts edit's charts keep the ids the view read and the new one gets its own; one sent twice is kept once.
#[test]
fn each_chart_has_a_stable_id() {
    let charts = with_ids(vec![
        json!({"id": "kept-1", "title": "Kept"}),
        json!({"title": "New"}),
        json!({"id": "kept-1", "title": "Twice"}),
        json!({"id": "no spaces or ' quotes", "title": "Unsafe"}),
    ]);
    let ids: Vec<&str> = charts.iter().map(|chart| chart["id"].as_str().unwrap()).collect();
    assert_eq!(ids[0], "kept-1");
    for made in &ids[1..] {
        assert!(
            made.starts_with('c') && made[1..].chars().all(|ch| ch.is_ascii_hexdigit()),
            "{made}"
        );
    }
    let unique: std::collections::BTreeSet<&&str> = ids.iter().collect();
    assert_eq!(unique.len(), 4);
    assert_eq!(charts[1]["title"], "New");
    // sent again as the view read them, they are the same
    assert_eq!(with_ids(charts.clone()), charts);

    // a document saved before charts had ids: by place, the same at every open, and an id it has is kept
    let saved = vec![
        json!({"title": "First"}),
        json!({"id": "c1", "title": "Second"}),
        json!({"title": "Third"}),
    ];
    let opened = saved_ids(saved.clone());
    assert_eq!(
        opened.iter().map(|chart| chart["id"].clone()).collect::<Vec<_>>(),
        [json!("chart1"), json!("c1"), json!("chart3")]
    );
    assert_eq!(saved_ids(saved), opened);
    // found by id, else by index for a panel added before
    assert_eq!(find(&opened, "c1").unwrap()["title"], "Second");
    assert_eq!(find(&opened, "2").unwrap()["title"], "Third");
    assert!(find(&opened, "nosuch").is_none());
    assert!(find(&opened, "9").is_none());
}

/// A chart, as a room read its ranges, is a table: its labels, then a column of numbers for each series.
#[test]
fn a_chart_is_a_frame_of_its_labels_and_a_column_for_each_series() {
    let chart = json!({"id": "c1", "title": "By region", "type": "bar", "labels": ["North", "South", "East"],
        "series": [{"name": "Amount", "data": [10.0, null, 13.0]}, {"name": "Amount", "data": [1, 2]}, {"name": "", "data": [5, 6, 7, 8]}]});
    let frame = frame_of(&chart).unwrap();
    let fields = frame["fields"].as_array().unwrap();
    let names: Vec<&str> = fields.iter().map(|field| field["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Label", "Amount", "Amount 2", "Series 3"]);
    assert_eq!(fields[0]["type"], "string");
    // the longest series sets the rows; a point with no label is labelled by its place
    assert_eq!(fields[0]["values"], json!(["North", "South", "East", "4"]));
    assert_eq!(fields[1]["type"], "number");
    assert_eq!(fields[1]["values"], json!([10.0, null, 13.0, null]));
    assert_eq!(fields[2]["values"], json!([1, 2, null, null]));
    // with no categories, each point by its place
    let plain = frame_of(&json!({"labels": [], "series": [{"name": "x", "data": [3, 4]}]})).unwrap();
    assert_eq!(plain["fields"][0]["values"], json!(["1", "2"]));
    // a series named Label does not take the label column's name
    let label = frame_of(&json!({"labels": ["a"], "series": [{"name": "Label", "data": [1]}]})).unwrap();
    assert_eq!(label["fields"][1]["name"], "Label 2");
    // a chart whose ranges did not read is its error
    let broken = frame_of(&json!({"error": "no sheet named Nowhere"})).unwrap_err();
    assert!(broken.message().contains("Nowhere"));
}
