/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
//! Reporting component regressions, kept in `report::tests`.
use super::*;
use crate::components::line_text;
use serde_json::json;
#[test]
fn frames_show_null_as_missing_and_share_the_browser_numbers() {
    let props = json!({"panel":{"title":"Latency","visualization":"table"},"frame":{"fields":[{"name":"duration","type":"number","unit":"ns","values":[1250000,null]}]}});
    let lines: Vec<_> = ReportPanel
        .lines(&props, &ComponentState::default(), 60, false)
        .iter()
        .map(line_text)
        .collect();
    assert!(lines.iter().any(|line| line.contains("1.25 ms")));
    assert!(lines.iter().any(|line| line.contains('—')));
}
#[test]
fn waterfall_preserves_nanosecond_offsets_and_parent_depth() {
    let props = json!({"spans":[{"span_id":"root","name":"request","start_nano":"1800000000000000000","duration_nano":10000000},
        {"span_id":"child","parent_span_id":"root","name":"service","start_nano":"1800000000005000000","duration_nano":1000000}]});
    let lines: Vec<_> = TraceWaterfall
        .lines(&props, &ComponentState::default(), 100, false)
        .iter()
        .map(line_text)
        .collect();
    assert!(lines[1].starts_with("  service"));
    assert!(lines[0].contains("10.00ms"));
    assert!(lines[1].contains("1.00ms"));
    assert!(
        lines[1].find('━') > lines[0].find('━'),
        "the child retains its offset when nanoseconds arrive as strings"
    );
    assert!(!lines[0].contains("tokens"));
    let llm = json!({"spans":[{"span_id":"call","name":"openai:gpt","start_nano":"1800000000000000000","duration_nano":5,
        "gen_ai.usage.input_tokens":120,"attributes":{"gen_ai.usage.output_tokens":"30"}}]});
    let lines: Vec<_> = TraceWaterfall
        .lines(&llm, &ComponentState::default(), 100, false)
        .iter()
        .map(line_text)
        .collect();
    assert!(lines[0].ends_with(" 120→30 tokens"), "{lines:?}");
}
#[test]
fn field_overrides_and_inline_refresh_are_real() {
    let props = json!({"panel":{"title":"Latency","fieldConfig":{"defaults":{"decimals":1},"overrides":[{"match":{"name":"latency"},"config":{"unit":"s","noValue":"Missing"}}]}},"frame":{"fields":[{"name":"latency","type":"number","values":[1.25,null]}]}});
    let lines: Vec<_> = ReportPanel
        .lines(&props, &ComponentState::default(), 60, false)
        .iter()
        .map(line_text)
        .collect();
    assert!(lines.iter().any(|line| line.contains("1.2 s")));
    assert!(lines.iter().any(|line| line.contains("Missing")));
    let mut state = ComponentState::default();
    assert_eq!(
        ReportPanel.key(&props, &mut state, &KeyPress::plain(Key::Char('r')), false),
        Some(vec![Act::ReloadScreen])
    );
    ReportPanel.key(&props, &mut state, &KeyPress::plain(Key::Char('i')), false);
    assert!(ReportPanel
        .lines(&props, &state, 60, false)
        .iter()
        .map(line_text)
        .any(|line| line.contains("fieldConfig")));
}
#[test]
fn dashboard_tabs_inspector_exports_and_time_keys_use_current_scope() {
    let props = json!({"dashboard":{"definitionId":"monitor-artifacts","absoluteTime":{"from":"2026-01-01T00:00:00Z","thru":"2026-01-02T00:00:00Z"},"variableValues":{"service":"ERP"},"layout":{"type":"tabs","items":[{"title":"First","layout":{"items":[{"panelId":"one"}]}},{"title":"Second","layout":{"items":[{"panelId":"two"}]}}]},"panels":{"one":{"title":"Panel one"},"two":{"title":"Panel two","queries":[{"datasetId":"otel_metrics"}]}}},"frames":{}});
    let mut state = ComponentState::default();
    let text = |state: &ComponentState| {
        ReportDashboard
            .lines(&props, state, 100, false)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(text(&state).contains("Panel one"));
    assert!(!text(&state).contains("Panel two"));
    ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Right), false);
    assert!(text(&state).contains("Panel two"));
    ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Char('i')), false);
    assert!(text(&state).contains("otel_metrics"));
    let export = ReportDashboard
        .key(&props, &mut state, &KeyPress::plain(Key::Char('x')), false)
        .unwrap();
    assert!(
        matches!(&export[0],Act::DownloadReport(url) if url.contains("panelId=two") && url.contains("renderMode=xlsx") && url.contains("ERP"))
    );
    ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Char('t')), false);
    let shifted = ReportDashboard
        .key(&props, &mut state, &KeyPress::plain(Key::Right), false)
        .unwrap();
    assert!(
        matches!(&shifted[0],Act::Follow(url) if url.contains("from=2026-01-02") && url.contains("var-service=ERP"))
    );
}
#[test]
fn live_tail_merges_cursor_pages_bounds_rows_and_pauses() {
    let props = json!({"source":"/tail"});
    let mut state = ComponentState::default();
    LogTail.receive(
        &props,
        &mut state,
        json!({"hits":[{"documentId":"otel_logs","id":"first","body":{"body":"first"}}],"nextCursor":{"id":1}}),
    );
    assert_eq!(LogTail.poll_millis(&props, &state), None);
    assert_eq!(
        LogTail.key(&props, &mut state, &KeyPress::plain(Key::Char('p')), false),
        Some(vec![Act::Reload])
    );
    assert_eq!(LogTail.poll_millis(&props, &state), Some(2000));
    assert_eq!(LogTail.source(&props, &state).unwrap().1["cursor"], "{\"id\":1}");
    LogTail.receive(&props,&mut state,json!({"hits":[{"documentId":"otel_logs","id":"first"},{"documentId":"otel_logs","id":"second"}],"nextCursor":{"id":2}}));
    assert_eq!(state.data.as_ref().unwrap()["hits"].as_array().unwrap().len(), 2);
    let hits: Vec<_> = (0..600)
        .map(|at| json!({"documentId":"otel_logs","id":at.to_string()}))
        .collect();
    LogTail.receive(&props, &mut state, json!({"hits":hits,"nextCursor":null}));
    assert_eq!(state.data.as_ref().unwrap()["hits"].as_array().unwrap().len(), 500);
    assert_eq!(state.data.as_ref().unwrap()["nextCursor"], json!({"id":2}));
    LogTail.key(&props, &mut state, &KeyPress::plain(Key::Char('p')), false);
    assert_eq!(LogTail.poll_millis(&props, &state), None);
}
#[test]
fn span_inspector_preserves_attributes_and_events() {
    let props = json!({"spans":[{"span_id":"child","name":"child","start_nano":"1800000000000000010","attributes":{"http.route":"/ERP"},"events":[{"name":"exception"}]},{"span_id":"root","name":"root","start_nano":"1800000000000000000"}]});
    let mut state = ComponentState::default();
    TraceWaterfall.key(&props, &mut state, &KeyPress::plain(Key::Down), false);
    TraceWaterfall.key(&props, &mut state, &KeyPress::plain(Key::Char('i')), false);
    let text = TraceWaterfall
        .lines(&props, &state, 100, false)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("http.route") && text.contains("exception"));
}
#[test]
fn text_panels_do_not_require_frames_and_bytes_use_the_browser_scale() {
    let lines = panel_lines(
        &json!({"fields":[]}),
        &json!({"title":"Notes","visualization":"text","text":"Release notes\nKeep watching latency"}),
        80,
    );
    let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
    assert!(text.contains("Release notes") && text.contains("Keep watching latency"));
    assert!(!text.contains("No data"));
    assert_eq!(
        formatted(&json!(2048), &json!({"type":"number","unit":"bytes"})),
        "2 KiB"
    );
    assert_eq!(
        formatted(&json!(1536), &json!({"type":"number","unit":"bytes"})),
        "1.50 KiB"
    );
    let fields = configured_fields(
        &json!({"fields":[{"name":"value","unit":"s"}]}),
        &json!({"fieldConfig":{"defaults":{"unit":"ms"}}}),
    );
    assert_eq!(fields[0]["unit"], "s");
}
#[test]
fn partial_and_subtotal_metadata_are_not_numeric_chart_series() {
    let frame = json!({"fields":[{"name":"partial","type":"boolean","values":[true]},{"name":"__level","type":"number","values":[0]},{"name":"__subtotal","type":"number","values":[1]},{"name":"time","type":"time","values":[1000]},{"name":"requests","type":"number","values":[3]}]});
    let fields = configured_fields(&frame, &json!({}));
    let series: Vec<_> = fields
        .iter()
        .filter(|field| field["type"] == "number" && !is_metadata(field))
        .map(|field| field["name"].as_str().unwrap())
        .collect();
    assert_eq!(series, ["requests"]);
    let lines = panel_lines(&frame, &json!({"visualization":"stat"}), 80)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(lines.contains('3'));
    assert!(!lines.contains("0.00"));
}
#[test]
fn gaps_are_missing_in_sparklines() {
    assert_eq!(sparkline(&[json!(1), Value::Null, json!(3)], 3), "▁ █");
}
