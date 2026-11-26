/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

use super::*;
use crate::components::line_text;

/// Props as ScreenTui.js writes them for a map-view with two places (one with its precision and source) and pick.
fn props(pick: bool) -> Value {
    json!({"points": [
        {"id": "p0", "lon": -87.6298, "lat": 41.8781, "label": "Chicago office", "precision": "Address match", "source": "Gallery sample"},
        {"id": "p1", "lon": 0, "lat": 0, "label": "Null Island"},
        {"id": "p2", "lon": 200, "lat": 10, "label": "Off the globe"},
        {"id": "p3", "lon": -68.13, "lat": -67.57, "label": "Rothera"}
    ], "pick": pick, "attribution": "Natural Earth"})
}

fn shown(props: &Value, state: &ComponentState) -> Vec<String> {
    Map.lines(props, state, 80, false).iter().map(line_text).collect()
}

fn press(key: Key) -> KeyPress {
    KeyPress::plain(key)
}

#[test]
fn the_places_are_a_list_over_the_land_with_the_attribution() {
    let props = props(false);
    let state = ComponentState::default();
    let lines = Map.lines(&props, &state, 80, false);
    let text: Vec<String> = lines.iter().map(line_text).collect();
    // the overview: 72 columns of braille, the places raised over the land
    assert!(
        text[0].chars().all(|ch| ('\u{2800}'..='\u{28ff}').contains(&ch)),
        "{text:?}"
    );
    assert_eq!(text[0].chars().count(), 72);
    assert!(
        lines.iter().flatten().any(|span| span.role == Role::Negative),
        "a place is drawn"
    );
    assert!(
        lines
            .iter()
            .flatten()
            .any(|span| span.role == Role::Primary && span.text != "▸ "),
        "the selected place is marked"
    );
    assert!(text.contains(&"Places".to_string()), "{text:?}");
    assert!(
        text.contains(&"▸ Chicago office  41.87810, -87.62980  Address match · Gallery sample".to_string()),
        "{text:?}"
    );
    assert!(
        text.contains(&"  Null Island  0.00000, 0.00000".to_string()),
        "a zero is a coordinate: {text:?}"
    );
    assert!(!text.iter().any(|line| line.contains("Off the globe")), "{text:?}");
    // south of the overview's edge: listed, not drawn
    assert!(
        text.contains(&"  Rothera  -67.57000, -68.13000".to_string()),
        "{text:?}"
    );
    let raised = |lines: &[Line]| {
        lines
            .iter()
            .flatten()
            .filter(|span| span.role == Role::Negative)
            .count()
    };
    let without = json!({"points": [props["points"][0], props["points"][1]]});
    assert_eq!(
        raised(&lines),
        raised(&Map.lines(&without, &state, 80, false)),
        "Rothera is not drawn"
    );
    assert_eq!(text.last().map(String::as_str), Some("Map data: Natural Earth"));
    assert!(
        !text.iter().any(|line| line.contains("choose a point")),
        "no pick: {text:?}"
    );
    // no place: the view's center or area
    let empty = shown(
        &json!({"points": [], "center": [10, 20], "extent": [-0.5, 51.3, 0.3, 51.7]}),
        &state,
    );
    assert_eq!(
        empty,
        [
            "No places",
            "Center: 20.00000, 10.00000",
            "Area: 51.30000, -0.50000 – 51.70000, 0.30000"
        ]
    );
}

#[test]
fn a_place_is_selected_and_its_coordinates_copied() {
    let props = props(false);
    let mut state = ComponentState::default();
    assert_eq!(Map.key(&props, &mut state, &press(Key::Down), false), Some(Vec::new()));
    assert_eq!(state.at, 1);
    assert!(shown(&props, &state).contains(&"▸ Null Island  0.00000, 0.00000".to_string()));
    let acts = Map.key(&props, &mut state, &press(Key::Char('c')), false).unwrap();
    assert_eq!(acts[0], Act::Copy("0.00000, 0.00000".to_string()));
    // the place off the globe is not one: Rothera is the last
    Map.key(&props, &mut state, &press(Key::Down), false).unwrap();
    assert_eq!(Map.key(&props, &mut state, &press(Key::Down), false), None);
    assert_eq!(state.at, 2);
    assert_eq!(
        Map.key(&props, &mut state, &press(Key::Char('p')), false),
        None,
        "no pick, no typing"
    );
    assert_eq!(Map.key(&props, &mut state, &press(Key::Enter), false), None);
}

#[test]
fn with_pick_a_point_is_chosen_by_its_latitude_and_longitude() {
    let props = props(true);
    let mut state = ComponentState::default();
    assert!(shown(&props, &state)
        .iter()
        .any(|line| line.starts_with("p: choose a point")));
    Map.key(&props, &mut state, &press(Key::Char('p')), false).unwrap();
    for ch in "41.9x, -87.6".chars() {
        Map.key(&props, &mut state, &press(Key::Char(ch)), false).unwrap();
    }
    assert!(
        shown(&props, &state).contains(&"Latitude, longitude: 41.9, -87.6▏".to_string()),
        "a letter is not taken"
    );
    Map.key(&props, &mut state, &press(Key::Backspace), false).unwrap();
    Map.paste(&props, &mut state, "5 ").unwrap();
    let acts = Map.key(&props, &mut state, &press(Key::Enter), false).unwrap();
    assert_eq!(acts[0], Act::Copy("41.90000, -87.50000".to_string()));
    let text = shown(&props, &state);
    assert!(text.contains(&"Chosen: 41.90000, -87.50000".to_string()), "{text:?}");
    // out of range: refused, still typing
    Map.key(&props, &mut state, &press(Key::Char('p')), false).unwrap();
    for ch in "-187.6, 41.9".chars() {
        Map.key(&props, &mut state, &press(Key::Char(ch)), false).unwrap();
    }
    assert_eq!(Map.key(&props, &mut state, &press(Key::Enter), false), Some(Vec::new()));
    assert!(shown(&props, &state)
        .iter()
        .any(|line| line.starts_with("Enter a latitude and a longitude")));
    Map.key(&props, &mut state, &press(Key::Escape), false).unwrap();
    assert!(!typing(&state));
    // Enter on a place chooses it
    let acts = Map.key(&props, &mut state, &press(Key::Enter), false).unwrap();
    assert_eq!(acts[0], Act::Copy("41.87810, -87.62980".to_string()));
    assert_eq!(parse_lat_lon("41.8781 -87.6298"), Some((-87.6298, 41.8781)));
    assert_eq!(parse_lat_lon("41.8"), None);
    assert_eq!(parse_lat_lon("NaN, 1"), None);
}

#[test]
fn a_source_gives_the_places_notes_and_attribution() {
    // Maps › Browse's terminalMap answer: the source's attribution wins over the props', its notes above the places
    let props = json!({"source": "terminalMap", "attribution": "Natural Earth"});
    let state = ComponentState {
        data: Some(json!({
            "points": [{"id": "view:1", "lon": -87.6298, "lat": 41.8781, "label": "Home", "source": "Your saved view"}],
            "notes": ["Not shown: Parks (it names no source)"],
            "attribution": "OpenFreeMap; © OpenMapTiles"
        })),
        ..ComponentState::default()
    };
    let text = shown(&props, &state);
    let note = text
        .iter()
        .position(|line| line == "Not shown: Parks (it names no source)");
    let places = text.iter().position(|line| line == "Places");
    assert!(note.is_some() && places.is_some() && note < places, "{text:?}");
    assert!(
        text.contains(&"▸ Home  41.87810, -87.62980  Your saved view".to_string()),
        "{text:?}"
    );
    assert_eq!(
        text.last().map(String::as_str),
        Some("Map data: OpenFreeMap; © OpenMapTiles")
    );
}

#[test]
fn with_a_source_a_postal_code_is_searched_and_the_overlays_listed() {
    // Maps › Browse: a postal search's places first, the saved places (a shared one read-only), then the overlays
    let props = json!({"source": "terminalMap"});
    let mut state = ComponentState {
        data: Some(json!({
            "points": [
                {"id": "postal:0", "lon": -71.1, "lat": 42.35, "label": "02134 Allston", "precision": "Approximate location — postal area", "source": "Census ZCTA 2026"},
                {"id": "place:1:a", "lon": -87.6298, "lat": 41.8781, "label": "Office", "source": "Shared by Ann · read-only"}
            ],
            "postalSearch": {"query": "US 02134", "count": 1, "reason": null},
            "overlays": [{"label": "Hikes", "details": "Yours · 12 features"}, {"label": "Ann's places", "details": "Shared by Ann · read-only · 3 features"}]
        })),
        ..ComponentState::default()
    };
    let text = shown(&props, &state);
    let search = text
        .iter()
        .position(|line| line == "Postal search US 02134: 1 approximate place, listed first");
    let places = text.iter().position(|line| line == "Places");
    assert!(search.is_some() && search < places, "{text:?}");
    assert!(
        text.contains(
            &"▸ 02134 Allston  42.35000, -71.10000  Approximate location — postal area · Census ZCTA 2026".to_string()
        ),
        "{text:?}"
    );
    assert!(
        text.contains(&"  Office  41.87810, -87.62980  Shared by Ann · read-only".to_string()),
        "{text:?}"
    );
    let overlays = text
        .iter()
        .position(|line| line == "Overlays")
        .expect("the overlays are listed");
    assert_eq!(text[overlays + 1], "  Hikes  Yours · 12 features");
    assert_eq!(
        text[overlays + 2],
        "  Ann's places  Shared by Ann · read-only · 3 features"
    );
    // s: type a code and its country (a character no code has is not taken); Enter asks the source again with it
    state.at = 1;
    Map.key(&props, &mut state, &press(Key::Char('s')), false).unwrap();
    for ch in "ca k1a?".chars() {
        Map.key(&props, &mut state, &press(Key::Char(ch)), false).unwrap();
    }
    Map.paste(&props, &mut state, " 0b1<").unwrap();
    assert!(
        shown(&props, &state).contains(&"Postal code (and its country, such as US 02134): ca k1a 0b1▏".to_string()),
        "{:?}",
        shown(&props, &state)
    );
    assert_eq!(
        Map.key(&props, &mut state, &press(Key::Char('c')), false),
        Some(Vec::new()),
        "typing, not copying"
    );
    // a chord is the screen's: nothing typed
    assert_eq!(
        Map.key(&props, &mut state, &KeyPress::with_ctrl(Key::Char('u')), false),
        None
    );
    assert_eq!(
        Map.key(&props, &mut state, &KeyPress::with_alt(Key::Char('x')), false),
        None
    );
    Map.key(&props, &mut state, &press(Key::Backspace), false).unwrap();
    assert_eq!(
        Map.key(&props, &mut state, &press(Key::Enter), false),
        Some(vec![Act::Reload])
    );
    assert_eq!(state.parameters.get("postal").map(String::as_str), Some("ca k1a 0b1"));
    assert_eq!(state.at, 0, "the first place is the search's");
    let (source, parameters) = Map.source(&props, &state).unwrap();
    assert_eq!(source, "terminalMap");
    assert_eq!(parameters.get("postal").map(String::as_str), Some("ca k1a 0b1"));
    // nothing found says why; an empty search takes the postal places away
    state.data = Some(
        json!({"points": [], "postalSearch": {"query": "ZZ 1", "count": 0, "reason": "No imported Geo data has this postal code"}}),
    );
    let text = shown(&props, &state);
    assert!(
        text.contains(&"Postal search ZZ 1: nothing found".to_string()),
        "{text:?}"
    );
    assert!(
        text.contains(&"No imported Geo data has this postal code".to_string()),
        "{text:?}"
    );
    Map.key(&props, &mut state, &press(Key::Char('s')), false).unwrap();
    for _ in 0..12 {
        Map.key(&props, &mut state, &press(Key::Backspace), false).unwrap();
    }
    assert_eq!(
        Map.key(&props, &mut state, &press(Key::Enter), false),
        Some(vec![Act::Reload])
    );
    assert!(!state.parameters.contains_key("postal"));
    // a map with no source (a map-view's props) offers no search
    assert_eq!(
        Map.key(&json!({"points": []}), &mut state, &press(Key::Char('s')), false),
        None
    );
}
