/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
//! Reporting data and interaction regressions, kept in `report::gap_tests`.
use super::*;
use serde_json::json;
#[test]
fn split_series_preserve_time_gaps_groups_and_original_rows() {
    let frame = json!({"fields":[
        {"name":"time","type":"time","values":[0,0,1000,2000,2000,3000]},
        {"name":"service","type":"string","values":["A","B","A","A","B",null]},
        {"name":"latency","type":"number","values":[1,100,null,3,300,null]},
        {"name":"__level","type":"number","values":[0,0,0,0,0,0]}
    ]});
    let panel = json!({"visualization":"timeSeries","queries":[{"aggregations":[{"id":"service","role":"series"}]}]});
    let trends = trend_series(&frame, &panel);
    assert!(!trends.limited);
    assert_eq!(trends.series.len(), 2);
    assert_eq!(trends.series[0].label, "latency {service=A}");
    assert_eq!(
        trends.series[0].values,
        vec![json!(1), Value::Null, json!(3), Value::Null]
    );
    assert_eq!(trends.series[0].rows, vec![Some(0), Some(2), Some(3), None]);
    assert_eq!(trends.series[1].label, "latency {service=B}");
    assert_eq!(
        trends.series[1].values,
        vec![json!(100), Value::Null, json!(300), Value::Null]
    );
    assert_eq!(trends.series[1].rows, vec![Some(1), None, Some(4), None]);
    let text = panel_lines(&frame, &panel, 100)
        .iter()
        .map(|line| line.iter().map(|span| span.text.as_str()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("service=A") && text.contains("service=B"));
    assert!(text.contains("source rows: 1,3,4") && text.contains("source rows: 2,5"));
    // Compression cannot bridge a missing coordinate.
    assert_eq!(sparkline(&[json!(0), Value::Null, json!(2), json!(3)], 2), " █");
}

#[test]
fn numeric_hidden_dimensions_and_duplicate_points_are_independent_tracks() {
    let frame = json!({"fields":[
        {"name":"time","type":"time","values":[0,0,0,1000]},
        {"name":"code","type":"integer","values":[200,200,500,500],"config":{"role":"split-series","hidden":true}},
        {"name":"requests","type":"integer","values":[0,2,10,11]}
    ]});
    let trends = trend_series(&frame, &json!({}));
    assert_eq!(trends.series.len(), 3);
    assert_eq!(trends.series[0].label, "requests {code=200} [observation 1]");
    assert_eq!(trends.series[0].values, vec![json!(0), Value::Null]);
    assert_eq!(trends.series[1].rows, vec![Some(1), None]);
    assert_eq!(trends.series[2].values, vec![json!(10), json!(11)]);
    let documents = trend_series(
        &json!({"fields":[{"name":"time","type":"time","values":[0,1000]},{"name":"message","type":"string","values":["one","two"]},{"name":"value","type":"number","values":[1,2]}]}),
        &json!({"queries":[{"mode":"documents"}]}),
    );
    assert_eq!(documents.series.len(), 1);
    assert_eq!(documents.series[0].values, vec![json!(1), json!(2)]);
}

#[test]
fn boolean_bucket_flags_do_not_split_trends_unless_explicitly_assigned() {
    let frame = json!({"fields":[
        {"name":"time","type":"time","values":[0,1000]},
        {"name":"missing","type":"boolean","values":[false,true]},
        {"name":"other","type":"boolean","values":[true,false]},
        {"name":"missing.2","type":"boolean","values":[true,false]},
        {"name":"other.2","type":"boolean","values":[false,true]},
        {"name":"requests","type":"number","values":[0,2]}
    ]});
    let trend = trend_series(&frame, &json!({}));
    assert_eq!(trend.series.len(), 1);
    assert_eq!(trend.series[0].label, "requests");
    assert_eq!(trend.series[0].values, vec![json!(0), json!(2)]);
    assert_eq!(trend.series[0].rows, vec![Some(0), Some(1)]);
    // An authored aggregation role wins over the reserved-looking name.
    let explicit = trend_series(
        &frame,
        &json!({"queries":[{"aggregations":[{"id":"missing.2","role":"series"}]}]}),
    );
    assert_eq!(explicit.series.len(), 2);
    assert!(explicit.series.iter().all(|series| series.label.contains("missing.2=")));
    assert!(!is_metadata(&json!({"name":"missing","type":"number"})));
    assert!(!is_metadata(&json!({"name":"other.business","type":"boolean"})));
}

#[test]
fn sorted_bucket_filters_keep_exact_missing_other_predicates_and_dashboard_scope() {
    let predicates = json!([{"not":{"field":{"name":"status","op":"in","values":["A"]}}},{"not":{"field":{"name":"status","op":"exists"}}},{"field":{"name":"status","op":"eq","value":"A"}}]);
    let frame = json!({"fields":[{"name":"status","type":"string","values":["Other","Missing","A"],"config":{"bucket":{"datasetId":"orders","filters":predicates,"labels":["other","missing","A"]}}}]});
    let dashboard = json!({"definitionId":"business","absoluteTime":{"from":"2026-01-01","thru":"2026-01-02"},"filters":[{"query":{"field":{"name":"tenant","op":"eq","value":"T1"}}}],"variableValues":{"region":"west"}});
    let mut state = ComponentState::default();
    table_key(
        &frame,
        &json!({}),
        &mut state,
        &KeyPress::plain(Key::Char('s')),
        Some(&dashboard),
    );
    assert_eq!(
        table_frame(&frame, &state)["fields"][0]["config"]["bucket"]["labels"][0],
        "A"
    );
    for (row, expected) in [(0, 2), (1, 1), (2, 0)] {
        state.at = row;
        let acts = table_key(
            &frame,
            &json!({}),
            &mut state,
            &KeyPress::plain(Key::Char('f')),
            Some(&dashboard),
        )
        .unwrap();
        let Act::Follow(url) = &acts[0] else {
            panic!("expected filtered dashboard")
        };
        let location = crate::nav::Location::of_url(url);
        let filters: Value = serde_json::from_str(&location.parameters["filtersJson"]).unwrap();
        assert_eq!(filters[0], dashboard["filters"][0]);
        assert_eq!(filters[1]["query"], predicates[expected]);
        assert_eq!(location.parameters["var-region"], "west");
        assert_eq!(location.parameters["from"], "2026-01-01");
    }
}
#[test]
fn sorting_keeps_row_link_values_aligned_and_rejects_unsafe_links() {
    let props = json!({"panel":{"visualization":"table","fieldConfig":{"overrides":[{"match":{"name":"id"},"config":{"link":"/record?id=${value}&amount=${amount}"}}]}},"frame":{"fields":[{"name":"id","type":"string","values":["B & C","A"]},{"name":"amount","type":"number","values":[9,0]}]}});
    let mut state = ComponentState {
        choosing: Some(1),
        ..Default::default()
    };
    ReportPanel.key(&props, &mut state, &KeyPress::plain(Key::Char('s')), false);
    let frame = table_frame(&props["frame"], &state);
    assert_eq!(frame["fields"][0]["values"][0], "A");
    assert_eq!(frame["fields"][1]["values"][0], 0);
    state.choosing = Some(0);
    assert_eq!(
        ReportPanel.key(&props, &mut state, &KeyPress::plain(Key::Enter), false),
        Some(vec![Act::Follow("/record?id=A&amount=0".into())])
    );
    ReportPanel.key(&props, &mut state, &KeyPress::plain(Key::Down), false);
    assert_eq!(
        ReportPanel.key(&props, &mut state, &KeyPress::plain(Key::Enter), false),
        Some(vec![Act::Follow("/record?id=B%20%26%20C&amount=9".into())])
    );
    let unsafe_props =
        json!({"panel":{},"frame":{"fields":[{"name":"id","values":["X"],"link":"javascript:alert('${value}')"}]}});
    state.at = 0;
    assert!(
        matches!(ReportPanel.key(&unsafe_props,&mut state,&KeyPress::plain(Key::Enter),false),Some(acts)if matches!(&acts[0],Act::Message(_)))
    );
}
#[test]
fn nested_tabs_and_rows_change_visibility_without_flattening_all_tabs() {
    let props = json!({"dashboard":{"layout":{"type":"row","title":"Business","items":[{"type":"tabs","items":[{"title":"First","layout":{"panelId":"a"}},{"title":"Second","layout":{"type":"row","items":[{"panelId":"b"}]}}]}]},"panels":{"a":{"title":"One"},"b":{"title":"Two"}}}});
    let mut state = ComponentState::default();
    assert_eq!(
        dashboard_entries(&props, &state)
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        vec!["a"]
    );
    ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Char(']')), false);
    ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Right), false);
    assert_eq!(dashboard_entries(&props, &state)[0].0, "b");
    ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Char('[')), false);
    ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Char(' ')), false);
    assert!(dashboard_entries(&props, &state).is_empty());
    ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Char(' ')), false);
    assert_eq!(dashboard_entries(&props, &state)[0].0, "b");
}
#[test]
fn inspector_fetches_authorized_steps_with_the_visible_panel_scope() {
    let props = json!({"dashboard":{"definitionId":"sales","absoluteTime":{"from":"2026-01-01T00:00:00Z","thru":"2026-01-02T00:00:00Z"},"variableValues":{"tenant":"A & B"},"panels":{"p":{"title":"Sales","queries":[{"datasetId":"sales"}]}}},"frames":{}});
    let mut state = ComponentState::default();
    let acts = ReportDashboard
        .key(&props, &mut state, &KeyPress::plain(Key::Char('i')), false)
        .unwrap();
    assert!(
        matches!(&acts[0],Act::Read(url)if url.contains("inspectPanel")&&url.contains("panelId=p")&&url.contains("A%20%26%20B")&&url.contains("variables="))
    );
    ReportDashboard.receive(&props,&mut state,json!({"dashboard":{"panels":{"p":{"requests":[{"datasetId":"sales"}],"inspection":[{"index":0,"type":"pivot","input":{"frames":[{"fields":[]}]},"output":{"frames":[{"fields":[{"name":"sales","values":[0,null]}]}]}}]}}}}));
    let text = ReportDashboard
        .lines(&props, &state, 120, false)
        .iter()
        .map(super::super::line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("pivot") && text.contains("input") && text.contains("output") && text.contains("null"));
    assert!(state.data.is_none(), "inspector must not replace dashboard frames");
}
#[test]
fn an_including_screen_takes_the_panel_requests_and_esc_leaves_the_single_panel_view() {
    let props = json!({"dashboard":{"definitionId":"monitor-overview","screenUrl":"/apps/system/Monitor/Overview","panelId":"p","absoluteTime":{"from":"2026-01-01T00:00:00Z","thru":"2026-01-02T00:00:00Z"},"variableValues":{"service":"erp"},"panels":{"p":{"title":"Errors"}}},"frames":{}});
    let mut state = ComponentState::default();
    let text = ReportDashboard
        .lines(&props, &state, 120, false)
        .iter()
        .map(super::super::line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Showing panel p; Esc shows all panels"), "{text}");
    let acts = ReportDashboard
        .key(&props, &mut state, &KeyPress::plain(Key::Char('c')), false)
        .unwrap();
    assert!(
        matches!(&acts[0],Act::DownloadReport(url)if url.starts_with("/apps/system/Monitor/Overview/exportPanel?")&&url.contains("panelId=p")),
        "{acts:?}"
    );
    // a time shift keeps the single-panel view and the variables
    ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Char('t')), false);
    let acts = ReportDashboard
        .key(&props, &mut state, &KeyPress::plain(Key::Left), false)
        .unwrap();
    assert!(
        matches!(&acts[0],Act::Follow(url)if url.starts_with("/apps/system/Monitor/Overview?")&&url.contains("panelId=p")&&url.contains("var-service=erp")),
        "{acts:?}"
    );
    // Esc closes the inspector first, then shows all the panels
    ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Char('i')), false);
    assert_eq!(
        ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Escape), false),
        Some(Vec::new())
    );
    let acts = ReportDashboard
        .key(&props, &mut state, &KeyPress::plain(Key::Escape), false)
        .unwrap();
    assert!(
        matches!(&acts[0],Act::Follow(url)if url.starts_with("/apps/system/Monitor/Overview?")&&!url.contains("panelId")&&url.contains("var-service=erp")),
        "{acts:?}"
    );
    // without a single-panel view Esc is the session's
    let all = json!({"dashboard":{"definitionId":"d","panels":{"p":{}}},"frames":{}});
    assert_eq!(
        ReportDashboard.key(
            &all,
            &mut ComponentState::default(),
            &KeyPress::plain(Key::Escape),
            false
        ),
        None
    );
}
#[test]
fn time_picker_applies_a_quick_range_and_panel_menu_runs_its_items() {
    let props = json!({"dashboard":{"definitionId":"d","title":"Board","editable":true,"absoluteTime":{"from":"2026-10-27T00:00:00Z","thru":"2026-10-28T00:00:00Z"},
        "requestedTime":{"from":"now-24h","thru":"now"},"filters":[{"datasetId":"orders","query":{"field":{"name":"status","op":"eq","value":"open"}}}],
        "panels":{"p":{"title":"Orders","queries":[{"datasetId":"orders","queryString":"total:>5","aggregations":[{"id":"n","type":"sum","role":"metric"}]}]}}},"frames":{}});
    let mut state = ComponentState::default();
    let text = |state: &ComponentState| {
        ReportDashboard
            .lines(&props, state, 120, false)
            .iter()
            .map(super::super::line_text)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let press = |state: &mut ComponentState, key: Key| ReportDashboard.key(&props, state, &KeyPress::plain(key), false);
    // p opens the picker at the current range; Down and Enter apply the next quick range.
    press(&mut state, Key::Char('p'));
    assert!(text(&state).contains("▶ Last 24 hours"), "{}", text(&state));
    press(&mut state, Key::Down);
    let Some(acts) = press(&mut state, Key::Enter) else {
        panic!("no act")
    };
    assert!(
        matches!(&acts[..], [Act::Follow(url)] if url.contains("from=now-2d") && url.contains("thru=now")),
        "{acts:?}"
    );
    assert!(state.local["picker"].is_null());
    // m opens the panel menu; its items follow the panel and the dashboard.
    press(&mut state, Key::Char('m'));
    let shown = text(&state);
    for item in [
        "View",
        "Edit",
        "Share",
        "Explore",
        "Inspect",
        "Duplicate",
        "Remove",
        "New alert rule",
    ] {
        assert!(shown.contains(item), "{item}: {shown}");
    }
    press(&mut state, Key::Down);
    press(&mut state, Key::Down);
    press(&mut state, Key::Down);
    let Some(acts) = press(&mut state, Key::Enter) else {
        panic!("no act")
    };
    assert!(
        matches!(&acts[..], [Act::Follow(url)] if url.starts_with("/apps/dashboards/Explore?datasetId=orders")
            && url.contains("queryString=total") && url.contains("status") && url.contains("from=2026-10-27")),
        "{acts:?}"
    );
    // Remove asks for a second Enter, then posts the dashboard's transition.
    state.local["menu"] = json!(6);
    assert!(matches!(
        press(&mut state, Key::Enter).as_deref(),
        Some([Act::Message(_)])
    ));
    let Some(acts) = press(&mut state, Key::Enter) else {
        panic!("no act")
    };
    assert!(
        matches!(&acts[..], [Act::Post { url, fields }] if url.ends_with("/removePanel") && fields["panelId"] == "p"),
        "{acts:?}"
    );
    assert!(state.local["menu"].is_null());
    // Esc closes an open menu without leaving anything else.
    press(&mut state, Key::Char('m'));
    assert_eq!(press(&mut state, Key::Escape), Some(Vec::new()));
    assert!(state.local["menu"].is_null());
}
#[test]
fn kiosk_mode_scrolls_by_panel_and_by_page() {
    let props = json!({"dashboard":{"definitionId":"d","title":"Wall","kiosk":true,"panels":{"a":{"title":"First","visualization":"text","text":"one"},"b":{"title":"Second","visualization":"text","text":"two"}}},"frames":{}});
    let mut state = ComponentState::default();
    let text = |state: &ComponentState| {
        ReportDashboard
            .lines(&props, state, 100, false)
            .iter()
            .map(super::super::line_text)
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(text(&state).contains("First"));
    ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Down), false);
    let shown = text(&state);
    assert!(
        shown.starts_with("Wall") && !shown.contains("First") && shown.contains("Second"),
        "{shown}"
    );
    ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::PageDown), false);
    assert_eq!(state.local["kioskOffset"], 10);
    assert!(text(&state).starts_with("Wall"));
    ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::PageUp), false);
    assert_eq!(state.local["kioskOffset"], 0);
}
#[test]
fn data_links_carry_the_dashboard_time_range() {
    let frame = json!({"fields":[{"name":"service","values":["shop"],"link":"/apps/system/Monitor/Services?var-service=${service}&from=${__from}&thru=${__thru}"}]});
    let dashboard = json!({"absoluteTime":{"from":"2026-10-27T00:00:00Z","thru":"2026-10-28T00:00:00Z"}});
    let mut state = ComponentState {
        choosing: Some(0),
        ..Default::default()
    };
    let acts = table_key(
        &frame,
        &json!({}),
        &mut state,
        &KeyPress::plain(Key::Enter),
        Some(&dashboard),
    );
    assert_eq!(
        acts,
        Some(vec![Act::Follow(
            "/apps/system/Monitor/Services?var-service=shop&from=2026-10-27T00%3A00%3A00Z&thru=2026-10-28T00%3A00%3A00Z"
                .into()
        )])
    );
}
#[test]
fn kiosk_mode_toggles_with_d_k_and_esc_leaves_it_before_the_single_panel_view() {
    let props = json!({"dashboard":{"definitionId":"d","title":"Shipping board","panelId":"p","refreshSeconds":30,"panels":{"p":{"title":"Orders"}}},"frames":{}});
    let mut state = ComponentState::default();
    let text = |state: &ComponentState| {
        ReportDashboard
            .lines(&props, state, 100, false)
            .iter()
            .map(super::super::line_text)
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(!ReportDashboard.kiosk(&props, &state));
    assert_eq!(
        ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Char('d')), false),
        Some(Vec::new())
    );
    ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Char('k')), false);
    assert!(ReportDashboard.kiosk(&props, &state));
    let shown = text(&state);
    assert!(shown.contains("Shipping board  (kiosk; Esc or d k leaves)"), "{shown}");
    assert!(
        !shown.contains("Showing panel") && !shown.contains("Selected panel"),
        "{shown}"
    );
    // Esc leaves kiosk first, then the single-panel view.
    assert_eq!(
        ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Escape), false),
        Some(Vec::new())
    );
    assert!(!ReportDashboard.kiosk(&props, &state));
    assert!(matches!(
        ReportDashboard
            .key(&props, &mut state, &KeyPress::plain(Key::Escape), false)
            .as_deref(),
        Some([Act::Follow(_)])
    ));
    // Alt+K as in the browser; a kiosk=true dashboard opens in kiosk mode.
    let alt = KeyPress {
        alt: true,
        ..KeyPress::plain(Key::Char('k'))
    };
    ReportDashboard.key(&props, &mut state, &alt, false);
    assert!(ReportDashboard.kiosk(&props, &state));
    let mut opened = props.clone();
    opened["dashboard"]["kiosk"] = json!(true);
    assert!(ReportDashboard.kiosk(&opened, &ComponentState::default()));
    // The dashboard's refresh interval reloads it: the screen, for inline frames.
    assert_eq!(ReportDashboard.poll_millis(&props, &state), Some(30_000));
    assert_eq!(ReportDashboard.tick(&props, &mut state), vec![Act::ReloadScreen]);
    let mut slow = props.clone();
    slow["dashboard"]["refreshSeconds"] = json!(2);
    assert_eq!(ReportDashboard.poll_millis(&slow, &state), None);
}
#[test]
fn currency_units_format_as_the_browser_does() {
    let field = |unit: &str| json!({"type":"number","unit":unit});
    assert_eq!(formatted(&json!(1234567.5), &field("USD")), "$1,234,567.50");
    assert_eq!(formatted(&json!(-12), &field("currency:EUR")), "-€12.00");
    assert_eq!(formatted(&json!(1234.4), &field("JPY")), "¥1,234.4");
    assert_eq!(formatted(&json!(1234), &field("JPY")), "¥1,234");
    assert_eq!(formatted(&json!(5), &field("XYZ")), "XYZ 5.00");
    assert_eq!(
        formatted(&json!(9.999), &json!({"type":"number","unit":"GBP","decimals":0})),
        "£10"
    );
    assert_eq!(formatted(&json!(5), &field("usd")), "5", "not a currency code");
}
#[test]
fn dashboard_charts_replace_the_table_except_in_table_mode() {
    let props = json!({"dashboard":{"definitionId":"d","panels":{"p":{"title":"By status","visualization":"bar"}}},
        "frames":{"p":[{"fields":[{"name":"status","type":"string","values":["Approved","Held"]},{"name":"orders","type":"number","values":[4,2]}]}]}});
    let mut state = ComponentState::default();
    let text = |state: &ComponentState| {
        ReportDashboard
            .lines(&props, state, 80, false)
            .iter()
            .map(super::super::line_text)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let chart = text(&state);
    assert!(
        chart.contains("Approved │████") && !chart.contains("status  "),
        "{chart}"
    );
    ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Char('g')), false);
    let table = text(&state);
    assert!(table.contains("Approved │") && table.contains("orders"), "{table}");
    // Too narrow for the chart: the table.
    let narrow = ReportDashboard
        .lines(&props, &ComponentState::default(), 24, false)
        .iter()
        .map(super::super::line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!narrow.contains('│') && narrow.contains("orders"), "{narrow}");
}
#[test]
fn playlist_rotation_is_bounded_pauses_and_preserves_request_authority() {
    let props = json!({"source":"/apps/dashboards/Playlist/data","definitionId":"my-playlist","from":"now-1h","thru":"now","seconds":1,"items":[{"title":"A"},{"title":"B"}]});
    let mut state = ComponentState::default();
    assert_eq!(ReportPlaylist.poll_millis(&props, &state), Some(5000));
    assert_eq!(ReportPlaylist.source(&props, &state).unwrap().1["index"], "0");
    assert_eq!(ReportPlaylist.tick(&props, &mut state), vec![Act::Reload]);
    let params = ReportPlaylist.source(&props, &state).unwrap().1;
    assert_eq!(params["index"], "1");
    assert_eq!(params["definitionId"], "my-playlist");
    ReportPlaylist.key(&props, &mut state, &KeyPress::plain(Key::Char('p')), false);
    assert_eq!(ReportPlaylist.poll_millis(&props, &state), None);
    // An older caller without entry indices keeps positional behavior above; a filtered screen keeps original indices.
    let mut filtered = props.clone();
    filtered["items"] = json!([{"title":"B","index":1},{"title":"D","index":3}]);
    let mut filtered_state = ComponentState::default();
    assert_eq!(
        ReportPlaylist.source(&filtered, &filtered_state).unwrap().1["index"],
        "1"
    );
    assert_eq!(ReportPlaylist.tick(&filtered, &mut filtered_state), vec![Act::Reload]);
    let params = ReportPlaylist.source(&filtered, &filtered_state).unwrap().1;
    assert_eq!(params["index"], "3");
    assert_eq!(params["definitionId"], "my-playlist");
    assert_eq!(params["from"], "now-1h");
    assert_eq!(params["thru"], "now");
}
#[test]
fn a_mounted_dashboard_links_its_apps_screens_and_an_unreadable_panel_says_so() {
    // An app that mounts the report screens (Insights) sends appLinks: edit, Explore and the panel posts go to its
    // screens, and a screen it leaves out (no alert rules) is not offered.
    let props = json!({"dashboard":{"definitionId":"d","editable":true,"absoluteTime":{"from":"2026-10-27T00:00:00Z","thru":"2026-10-28T00:00:00Z"},
        "appLinks":{"definition":"/apps/Insights/DashboardEditor","panel":"/apps/Insights/PanelDetail","explore":"/apps/Insights/ReportBuilder/Explore","dashboard":"/apps/Insights/Dashboard","alertRule":null},
        "panels":{"p":{"title":"Orders","queries":[{"datasetId":"orders","aggregations":[{"id":"n","type":"sum","role":"metric"}]}]},
            "q":{"title":"Sales","queries":[{"datasetId":"sales"}],"notices":[{"level":"error","complete":false,"message":"viewer may not read dataset sales"}]}}},"frames":{}});
    let mut state = ComponentState::default();
    let press = |state: &mut ComponentState, key: Key| ReportDashboard.key(&props, state, &KeyPress::plain(key), false);
    press(&mut state, Key::Char('m'));
    let items: Vec<&str> = panel_menu(&props["dashboard"], &props["dashboard"]["panels"]["p"])
        .iter()
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(
        items,
        ["view", "edit", "share", "explore", "inspect", "duplicate", "remove"]
    );
    let dashboard = &props["dashboard"];
    let panel = &dashboard["panels"]["p"];
    let edit = panel_menu_act("edit", dashboard, "p", panel, &mut state);
    assert!(
        matches!(&edit[..], [Act::Follow(url)] if url.starts_with("/apps/Insights/DashboardEditor?definitionId=d&panelId=p")),
        "{edit:?}"
    );
    let explore = panel_menu_act("explore", dashboard, "p", panel, &mut state);
    assert!(
        matches!(&explore[..], [Act::Follow(url)] if url.starts_with("/apps/Insights/ReportBuilder/Explore?datasetId=orders")),
        "{explore:?}"
    );
    let duplicate = panel_menu_act("duplicate", dashboard, "p", panel, &mut state);
    assert!(
        matches!(&duplicate[..], [Act::Post { url, .. }] if url == "/apps/Insights/Dashboard/duplicatePanel"),
        "{duplicate:?}"
    );
    assert!(panel_menu_act("alert", dashboard, "p", panel, &mut state).is_empty());
    let unavailable = panel_lines(&json!({"fields":[]}), &dashboard["panels"]["q"], 80)
        .iter()
        .map(|line| line.iter().map(|span| span.text.as_str()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        unavailable.contains("Not available: viewer may not read dataset sales"),
        "{unavailable}"
    );
    assert!(!unavailable.contains("No data"), "{unavailable}");
    // Tools' own dashboards send Tools' links, and a dashboard without appLinks keeps them.
    let tools = json!({"definitionId":"d","panels":{}});
    assert_eq!(
        app_link(&tools, "alertRule", "AlertRule").as_deref(),
        Some("/apps/dashboards/AlertRule")
    );
}
