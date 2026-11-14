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
//! A CalcGrid document's charts in its xlsx (plans/IronCalc-DB.md, Phase 4): the charts of the document's body (a title,
//! a type of bar, column, line, area or pie, a categories range and series of a name and a values range, drawn on the
//! screen with Chart.js) as charts of the engine, on a sheet of their own, which Excel and LibreOffice draw from the
//! cells as the screen does.

use moqui_core::context::Error;
use moqui_core::workbook::ironcalc::base::drawings::{
    Anchor, CellPoint, Chart, ChartKind, ChartSeries, Drawing, DrawingKind, LegendPosition, SeriesName, SheetArea,
};
use moqui_core::workbook::ironcalc::base::Model;
use moqui_core::workbook::parse_area;
use serde_json::Value as Json;

/// The colours of the screen's charts (`CHART_COLORS` in WebrootCalc.js), series by series.
const COLORS: [&str; 8] = ["1976D2", "26A69A", "EF6C00", "9C27B0", "C62828", "558B2F", "00838F", "6D4C41"];
/// The rows and columns a chart takes on the charts' sheet, and the rows between two.
const CHART_ROWS: i32 = 18;
const CHART_COLUMNS: i32 = 9;
const GAP_ROWS: i32 = 2;

fn engine(message: String) -> Error {
    Error::new(format!("Spreadsheet engine: {message}"))
}

/// A range of the document as the engine's chart reads it.
fn area(model: &Model<'_>, reference: &str) -> Result<SheetArea, Error> {
    let found = parse_area(model, reference)?;
    let sheet_id = model
        .workbook
        .worksheets
        .get(found.sheet as usize)
        .map(|worksheet| worksheet.sheet_id)
        .ok_or_else(|| Error::new(format!("{reference} is not in the workbook")))?;
    Ok(SheetArea {
        sheet_id,
        row: found.row,
        column: found.column,
        width: found.last_column - found.column + 1,
        height: found.last_row - found.row + 1,
    })
}

/// One chart of the document as the engine's.
fn chart(model: &Model<'_>, chart: &Json) -> Result<Chart, Error> {
    let kind = match chart["type"].as_str().unwrap_or_default() {
        // a bar is drawn across, as the screen draws it; a column upright
        "bar" => ChartKind::Bar,
        "column" => ChartKind::Column,
        "area" => ChartKind::Area,
        "pie" => ChartKind::Pie,
        _ => ChartKind::Line,
    };
    let mut made = Chart::new(kind);
    made.title = chart["title"].as_str().filter(|title| !title.is_empty()).map(str::to_string);
    let series_list = chart["series"].as_array().map(Vec::as_slice).unwrap_or_default();
    made.legend = (kind == ChartKind::Pie || series_list.len() > 1).then_some(LegendPosition::Bottom);
    made.value_axis.min = (kind != ChartKind::Pie).then_some(0.0);
    let categories = match chart["categories"].as_str().filter(|text| !text.trim().is_empty()) {
        Some(reference) => Some(area(model, reference)?),
        None => None,
    };
    for (index, one) in series_list.iter().enumerate() {
        made.series.push(ChartSeries {
            name: one["name"]
                .as_str()
                .filter(|name| !name.is_empty())
                .map(|name| SeriesName::Text(name.to_string())),
            categories,
            values: area(model, one["values"].as_str().unwrap_or_default())?,
            color: (kind != ChartKind::Pie).then(|| COLORS[index % COLORS.len()].to_string()),
        });
    }
    Ok(made)
}

/// The name of the sheet the charts go on: Charts, or Charts 2 and so on when the workbook has a sheet so named.
fn sheet_name(model: &Model<'_>) -> String {
    let names = model.workbook.get_worksheet_names();
    let taken = |name: &str| names.iter().any(|found| found.eq_ignore_ascii_case(name));
    if !taken("Charts") {
        return "Charts".to_string();
    }
    (2..).map(|n| format!("Charts {n}")).find(|name| !taken(name)).unwrap_or_default()
}

/// Puts the document's `charts` on a sheet of their own at the end of `model`, one under the other, and gives what was
/// left out: a chart whose ranges are not in the workbook is said and skipped. Nothing is added when there is no chart.
///
/// # Errors
///
/// Returns the engine's error when the sheet cannot be added.
pub fn add_to_model(model: &mut Model<'static>, charts: &[Json]) -> Result<Vec<String>, Error> {
    let mut left_out = Vec::new();
    let mut made = Vec::new();
    for (index, one) in charts.iter().enumerate() {
        match chart(model, one) {
            Ok(chart) if !chart.series.is_empty() => made.push((index, chart)),
            Ok(_) => left_out.push(format!("Chart {} has no series", index + 1)),
            Err(err) => left_out.push(format!("Chart {}: {}", index + 1, err.message())),
        }
    }
    if made.is_empty() {
        return Ok(left_out);
    }
    let name = sheet_name(model);
    model.add_sheet(&name).map_err(engine)?;
    let sheet = u32::try_from(model.workbook.worksheets.len() - 1).unwrap_or(u32::MAX);
    for (place, (index, chart)) in made.into_iter().enumerate() {
        let top = 2 + i32::try_from(place).unwrap_or(0) * (CHART_ROWS + GAP_ROWS);
        let drawing = Drawing {
            anchor: Anchor::TwoCell {
                from: CellPoint::at(top, 2),
                to: CellPoint::at(top + CHART_ROWS, 2 + CHART_COLUMNS),
            },
            name: format!("Chart {}", index + 1),
            description: chart.title.clone().unwrap_or_default(),
            kind: DrawingKind::Chart(Box::new(chart)),
        };
        model.add_drawing(sheet, drawing).map_err(engine)?;
    }
    Ok(left_out)
}

#[cfg(test)]
#[path = "charts_tests.rs"]
mod tests;
