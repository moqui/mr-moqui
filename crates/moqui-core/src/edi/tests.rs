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

use moqui_lib::object::Value;

use super::*;

/// `mantle-edi` `X12Envelope.groovy`, with its `[:]` maps written as objects.
const X12_ENVELOPE: &str = r"[
    {ID:'ISA', MIN:0, MAX:99999,
        LEVEL: [
            {ID:'TA1', MIN:0, MAX:99999},
            {ID:'GS', MIN:0, MAX:99999, LEVEL: [
                    {ID:'ST', MIN:0, MAX:99999},
                    {ID:'GE', MIN:1, MAX:1},
                ]
            },
            {ID:'IEA', MIN:1, MAX:1}
        ]
    }
]";

/// A cut-down purchase order body.
const X12_850: &str = r"[
    {ID:'ST', MIN:1, MAX:1, LEVEL:[
        {ID:'BEG', MIN:1, MAX:1},
        {ID:'N1', MIN:0, MAX:200, LEVEL:[
            {ID:'N3', MIN:0, MAX:2},
        ]},
        {ID:'PO1', MIN:1, MAX:100000},
        {ID:'SE', MIN:1, MAX:1},
    ]}
]";

/// The fixed-width ISA segment, padded with the handler's padding character.
fn isa(control: &str) -> String {
    let pad = |text: &str, width: usize| {
        let mut out = text.to_string();
        while out.chars().count() < width {
            out.push('\u{a0}');
        }
        out
    };
    [
        "ISA".to_string(),
        "00".to_string(),
        pad("", 10),
        "00".to_string(),
        pad("", 10),
        "ZZ".to_string(),
        pad("SENDER", 15),
        "ZZ".to_string(),
        pad("RECEIVER", 15),
        "231015".to_string(),
        "1200".to_string(),
        "U".to_string(),
        "00401".to_string(),
        control.to_string(),
        "0".to_string(),
        "P".to_string(),
        ":".to_string(),
    ]
    .join("*")
}

fn interchange(control: &str, body: &[&str]) -> String {
    let mut segments = vec![
        isa(control),
        "GS*PO*SENDER*RECEIVER*20231015*1200*1*X*004010".to_string(),
    ];
    segments.extend(body.iter().map(|segment| segment.to_string()));
    segments.push("GE*1*1".to_string());
    segments.push(format!("IEA*1*{control}"));
    segments.iter().map(|segment| format!("{segment}~\n")).collect()
}

const PO_BODY: [&str; 7] = [
    "ST*850*0001",
    "BEG*00*SA*PO1**20231015",
    "N1*ST*Smith?*Jones",
    "N3*1 Main St",
    "PO1*1*10*EA*9.95**BP:ABC123",
    "PO1*2*1*EA*100",
    "SE*7*0001",
];

fn structure(source: &str) -> Value {
    crate::script::eval_value(source).unwrap()
}

fn handler(body: bool) -> EdiHandler {
    let mut handler = EdiHandler::new();
    handler.set_envelope(&structure(X12_ENVELOPE)).unwrap();
    if body {
        handler.set_body(&structure(X12_850)).unwrap();
    }
    handler
}

fn children<'a>(map: &'a EdiMap, id: &str) -> &'a [Value] {
    match map.get(id) {
        Some(Value::List(items)) => items,
        _ => &[],
    }
}

fn child<'a>(map: &'a EdiMap, id: &str, index: usize) -> &'a EdiMap {
    match &children(map, id)[index] {
        Value::Map(segment) => segment,
        other => panic!("{id} is not a segment: {other:?}"),
    }
}

fn elements(segment: &EdiMap) -> Vec<Value> {
    match segment.get("elements") {
        Some(Value::List(items)) => items.clone(),
        _ => Vec::new(),
    }
}

fn s(text: &str) -> Value {
    Value::string(text)
}

#[test]
fn x12_parse_and_generate_round_trip() {
    let text = interchange("000000001", &PO_BODY);
    let mut edi = handler(true);
    let root = edi.parse_text(text.trim()).unwrap();
    assert_eq!(edi.segment_errors(), Some(&[][..]));
    assert_eq!(edi.segment_terminator, Some('~'));
    assert_eq!(edi.element_separator, Some('*'));
    assert_eq!(edi.component_delimiter, Some(':'));

    let isa = child(&root, "ISA", 0);
    // Padding is read as spaces and trimmed.
    assert_eq!(elements(isa)[6], s("SENDER"));
    assert_eq!(elements(isa)[2], s(""));
    let st = child(child(isa, "GS", 0), "ST", 0);
    assert_eq!(elements(child(st, "BEG", 0))[4], s(""));
    let n1 = child(st, "N1", 0);
    assert_eq!(elements(n1), vec![s("N1"), s("ST"), s("Smith*Jones")]);
    assert_eq!(elements(child(n1, "N3", 0)), vec![s("N3"), s("1 Main St")]);
    assert_eq!(children(st, "PO1").len(), 2);
    assert_eq!(
        elements(child(st, "PO1", 0))[6],
        Value::List(vec![s("BP"), s("ABC123")])
    );
    // BEG, N1, N3, two PO1, and SE are under ST.
    assert_eq!(edi.count_segments(st), 6);
    assert_eq!(
        child(isa, "IEA", 0).get("elements"),
        Some(&Value::List(vec![s("IEA"), s("1"), s("000000001")]))
    );

    let generated = edi.generate_text(&root).unwrap();
    assert_eq!(generated, text);
    assert_eq!(edi.parse_text(generated.trim()).unwrap(), root);
}

#[test]
fn x12_structure_files_load_by_location() {
    let dir = std::env::temp_dir().join(format!("moqui-edi-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let envelope = dir.join("X12Envelope.js");
    let body = dir.join("X12V4010TS850.js");
    std::fs::write(&envelope, X12_ENVELOPE).unwrap();
    std::fs::write(&body, X12_850).unwrap();
    let mut edi = EdiHandler::new();
    edi.load_envelope(envelope.to_str().unwrap())
        .unwrap()
        .load_body(body.to_str().unwrap())
        .unwrap();
    let root = edi.parse_text(interchange("000000002", &PO_BODY).trim()).unwrap();
    let st = child(child(child(&root, "ISA", 0), "GS", 0), "ST", 0);
    assert_eq!(elements(child(st, "SE", 0)), vec![s("SE"), s("7"), s("0001")]);
    assert!(edi.load_envelope(dir.join("missing.js").to_str().unwrap()).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn envelope_only_keeps_body_segments_as_text() {
    let mut edi = handler(false);
    let root = edi.parse_text(interchange("000000003", &PO_BODY).trim()).unwrap();
    let gs = child(child(&root, "ISA", 0), "GS", 0);
    assert_eq!(children(gs, "ST").len(), 1);
    let original: Vec<String> = children(gs, "originalList").iter().map(to_plain_string).collect();
    assert_eq!(original, PO_BODY[1..].to_vec());
    let generated = edi.generate_text(&root).unwrap();
    // The original segments are written back with the terminator and no suffix.
    assert!(
        generated.contains("ST*850*0001~\nBEG*00*SA*PO1**20231015~N1*ST*Smith?*Jones~N3"),
        "{generated}"
    );
}

#[test]
fn undefined_segment_is_a_segment_error_with_ak3() {
    let body = ["ST*850*0001", "BEG*00*SA*PO1**20231015", "ZZZ*odd", "SE*4*0001"];
    let mut edi = handler(true);
    edi.parse_text(interchange("000000004", &body).trim()).unwrap();
    let errors = edi.segment_errors().unwrap().to_vec();
    assert_eq!(errors.len(), 1);
    let error = &errors[0];
    assert_eq!(error.error_type, SegmentErrorType::NotDefinedInTxSet);
    assert_eq!(error.segment_id, "ZZZ");
    assert_eq!(error.segment_index, 4);
    assert_eq!(error.position_in_tx_set, 2);
    assert_eq!(
        error.make_ak3().get("elements"),
        Some(&Value::List(vec![s("AK3"), s("ZZZ"), s("2"), s(""), s("6")]))
    );
    let back = SegmentError::from_value(&error.to_value()).unwrap();
    assert_eq!(&back, error);

    let with_elements = SegmentError {
        element_errors: vec![ElementError {
            error_type: ElementErrorType::InvalidCode,
            element_position: 3,
            composite_position: Some(2),
            element_text: "XX".to_string(),
        }],
        ..error.clone()
    };
    let ak3 = with_elements.make_ak3();
    let Some(Value::List(ak4)) = ak3.get("AK4") else {
        panic!("no AK4");
    };
    let Value::Map(ak4) = &ak4[0] else {
        panic!("AK4 is not a segment");
    };
    assert_eq!(
        elements(ak4),
        vec![s("AK4"), Value::List(vec![s("3"), s("2")]), s("7"), s("XX")]
    );
}

#[test]
fn split_message_by_interchange() {
    let first = interchange("000000005", &PO_BODY);
    let second = interchange("000000006", &PO_BODY);
    let mut edi = handler(false);
    let splits = edi
        .split_message(Some("ISA"), Some("IEA"), format!("{first}{second}").trim())
        .unwrap();
    assert_eq!(splits, vec![first.clone(), second]);
    // A header with no trailer before it starts a new split. Text after the last trailer is dropped.
    let no_trailer = first.replace("IEA*1*000000005~\n", "");
    let splits = edi
        .split_message(
            Some("ISA"),
            Some("IEA"),
            format!("{no_trailer}{first}GS*extra").as_str(),
        )
        .unwrap();
    assert_eq!(splits, vec![no_trailer, first]);
}

#[test]
fn edifact_separators_from_una_and_release_character() {
    let envelope = structure(r"[{ID:'UNB', LEVEL:[{ID:'UNH', LEVEL:[{ID:'BGM'}, {ID:'UNT'}]}, {ID:'UNZ'}]}]");
    let mut edi = EdiHandler::new();
    edi.set_envelope(&envelope).unwrap();
    let text = "UNA:+.? 'UNB+UNOC:3+SENDER+RECEIVER+231015:1200+1'UNH+1+ORDERS:D:96A:UN'\
        BGM+220+PO?+1+9'UNT+3+1'UNZ+1+1'";
    let root = edi.parse_text(text).unwrap();
    assert_eq!(edi.segment_terminator, Some('\''));
    assert_eq!(edi.element_separator, Some('+'));
    assert_eq!(edi.component_delimiter, Some(':'));
    // Java reads the UNA segment ID as "UNA:", so it is kept as text.
    assert_eq!(children(&root, "originalList"), &[s("UNA:+.?")]);
    let unh = child(child(&root, "UNB", 0), "UNH", 0);
    assert_eq!(
        elements(unh)[2],
        Value::List(vec![s("ORDERS"), s("D"), s("96A"), s("UN")])
    );
    assert_eq!(
        elements(child(unh, "BGM", 0)),
        vec![s("BGM"), s("220"), s("PO+1"), s("9")]
    );

    let mut generator = EdiHandler::new();
    generator.set_edifact_default_chars();
    generator.segment_suffix = None;
    let mut segment = EdiMap::new();
    segment.insert("elements".to_string(), Value::List(vec![s("FTX"), s("a+b:c'd?")]));
    let mut message = EdiMap::new();
    message.insert("FTX".to_string(), Value::List(vec![Value::Map(segment)]));
    assert_eq!(generator.generate_text(&message).unwrap(), "FTX+a?+b?:c?'d??'");
}

#[test]
fn parse_needs_an_envelope_and_segments() {
    let mut edi = EdiHandler::new();
    assert!(edi.parse_text("ISA*00").is_err());
    let mut edi = handler(false);
    assert!(edi.parse_text("").is_err());
    edi.set_x12_default_chars();
    assert!(edi
        .parse_text("ISA*00")
        .unwrap_err()
        .message()
        .contains("No segments found"));
    assert!(EdiHandler::new().generate_text(&EdiMap::new()).is_err());
    assert_eq!(java_split("a~b?~c~~", '~', '?'), vec!["a", "b?~c"]);
    assert_eq!(java_split("~a", '~', '?'), vec!["", "a"]);
    assert_eq!(java_split("", '~', '?'), vec![""]);
}

/// The six grammar files of mantle-edi, as `moqui convert --groovy` writes them (moqui-devkit's groovy_convert tests
/// check the conversion), load in the handler, and the 850 parses the purchase order the cut-down fixture parses.
#[test]
fn the_converted_mantle_grammars_load_and_parse() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/edi/testdata/mantle");
    for name in [
        "X12Envelope",
        "X12V4010TS810",
        "X12V4010TS850",
        "X12V4010TS855",
        "X12V4010TS856",
        "X12V4010TS997",
    ] {
        let mut edi = EdiHandler::new();
        edi.load_envelope(dir.join("X12Envelope.js").to_str().unwrap())
            .unwrap()
            .load_body(dir.join(format!("{name}.js")).to_str().unwrap())
            .unwrap_or_else(|err| panic!("{name}: {err}"));
    }
    let mut edi = EdiHandler::new();
    edi.load_envelope(dir.join("X12Envelope.js").to_str().unwrap())
        .unwrap()
        .load_body(dir.join("X12V4010TS850.js").to_str().unwrap())
        .unwrap();
    let root = edi.parse_text(interchange("000000004", &PO_BODY).trim()).unwrap();
    let st = child(child(child(&root, "ISA", 0), "GS", 0), "ST", 0);
    assert_eq!(elements(child(st, "SE", 0)), vec![s("SE"), s("7"), s("0001")]);
}
