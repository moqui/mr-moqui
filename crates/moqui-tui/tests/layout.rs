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

//! Canvas outlines at the widths of section 6 (72, 100, 120, 153, 180, 240), the frame, and the layout rules of 1.3.

use std::collections::BTreeMap;

use moqui_tui::{
    dotted_label, frame, frame_with, label_column_width, outline, Breakpoints, ScreenModel, WidthClass,
    GALLERY_DETAIL_JSON, GALLERY_LIST_JSON,
};

fn model(json: &str) -> ScreenModel {
    ScreenModel::from_json(json).unwrap_or_else(|err| panic!("{err}: {json}"))
}

fn screen(nodes: &str) -> ScreenModel {
    model(&format!(
        r#"{{"v":1,"screen":{{"path":"/t","title":"T"}},"c":[{nodes}]}}"#
    ))
}

#[test]
fn gallery_list_outline() {
    assert_eq!(
        outline(&model(GALLERY_LIST_JSON), 100),
        "\
class=sm canvas=100
form EnumFind
  field Type beside label=11 value=21
  field Description beside label=11 value=32
  buttons find
list EnumList scroll=false rows=20
  col Identifier w=10
  col Description w=40
  col Code w=15
  col Note w=11
inline openDetail
"
    );
    let narrow = outline(&model(GALLERY_LIST_JSON), 72);
    assert!(narrow.starts_with("class=xs canvas=72\n"), "{narrow}");
    assert!(narrow.contains("field Type above label=0"), "{narrow}");
    assert!(narrow.contains("list EnumList scroll=true"), "{narrow}");
    for width in [120, 153, 180, 240] {
        let wide = outline(&model(GALLERY_LIST_JSON), width);
        assert!(wide.contains("list EnumList scroll=false"), "{wide}");
    }
    assert!(outline(&model(GALLERY_LIST_JSON), 240).starts_with("class=xl"));
}

#[test]
fn detail_boxes_sit_side_by_side_from_md_and_stack_below() {
    let detail = model(GALLERY_DETAIL_JSON);
    let wide = outline(&detail, 153);
    assert!(wide.contains("row\ncol x=0 w=76\n  box Record w=76"), "{wide}");
    assert!(wide.contains("col x=77 w=76\n  box Type w=76"), "{wide}");
    // Labels pad to the longest title of their box, capped at 24.
    assert!(wide.contains("field Enum ID beside label=24"), "{wide}");
    assert!(wide.contains("field Code beside label=6"), "{wide}");
    let at_120 = outline(&detail, 120);
    assert!(at_120.contains("col x=60 w=60"), "{at_120}");
    let narrow = outline(&detail, 100);
    assert!(narrow.starts_with("class=sm canvas=100\nbox Record w=100"), "{narrow}");
    assert!(!narrow.contains("row\n"), "{narrow}");
    assert!(narrow.contains("label=18"), "{narrow}");
}

#[test]
fn the_drawer_docks_while_the_canvas_left_is_still_md() {
    assert_eq!((frame(100).drawer, frame(100).class), (0, WidthClass::Sm));
    let wide = frame(180);
    assert_eq!((wide.drawer, wide.canvas, wide.class), (26, 154, WidthClass::Md));
    assert_eq!(frame(146).drawer, 26);
    assert_eq!(frame(146).class, WidthClass::Md);
    assert_eq!(frame(145).drawer, 0);
    // F9 shows it at any width, or hides it.
    let bp = Breakpoints::default();
    assert_eq!(frame_with(100, &bp, Some(true)).drawer, 26);
    assert_eq!(frame_with(180, &bp, Some(false)).drawer, 0);
    // A theme's breakpoints move the line.
    let (themed, skipped) = Breakpoints::with(&BTreeMap::from([("md".to_string(), 100), ("drawer".to_string(), 30)]));
    assert!(skipped.is_empty());
    assert_eq!(frame_with(130, &themed, None).drawer, 30);
    assert_eq!(frame_with(130, &themed, None).class, WidthClass::Md);
}

#[test]
fn labels_pad_with_spaced_dot_leaders_and_sit_above_on_xs() {
    assert_eq!(label_column_width(&["Type", "Description"], WidthClass::Sm), 11);
    assert_eq!(label_column_width(&["Type", "Description"], WidthClass::Xs), 0);
    assert_eq!(dotted_label("Type", 11), "Type . . . ");
    assert_eq!(dotted_label("Order ID", 11), "Order ID . ");
    assert_eq!(dotted_label("Type", 0), "Type");
}

#[test]
fn row_columns_past_twelve_spans_wrap_and_a_narrow_column_takes_its_own_line() {
    let cols: Vec<String> = (0..4)
        .map(|index| {
            format!(
                r#"{{"t":"col","id":"c{index}","md":"6","c":[{{"t":"text","id":"t{index}","text":"Col{index}"}}]}}"#
            )
        })
        .collect();
    let grid = screen(&format!(r#"{{"t":"row","id":"r","c":[{}]}}"#, cols.join(",")));
    assert_eq!(
        outline(&grid, 154),
        "\
class=md canvas=154
row
col x=0 w=76
  text Col0
col x=77 w=76
  text Col1
row
col x=0 w=76
  text Col2
col x=77 w=76
  text Col3
"
    );
    // A 2-of-12 column at 154 is 25 wide, under the 40 minimum: the others stay a row, and it follows on its own line.
    let narrow = screen(
        r#"{"t":"row","id":"r","c":[
            {"t":"col","id":"a","md":"5","c":[{"t":"text","id":"a1","text":"A"}]},
            {"t":"col","id":"b","md":"2","c":[{"t":"text","id":"b1","text":"B"}]},
            {"t":"col","id":"c","md":"5","c":[{"t":"text","id":"c1","text":"C"}]}]}"#,
    );
    assert_eq!(
        outline(&narrow, 154),
        "class=md canvas=154\nrow\ncol x=0 w=63\n  text A\ncol x=64 w=63\n  text C\ntext B\n"
    );
}

#[test]
fn field_row_cells_stay_side_by_side_while_each_has_its_label_and_24_columns() {
    let form = screen(
        r#"{"t":"form","id":"F","name":"F","c":[{"t":"row","id":"F.l0","c":[
            {"t":"field","id":"F.a","name":"a","title":"First name of person","w":{"kind":"textLine","value":""}},
            {"t":"field","id":"F.b","name":"b","title":"Second","w":{"kind":"textLine","value":""}}]}]}"#,
    );
    // Each cell needs its label (20), the ` : ` and 24 columns: 47, so two fit at 120 and not at 90.
    let wide = outline(&form, 120);
    assert!(wide.contains("cells First name of person@0 Second@60"), "{wide}");
    let narrow = outline(&form, 90);
    assert!(narrow.contains("field First name of person beside"), "{narrow}");
    assert!(narrow.contains("field Second beside"), "{narrow}");
}

#[test]
fn a_panel_and_a_split_stack_below_md() {
    let nodes = r#"{"t":"panel","id":"P","c":[
            {"t":"region","id":"P.left","name":"left","size":200,"c":[{"t":"text","id":"l","text":"side"}]},
            {"t":"region","id":"P.center","name":"center","c":[{"t":"text","id":"m","text":"main"}]}]},
        {"t":"split","id":"S","sideWidth":240,"c":[
            {"t":"region","id":"S.side","name":"side","c":[{"t":"text","id":"s","text":"list"}]},
            {"t":"region","id":"S.main","name":"main","c":[{"t":"text","id":"r","text":"record"}]}]}"#;
    let wide = outline(&screen(nodes), 160);
    assert!(wide.contains("col x=0 w=25\n  text side"), "{wide}");
    assert!(wide.contains("col x=0 w=30\n  text list"), "{wide}");
    assert!(wide.contains("col x=31 w=129\n  text record"), "{wide}");
    let narrow = outline(&screen(nodes), 90);
    assert_eq!(
        narrow,
        "class=sm canvas=90\ntext side\ntext main\ntext list\ntext record\n"
    );
}

#[test]
fn a_box_toolbar_and_buttons_are_their_own_lines() {
    let nodes = r#"{"t":"box","id":"B","title":"Order","c":[
        {"t":"region","id":"B.toolbar","name":"toolbar","c":[{"t":"link","id":"ship","text":"Ship","url":"/ship"}]},
        {"t":"text","id":"b","text":"body"}]}"#;
    let text = outline(&screen(nodes), 100);
    assert_eq!(
        text,
        "class=sm canvas=100\nbox Order w=100\n  toolbar ship\n  text body\n"
    );
}

/// A run of buttons wider than the canvas wraps onto more lines, as many to a line as fit (AppList at 120 columns).
#[test]
fn a_row_of_buttons_wraps() {
    let links: Vec<String> = (0..8)
        .map(|n| {
            format!(r#"{{"t":"link","id":"app{n}","text":"Application Number {n}","kind":"anchor-button","url":"/apps/a{n}"}}"#)
        })
        .collect();
    let model = screen(&links.join(","));
    let wide = outline(&model, 240);
    assert_eq!(wide.matches("inline ").count(), 1, "{wide}");
    // Each button is `[ Application Number n ]`, 26 columns, 28 with the gap: four to a line at 120.
    let narrow = outline(&model, 120);
    assert_eq!(narrow.matches("inline ").count(), 2, "{narrow}");
    assert!(narrow.contains("inline app0 app1 app2 app3\ninline app4"), "{narrow}");
}
