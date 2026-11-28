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

//! An uploaded workbook as a document's overlay: its formulas over the data sheet read the source's rows, and a sheet,
//! table or name of the data sheet's is refused.

use moqui_core::workbook::ironcalc::base::DefinedNameEntry;
use moqui_core::workbook::ironcalc::base::Model;

use super::*;
use crate::fill::{self, CellData, SheetRows};

fn orders(count: usize) -> SheetRows {
    SheetRows {
        name: "Orders".to_string(),
        columns: vec!["customer".to_string(), "amount".to_string()],
        rows: (1..=count)
            .map(|row| {
                #[allow(clippy::cast_precision_loss)]
                let amount = row as f64 * 10.0;
                vec![CellData::Text(format!("Customer {row}")), CellData::Number(amount)]
            })
            .collect(),
        truncated: false,
    }
}

/// An author's workbook as a file: a Summary sheet whose formulas name the Orders data before it is there, as a
/// workbook made elsewhere does, and a sheet of the author's own values.
fn uploaded() -> Model<'static> {
    let mut model = fill::new_model("Summary").unwrap();
    for (row, formula) in ["=SUM(Orders[amount])", "=COUNTA(Orders!A:A)-1", "=Notes!A1*2"]
        .iter()
        .enumerate()
    {
        model
            .set_user_input(0, i32::try_from(row + 1).unwrap(), 1, (*formula).to_string())
            .unwrap();
    }
    model.add_sheet("Notes").unwrap();
    model.set_user_input(1, 1, 1, "21".to_string()).unwrap();
    model.evaluate();
    let bytes = moqui_core::workbook::model_xlsx(&model).unwrap();
    moqui_core::workbook::open_xlsx(&bytes, "upload.xlsx").unwrap()
}

#[test]
fn an_uploaded_workbooks_formulas_read_the_data_sheet_filled_from_the_source() {
    let model = uploaded();
    assert_eq!(overlay_refusal(&model, &["Orders".to_string()]), None);
    // read from the file, a table it does not have yet is an error until the formula is typed again
    let mut unready = fill::fill(uploaded(), &[orders(3)]).unwrap();
    unready.evaluate();
    assert_eq!(unready.get_formatted_cell_value(0, 1, 1).unwrap(), "#ERROR!");
    // prepared over the data sheet's stub, then filled as a room fills it: every formula reads the source's rows
    let prepared = prepare(model, &[orders(3)]).unwrap();
    assert_eq!(prepared.workbook.get_worksheet_names(), ["Summary", "Notes", "Orders"]);
    assert_eq!(
        prepared.get_formatted_cell_value(2, 2, 1).unwrap(),
        "",
        "no row of the source is in the prepared workbook"
    );
    let stored = fill::open_stored(&prepared.to_bytes()).unwrap();
    let mut filled = fill::fill(stored, &[orders(3)]).unwrap();
    filled.evaluate();
    let text = |row| filled.get_formatted_cell_value(0, row, 1).unwrap();
    assert_eq!(text(1), "60", "the data sheet's table, by its column");
    assert_eq!(text(2), "3", "the data sheet by its name");
    assert_eq!(text(3), "42", "the author's own sheet");
    // filled again with more rows, the table's range stretches as a saved document's does
    let mut more = fill::fill(fill::open_stored(&prepared.to_bytes()).unwrap(), &[orders(5)]).unwrap();
    more.evaluate();
    assert_eq!(more.get_formatted_cell_value(0, 1, 1).unwrap(), "150");
}

#[test]
fn a_sheet_table_or_name_of_the_data_sheets_is_refused() {
    let data = ["Orders".to_string()];
    // a sheet of the data sheet's name, whatever its case: its rows would stand below the data
    let mut model = uploaded();
    model.add_sheet("orders").unwrap();
    let refused = overlay_refusal(&model, &data).unwrap_or_default();
    assert!(refused.contains("sheet named orders"), "{refused}");
    // a table named as the data sheet's, on another sheet
    let mut model = fill::fill(fill::new_model("Summary").unwrap(), &[orders(2)]).unwrap();
    model.rename_sheet_by_index(1, "Old orders").unwrap();
    let mut model = Model::from_bytes(&model.to_bytes(), "en").unwrap();
    assert!(model.workbook.tables.values().any(|table| table.name == "Orders"));
    let refused = overlay_refusal(&model, &data).unwrap_or_default();
    assert!(refused.contains("table named Orders"), "{refused}");
    // a defined name of it
    model.workbook.tables.clear();
    let mut model = Model::from_workbook(model.workbook, "en").unwrap();
    model
        .new_defined_name(DefinedNameEntry::new("ORDERS", "Summary!$A$1"))
        .unwrap();
    let refused = overlay_refusal(&model, &data).unwrap_or_default();
    assert!(refused.contains("name ORDERS"), "{refused}");
    // a table name that is the sheet's with its spaces made _
    let sheets = ["Open orders".to_string()];
    let mut model = fill::fill(
        fill::new_model("Summary").unwrap(),
        &[SheetRows {
            name: "Open orders".into(),
            ..orders(1)
        }],
    )
    .unwrap();
    model.rename_sheet_by_index(1, "Kept").unwrap();
    let model = Model::from_bytes(&model.to_bytes(), "en").unwrap();
    assert!(overlay_refusal(&model, &sheets).is_some_and(|refused| refused.contains("Open_orders")));
}

/// A tab that only looks like the data sheet's (a space after it, a zero-width, bidirectional or other ignorable mark
/// in it, a letter written decomposed, another case of a letter beyond ASCII) is the data sheet's name too.
#[test]
fn a_sheet_that_only_looks_like_the_data_sheets_is_refused() {
    for lookalike in [
        "Orders ",
        " Orders",
        "Ord\u{200b}ers",
        "\u{202e}Orders",
        "Orders\u{feff}",
        "Or\u{34f}ders",
        "Orders\u{3164}",
        "Ord\u{fe0f}ers",
        "Orders\u{e0041}",
        "Orders\u{1d173}",
        "Ord\u{180b}ers",
    ] {
        let mut model = uploaded();
        model.add_sheet(lookalike).unwrap();
        let refused = overlay_refusal(&model, &["Orders".to_string()]).unwrap_or_default();
        assert!(
            refused.contains("which is the data sheet's name"),
            "{lookalike:?}: {refused}"
        );
    }
    // a letter with its accent apart (NFD) is the same letter composed
    let mut model = uploaded();
    model.add_sheet("Cafe\u{301}").unwrap();
    assert!(
        overlay_refusal(&model, &["Caf\u{e9}".to_string()]).is_some(),
        "compared composed"
    );
    // and with an unseen joiner or variation selector between the letter and its accent
    for between in ["\u{34f}", "\u{fe0f}"] {
        let mut model = uploaded();
        model.add_sheet(&format!("Cafe{between}\u{301}")).unwrap();
        assert!(
            overlay_refusal(&model, &["Caf\u{e9}".to_string()]).is_some(),
            "{between:?} between a letter and its accent"
        );
    }
    let mut model = uploaded();
    model.add_sheet("ÉTÉ").unwrap();
    assert!(
        overlay_refusal(&model, &["été".to_string()]).is_some(),
        "compared in capitals, as the engine does"
    );
    assert_eq!(overlay_refusal(&uploaded(), &["Orders".to_string()]), None);
}

/// A saved document's stored overlay as CalcGrid writes it: a Summary over the Orders data sheet's stub.
fn stored() -> Model<'static> {
    let mut model = fill::fill(fill::new_model("Summary").unwrap(), &[orders(3)]).unwrap();
    model
        .set_user_input(0, 1, 1, "=SUM(Orders[amount])".to_string())
        .unwrap();
    model
        .set_user_input(0, 2, 1, "=COUNTA(Orders!A:A)-1".to_string())
        .unwrap();
    model.evaluate();
    fill::open_stored(&fill::stored_form(&model.to_bytes(), &[("Orders".to_string(), 3)]).unwrap()).unwrap()
}

/// A saved overlay written without CalcGrid (the definition editor's JSON, an import, a git pull) is held to what a
/// data sheet may be when it opens: rows it planted in a data sheet are cut, and a table or name of the data sheet's
/// table name elsewhere refuses it; one CalcGrid wrote opens as it is.
#[test]
fn a_saved_overlay_brings_no_row_into_a_data_sheet() {
    let data = ["Orders".to_string()];
    let filled = |model: Model<'static>| {
        let mut filled = fill::fill(model, &[orders(3)]).unwrap();
        filled.evaluate();
        let text = |row| filled.get_formatted_cell_value(0, row, 1).unwrap();
        (text(1), text(2))
    };
    // as CalcGrid wrote it
    let model = saved_overlay(stored(), &data).unwrap();
    assert_eq!(filled(model), ("60".to_string(), "3".to_string()));
    // rows planted below the stub, and in it: cut, so only the source's rows are read
    let mut planted = stored();
    for (row, amount) in [(2, "1000"), (3, "1000"), (40, "5000")] {
        planted.set_user_input(1, row, 1, "Planted".to_string()).unwrap();
        planted.set_user_input(1, row, 2, amount.to_string()).unwrap();
    }
    planted.evaluate();
    let model = saved_overlay(planted, &data).unwrap();
    assert_eq!(model.get_formatted_cell_value(1, 40, 2).unwrap(), "");
    assert_eq!(filled(model), ("60".to_string(), "3".to_string()));
    // the data sheet's own table is kept; one of its name on another sheet, or a defined name of it, is refused
    let mut table = stored().workbook;
    table
        .tables
        .get_mut("Orders")
        .expect("the data sheet's table")
        .sheet_name = "Summary".to_string();
    let table = Model::from_workbook(table, "en").unwrap();
    let refused = saved_overlay(table, &data).map(|_| ()).unwrap_err();
    assert!(
        refused.message().contains("table named Orders"),
        "{}",
        refused.message()
    );
    let mut named = stored();
    named
        .new_defined_name(DefinedNameEntry::new("orders", "Summary!$A$1"))
        .unwrap();
    let refused = saved_overlay(named, &data).map(|_| ()).unwrap_err();
    assert!(refused.message().contains("name orders"), "{}", refused.message());
    // a tab that only looks like the data sheet's is renamed, so the document still opens, and a formula over it follows
    let mut lookalike = stored();
    lookalike.add_sheet("Orders\u{200b}").unwrap();
    lookalike.set_user_input(2, 1, 1, "7".to_string()).unwrap();
    lookalike
        .set_user_input(0, 3, 1, "='Orders\u{200b}'!A1*2".to_string())
        .unwrap();
    lookalike.evaluate();
    let mut opened = saved_overlay(lookalike, &data).unwrap();
    assert_eq!(
        opened.workbook.get_worksheet_names(),
        ["Summary", "Orders", "Orders (renamed)"]
    );
    opened.evaluate();
    assert_eq!(opened.get_formatted_cell_value(0, 3, 1).unwrap(), "14");
}

/// What a forged overlay may draw over a data sheet's rows or hide of them is taken off when it opens; its widths and
/// frozen panes, an author's to set, stay.
#[test]
fn a_saved_overlay_draws_nothing_over_a_data_sheet() {
    use moqui_core::workbook::ironcalc::base::types::{Comment, MergedCell, SheetState};
    let data = ["Orders".to_string()];
    let mut model = stored();
    model.set_column_width(1, 2, 140.0).unwrap();
    let mut workbook = model.workbook;
    let sheet = &mut workbook.worksheets[1];
    sheet.merged_cells.push(MergedCell {
        row: 2,
        column: 1,
        width: 2,
        height: 2,
    });
    sheet.comments.push(Comment {
        text: "not from the source".into(),
        author_name: "forger".into(),
        author_id: None,
        cell_ref: "A2".into(),
    });
    sheet.state = SheetState::Hidden;
    for row in &mut sheet.rows {
        row.hidden = true;
    }
    let forged = Model::from_workbook(workbook, "en").unwrap();
    let opened = saved_overlay(forged, &data).unwrap();
    let sheet = &opened.workbook.worksheets[1];
    assert!(sheet.merged_cells.is_empty());
    assert!(sheet.comments.is_empty());
    assert!(matches!(sheet.state, SheetState::Visible));
    assert!(sheet.rows.iter().all(|row| !row.hidden));
    assert_eq!(opened.get_column_width(1, 2).unwrap(), 140.0, "a width stays");
    assert_eq!(sheet.frozen_rows, 1, "the frozen header stays");
    // a width or a height that hides a column or a row is not kept
    let mut model = stored();
    model.set_column_width(1, 1, 0.0).unwrap();
    model.set_row_height(1, 1, 0.0).unwrap();
    let opened = saved_overlay(model, &data).unwrap();
    assert!(opened.get_column_width(1, 1).unwrap() >= MIN_DATA_WIDTH);
    assert!(opened.get_row_height(1, 1).unwrap() >= MIN_DATA_HEIGHT);
}

/// A tab named like a data sheet whose name is long (up to the engine's 31 characters) still gets a name the engine
/// takes, so the document opens.
#[test]
fn a_lookalike_of_a_long_data_sheet_is_renamed_within_the_engines_limit() {
    for long in ["FinancialAccountTransaction", "ThirtyOneCharactersOfDataSheetX"] {
        let data = [long.to_string()];
        let rows = SheetRows {
            name: long.to_string(),
            ..orders(1)
        };
        let model = fill::fill(fill::new_model("Summary").unwrap(), &[rows]).unwrap();
        let mut model =
            fill::open_stored(&fill::stored_form(&model.to_bytes(), &[(long.to_string(), 1)]).unwrap()).unwrap();
        let lookalike: String = format!("{}\u{200b}", &long[..long.len() - 1]) + &long[long.len() - 1..];
        // as a forged body holds it: past the engine's own check of a name typed
        model.add_sheet("Placeholder").unwrap();
        let mut workbook = model.workbook;
        workbook.worksheets[2].name = lookalike;
        let model = Model::from_workbook(workbook, "en").unwrap();
        let opened = saved_overlay(model, &data).unwrap();
        let names = opened.workbook.get_worksheet_names();
        assert_eq!(names.len(), 3, "{names:?}");
        let renamed = &names[2];
        assert!(renamed.chars().count() <= 31, "{renamed}");
        assert!(renamed.ends_with("(renamed)"), "{renamed}");
        assert!(!crate::upload::same(renamed, long));
    }
}

/// A data cell's look is not the overlay's style 0, which a forged body or an upload could make hide every value.
#[test]
fn a_data_cell_is_drawn_in_the_plain_style_not_the_workbooks_first() {
    let mut workbook = stored().workbook;
    workbook
        .styles
        .num_fmts
        .push(moqui_core::workbook::ironcalc::base::types::NumFmt {
            num_fmt_id: 200,
            format_code: ";;;".to_string(),
        });
    workbook.styles.cell_xfs[0].num_fmt_id = 200;
    let forged = Model::from_workbook(workbook, "en").unwrap();
    let mut filled = fill::fill(forged, &[orders(2)]).unwrap();
    filled.evaluate();
    assert_eq!(filled.get_formatted_cell_value(1, 2, 1).unwrap(), "Customer 1");
    assert_eq!(filled.get_formatted_cell_value(1, 2, 2).unwrap(), "10");
}

/// A date is written in the workbook's own date system: an Excel for Mac workbook (1904) shows the same day.
#[test]
fn a_data_date_shows_the_same_day_in_a_1904_workbook() {
    use moqui_core::workbook::ironcalc::base::types::DateSystem;
    let dated = SheetRows {
        name: "Orders".to_string(),
        columns: vec!["placed".to_string()],
        // 2026-11-28, counted from 1899-12-30
        rows: vec![vec![CellData::Date(46_354.0)]],
        truncated: false,
    };
    for system in [DateSystem::IronCalc, DateSystem::Excel1904] {
        let mut model = fill::new_model("Summary").unwrap();
        model.set_date_system(system);
        let mut filled = fill::fill(model, std::slice::from_ref(&dated)).unwrap();
        filled.evaluate();
        assert_eq!(
            filled.get_formatted_cell_value(1, 2, 1).unwrap(),
            "2026-11-28",
            "{system:?}"
        );
    }
}

/// A workbook's own number formats cannot stand for the plain or date styles of the data cells: a format given to a
/// built-in id (`general`'s 0) and an id given twice are not kept, and the fill's styles stay found, so a refill
/// adds none.
#[test]
fn a_planted_number_format_hides_no_data_cell() {
    use moqui_core::workbook::ironcalc::base::types::NumFmt;
    let dated = |rows: usize| SheetRows {
        name: "Orders".to_string(),
        columns: vec!["customer".to_string(), "placed".to_string()],
        rows: (0..rows)
            .map(|_| vec![CellData::Text("Customer 1".into()), CellData::Date(46_354.0)])
            .collect(),
        truncated: false,
    };
    let planted = |formats: Vec<NumFmt>| {
        let mut workbook = fill::new_model("Summary").unwrap().workbook;
        workbook.styles.num_fmts.extend(formats);
        Model::from_workbook(workbook, "en").unwrap()
    };
    let fmt = |num_fmt_id: i32, code: &str| NumFmt {
        num_fmt_id,
        format_code: code.to_string(),
    };
    for formats in [vec![fmt(0, ";;;")], vec![fmt(50, ";;;"), fmt(50, "yyyy-mm-dd")]] {
        let mut filled = fill::fill(planted(formats.clone()), &[dated(1)]).unwrap();
        filled.evaluate();
        let text = |row, column| filled.get_formatted_cell_value(1, row, column).unwrap();
        assert_eq!(text(1, 1), "customer", "{formats:?}");
        assert_eq!(text(2, 1), "Customer 1", "{formats:?}");
        assert_eq!(text(2, 2), "2026-11-28", "{formats:?}");
        // the stored form filled again finds the same styles: no style is added
        let styles = filled.workbook.styles.cell_xfs.len();
        let again =
            fill::open_stored(&fill::stored_form(&filled.to_bytes(), &[("Orders".to_string(), 1)]).unwrap()).unwrap();
        let again = fill::fill(again, &[dated(1)]).unwrap();
        assert_eq!(again.workbook.styles.cell_xfs.len(), styles, "{formats:?}");
    }
}

/// However many of the names a rename would take a forged body holds, a lookalike tab is renamed, and a fill whose
/// data sheet the engine takes for another sheet's name (a capital beyond ASCII) renames that one aside and opens.
#[test]
fn a_lookalike_is_always_renamed_and_a_clash_never_stops_a_fill() {
    let data = ["Orders".to_string()];
    let mut model = stored();
    let mut taken = vec!["Orders (renamed)".to_string()];
    taken.extend((2..=20).map(|n| format!("Orders (renamed {n})")));
    taken.extend((1..=20).map(|n| format!("Renamed {n}")));
    for name in &taken {
        model.add_sheet(name).unwrap();
    }
    model.add_sheet("Orders\u{200b}").unwrap();
    let opened = saved_overlay(model, &data).unwrap();
    let names = opened.workbook.get_worksheet_names();
    assert!(!names.iter().any(|name| name == "Orders\u{200b}"), "{names:?}");
    assert!(names.iter().any(|name| name == "Renamed 21"), "{names:?}");

    let mut model = fill::new_model("Summary").unwrap();
    model.add_sheet("ÉTÉ").unwrap();
    let rows = SheetRows {
        name: "été".to_string(),
        ..orders(2)
    };
    let filled = fill::fill(model, &[rows]).unwrap();
    let names = filled.workbook.get_worksheet_names();
    assert!(names.iter().any(|name| name == "été"), "{names:?}");
    assert!(names.iter().any(|name| name.starts_with("Renamed")), "{names:?}");
}

/// A data sheet's width or height that is not a number, or so large it pushes the data past it out of reach, is not
/// kept.
#[test]
fn a_saved_size_that_is_not_a_number_or_too_large_is_not_kept() {
    let data = ["Orders".to_string()];
    let mut model = stored();
    model.set_column_width(1, 1, 120.0).unwrap();
    model.set_row_height(1, 1, 30.0).unwrap();
    let mut workbook = model.workbook;
    for col in &mut workbook.worksheets[1].cols {
        col.width = f64::NAN;
    }
    for row in &mut workbook.worksheets[1].rows {
        row.height = 1e9;
    }
    let opened = saved_overlay(Model::from_workbook(workbook, "en").unwrap(), &data).unwrap();
    let width = opened.get_column_width(1, 1).unwrap();
    let height = opened.get_row_height(1, 1).unwrap();
    assert!((MIN_DATA_WIDTH..=MAX_DATA_WIDTH).contains(&width), "{width}");
    assert!((MIN_DATA_HEIGHT..=MAX_DATA_HEIGHT).contains(&height), "{height}");
    assert_eq!(data_size(f64::NAN, MIN_DATA_WIDTH, MAX_DATA_WIDTH, 90.0), 90.0);
    assert_eq!(data_size(1e9, MIN_DATA_HEIGHT, MAX_DATA_HEIGHT, 25.0), MAX_DATA_HEIGHT);
}
