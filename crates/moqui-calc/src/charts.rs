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
//! A CalcGrid document's charts in its xlsx (plans/archive/IronCalc-DB.md, Phase 4): the charts of the document's body
//! (a title, a type of bar, column, line, area or pie, a categories range and series of a name and a values range,
//! drawn on the screen with Chart.js) as charts of the engine, on a sheet of their own, which Excel and LibreOffice
//! draw from the cells as the screen does. Each chart has a stable `id` ([`with_ids`]), by which a dashboard panel
//! reads it as a table ([`frame_of`], `get#CalcFrame`'s `chart`; plans/IronCalc-DB2.md CG4.2).

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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

/// The longest chart id kept as a client sends it.
const MAX_ID: usize = 64;
/// Ids made on this server, so two made in one millisecond differ.
static ID_SEQ: AtomicU64 = AtomicU64::new(0);

/// Whether `id` is one a chart keeps: letters, digits, `-` and `_`, at most [`MAX_ID`].
fn is_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

/// A new chart id: `c` and hexadecimal digits, never one of [`saved_ids`]' `chart<n>`.
fn new_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    format!("c{millis:x}{:03x}", ID_SEQ.fetch_add(1, Ordering::Relaxed) % 0x1000)
}

/// The charts of a `charts` edit, each with a stable `id` (plans/IronCalc-DB2.md CG4.2): the one the view sent with
/// it, a chart of the list it read, else a new one, as the edit arrives and before it is in the document's log, so
/// every room that follows the log gives a chart the same id. A dashboard panel names a chart by it
/// (`get#CalcFrame`'s `chart`). An id sent twice is kept the first time.
#[must_use]
pub fn with_ids(charts: Vec<Json>) -> Vec<Json> {
    let mut seen = BTreeSet::new();
    charts
        .into_iter()
        .map(|mut chart| {
            let kept = chart["id"]
                .as_str()
                .filter(|id| is_id(id) && !seen.contains(*id))
                .map(str::to_string);
            let id = kept.unwrap_or_else(|| {
                let mut made = new_id();
                while seen.contains(&made) {
                    made = new_id();
                }
                made
            });
            seen.insert(id.clone());
            if let Some(object) = chart.as_object_mut() {
                object.insert("id".into(), Json::String(id));
            }
            chart
        })
        .collect()
}

/// A saved document's charts as a room opens them: one saved before charts had ids is given `chart<n>` by its place
/// (from 1), the same in every room of the document, and keeps it when the document is next saved.
#[must_use]
pub fn saved_ids(charts: Vec<Json>) -> Vec<Json> {
    charts
        .into_iter()
        .enumerate()
        .map(|(index, mut chart)| {
            let has = chart["id"].as_str().is_some_and(is_id);
            if let (false, Some(object)) = (has, chart.as_object_mut()) {
                object.insert("id".into(), Json::String(format!("chart{}", index + 1)));
            }
            chart
        })
        .collect()
}

/// The chart `wanted` names among `charts`: the one of that id, else the one at that index (from 0), for a panel added
/// before the document's charts had ids.
#[must_use]
pub fn find<'a>(charts: &'a [Json], wanted: &str) -> Option<&'a Json> {
    charts
        .iter()
        .find(|chart| chart["id"].as_str() == Some(wanted))
        .or_else(|| wanted.parse::<usize>().ok().and_then(|index| charts.get(index)))
}

/// The chart `wanted` names ([`find`]) among a room's charts as it read them (`Room::charts`), as `get#CalcFrame`
/// answers it: its table ([`frame_of`]), its type and its title.
///
/// # Errors
///
/// No such chart, or the chart's own error.
pub fn chart_frame(charts: &Json, wanted: &str) -> Result<(Json, String, String), Error> {
    let found = find(charts.as_array().map(Vec::as_slice).unwrap_or_default(), wanted)
        .ok_or_else(|| Error::new(format!("The document has no chart {wanted}")))?;
    let frame = frame_of(found)?;
    Ok((
        frame,
        found["type"].as_str().unwrap_or("column").to_string(),
        found["title"].as_str().unwrap_or_default().to_string(),
    ))
}

/// A chart as a table for the report engine, from the values a room read for it (`Room::charts`: its `labels` and
/// each series' `data`): a label column, then a column of numbers for each series, named by the series (made unique),
/// one row for each point. A chart with no labels is labelled by the point's place, from 1.
///
/// # Errors
///
/// The chart's own error, when its ranges did not read.
pub fn frame_of(chart: &Json) -> Result<Json, Error> {
    if let Some(error) = chart["error"].as_str() {
        return Err(Error::new(error));
    }
    let series = chart["series"].as_array().map(Vec::as_slice).unwrap_or_default();
    let labels = chart["labels"].as_array().map(Vec::as_slice).unwrap_or_default();
    let points = series
        .iter()
        .map(|one| one["data"].as_array().map_or(0, Vec::len))
        .max()
        .unwrap_or(0)
        .max(labels.len());
    let label_values: Vec<Json> = (0..points)
        .map(|at| match labels.get(at) {
            Some(Json::String(label)) => Json::String(label.clone()),
            _ => Json::String((at + 1).to_string()),
        })
        .collect();
    let mut names = BTreeSet::from(["Label".to_string()]);
    let mut fields = vec![serde_json::json!({"name": "Label", "type": "string", "values": label_values})];
    for (at, one) in series.iter().enumerate() {
        let given = one["name"]
            .as_str()
            .filter(|name| !name.trim().is_empty())
            .map_or_else(|| format!("Series {}", at + 1), str::to_string);
        let mut name = given.clone();
        let mut again = 2;
        while names.contains(&name) {
            name = format!("{given} {again}");
            again += 1;
        }
        names.insert(name.clone());
        let data = one["data"].as_array().map(Vec::as_slice).unwrap_or_default();
        let values: Vec<Json> = (0..points)
            .map(|point| {
                data.get(point)
                    .filter(|value| value.is_number())
                    .cloned()
                    .unwrap_or(Json::Null)
            })
            .collect();
        fields.push(serde_json::json!({"name": name, "type": "number", "values": values}));
    }
    Ok(serde_json::json!({ "fields": fields }))
}

#[cfg(test)]
#[path = "charts_tests.rs"]
mod tests;
