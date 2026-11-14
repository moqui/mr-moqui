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
