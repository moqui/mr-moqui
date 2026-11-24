//! A wiki board stores each scene element as one JSON object in Yjs. Concurrent versions
//! use distinct map keys so that Excalidraw's version order survives Yjs map conflict resolution.
use std::cmp::Reverse;
use std::collections::BTreeMap;

use serde_json::{json, Value};
use yrs::updates::decoder::Decode;
use yrs::{Any, Doc, Map, Out, ReadTxn, StateVector, Transact, Update};

pub(crate) fn limit(selector: &str, default: usize) -> usize {
    moqui_core::context::conf_env::setting(selector)
        .parse()
        .ok()
        .filter(|n| *n > 0)
        .unwrap_or(default)
}

fn rank(element: &Value) -> (u64, Reverse<u64>) {
    (
        element["version"].as_f64().unwrap_or_default() as u64,
        Reverse(element["versionNonce"].as_f64().unwrap_or_default() as u64),
    )
}

fn board_setting(key: &str) -> bool {
    matches!(key, "viewBackgroundColor" | "gridSize" | "gridStep" | "gridModeEnabled")
}

/// How long a deleted element's tombstone is kept: long enough that an editor offline that long cannot bring it back.
const TOMBSTONE_MILLIS: f64 = 30.0 * 24.0 * 3600.0 * 1000.0;

/// Whether an element may be on a board. A deleted one is only a tombstone. A shown one is one of the drawing types:
/// never an embeddable, an iframe or a magic frame, which run a page of their own, and never one carrying generated
/// HTML (`customData.generationData`), which Excalidraw puts in an iframe's `srcdoc`.
pub(crate) fn allowed_element(el: &Value) -> bool {
    if !el.is_object() || !el["id"].as_str().is_some_and(|id| !id.is_empty()) {
        return false;
    }
    if el["customData"].get("generationData").is_some() {
        return false;
    }
    el["isDeleted"] == true
        || matches!(
            el["type"].as_str(),
            Some("rectangle" | "diamond" | "ellipse" | "arrow" | "line" | "freedraw" | "text" | "image" | "frame")
        )
}

/// The file ids that the board's shown image elements use.
fn used_files(tx: &impl ReadTxn) -> std::collections::BTreeSet<String> {
    elements(tx)
        .iter()
        .filter(|e| e["type"] == "image")
        .filter_map(|e| e["fileId"].as_str().map(str::to_string))
        .collect()
}

/// Seed `doc` from a saved scene. A scene that is not one (no JSON, no elements, past the size limit) is refused; within
/// one, an element with no id, an image entry that is not a page attachment, and the images past the limit are
/// logged and left out, so one bad piece does not close the whole board. A deleted element's tombstone (the scene's
/// `deleted`, written by [`render`]) goes in as deleted, so a client that still holds an older revision of it does not
/// bring it back.
pub(crate) fn seed(doc: &Doc, text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Ok(());
    }
    if text.len() > limit("board@max-document-bytes", 20 << 20) {
        return Err("board exceeds the document size limit".into());
    }
    let scene: Value = serde_json::from_str(text).map_err(|err| format!("invalid board JSON: {err}"))?;
    let elements = scene["elements"].as_array().ok_or("board needs an elements array")?;
    if scene["type"] != "excalidraw" {
        return Err("expected an Excalidraw scene".into());
    }
    let element_map = doc.get_or_insert_map("elements");
    let files = doc.get_or_insert_map("files");
    let app = doc.get_or_insert_map("app");
    let mut tx = doc.transact_mut();
    let tombstones = scene["deleted"].as_array().into_iter().flatten().map(|el| {
        json!({"id": el["id"], "type": el["type"], "version": el["version"], "versionNonce": el["versionNonce"],
            "updated": el["updated"], "isDeleted": true})
    });
    for el in elements.iter().cloned().chain(tombstones) {
        let Some(id) = el["id"].as_str().filter(|s| !s.is_empty()) else {
            tracing::warn!("board: an element with no id was left out of the board");
            continue;
        };
        if !allowed_element(&el) {
            tracing::warn!("board: element {id} of type {} was left out of the board", el["type"]);
            continue;
        }
        let (v, Reverse(nonce)) = rank(&el);
        let key = json!([id, v, nonce]).to_string();
        element_map.insert(&mut tx, key, any(&el));
    }
    if let Some(entries) = scene["files"].as_object() {
        let most = limit("board@max-images", 200);
        if entries.len() > most {
            tracing::warn!(
                "board: {} images are past the limit of {most}; those after it are left out",
                entries.len()
            );
        }
        for (id, file) in entries.iter().take(most) {
            let file = any(file);
            if !attachment(&file) {
                tracing::warn!("board: image {id} is not a page attachment and was left out");
                continue;
            }
            files.insert(&mut tx, id.as_str(), file);
        }
    }
    if let Some(settings) = scene["appState"].as_object() {
        for (key, value) in settings.iter().filter(|(key, _)| board_setting(key)) {
            app.insert(&mut tx, key.as_str(), any(value));
        }
    }
    Ok(())
}

// JavaScript numbers use IEEE-754. Explicit conversion also handles serde_json's arbitrary-precision number representation.
fn any(value: &Value) -> Any {
    match value {
        Value::Null => Any::Null,
        Value::Bool(b) => Any::Bool(*b),
        Value::String(s) => Any::from(s.as_str()),
        Value::Number(n) => Any::from(n.as_f64().unwrap_or_default()),
        Value::Array(items) => Any::Array(items.iter().map(any).collect()),
        Value::Object(obj) => Any::Map(std::sync::Arc::new(
            obj.iter().map(|(k, v)| (k.clone(), any(v))).collect(),
        )),
    }
}

fn attachment(file: &Any) -> bool {
    let Any::Map(metadata) = file else { return false };
    let Some(Any::String(name)) = metadata.get("attachmentName") else {
        return false;
    };
    let Some(Any::String(mime)) = metadata.get("mimeType") else {
        return false;
    };
    let plain_name = !matches!(name.as_ref(), "" | "." | "..") && !name.contains(['/', '\\', '#', '?']);
    let image = matches!(
        mime.as_ref(),
        "image/png" | "image/jpeg" | "image/webp" | "image/gif" | "image/svg+xml"
    );
    plain_name && image && !metadata.contains_key("dataURL")
}

/// A second Yjs document lets the server try an update without changing the canonical
/// state or its clocks if validation fails. Healthy edits are applied incrementally to
/// both documents. Only a refusal or an out-of-band merge requires copying the state.
pub(crate) fn validate_update(doc: &Doc, shadow: &mut Option<Doc>, bytes: &[u8]) -> Result<(), super::Refusal> {
    let candidate = shadow.get_or_insert_with(|| {
        let candidate = Doc::new();
        let state = doc.transact().encode_state_as_update_v1(&StateVector::default());
        // The source is yrs's own encoded document.
        super::merge_into("board validation", &candidate, &state);
        candidate
    });
    let result = (|| {
        let update = Update::decode_v1(bytes).map_err(|e| super::Refusal::new(400, e.to_string()))?;
        candidate
            .transact_mut()
            .apply_update(update)
            .map_err(|e| super::Refusal::new(400, e.to_string()))?;
        let tx = candidate.transact();
        // the revisions this update adds: each revision has a key of its own, so they are the keys the board lacks
        let known = doc.transact();
        if let Some(after) = tx.get_map("elements") {
            let before = known.get_map("elements");
            for (key, value) in after.iter(&tx) {
                if before.as_ref().is_some_and(|before| before.contains_key(&known, key)) {
                    continue;
                }
                let element = match value {
                    Out::Any(ref value) => serde_json::to_value(value).unwrap_or_default(),
                    _ => Value::Null,
                };
                if !allowed_element(&element) || !element["version"].is_number() {
                    return Err(super::Refusal::new(422, "board elements must be drawing elements"));
                }
            }
        }
        if let Some(app) = tx.get_map("app") {
            if app.keys(&tx).any(|key| !board_setting(key)) {
                return Err(super::Refusal::new(
                    422,
                    "a board shares only its background and grid settings",
                ));
            }
        }
        if let Some(files) = tx.get_map("files") {
            // the images in use count; an image whose element is gone stays in the map but not against the limit
            let most = limit("board@max-images", 200);
            if files.len(&tx) as usize > most && used_files(&tx).len() > most {
                return Err(super::Refusal::new(413, "board exceeds its image limit"));
            }
            for (_, file) in files.iter(&tx) {
                if !matches!(file, Out::Any(ref value) if attachment(value)) {
                    return Err(super::Refusal::new(422, "board images must be page attachments"));
                }
            }
        }
        Ok(())
    })();
    if result.is_err() {
        *shadow = None;
    }
    result
}

fn entries(tx: &impl ReadTxn, name: &str) -> BTreeMap<String, Value> {
    let Some(map) = tx.get_map(name) else {
        return BTreeMap::new();
    };
    map.iter(tx)
        .filter_map(|(k, v)| match v {
            Out::Any(value) => serde_json::to_value(value).ok().map(|v| (k.to_string(), v)),
            _ => None,
        })
        .collect()
}

/// Each element's winning revision, deleted ones too.
fn winners(tx: &impl ReadTxn) -> BTreeMap<String, Value> {
    let mut winners: BTreeMap<String, Value> = BTreeMap::new();
    for el in entries(tx, "elements").into_values() {
        let Some(id) = el["id"].as_str() else { continue };
        if winners.get(id).is_none_or(|old| rank(&el) > rank(old)) {
            winners.insert(id.to_string(), el);
        }
    }
    winners
}

fn elements(tx: &impl ReadTxn) -> Vec<Value> {
    let mut elements: Vec<_> = winners(tx).into_values().filter(|e| e["isDeleted"] != true).collect();
    elements.sort_by(|a, b| {
        a["index"]
            .as_str()
            .cmp(&b["index"].as_str())
            .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
    });
    elements
}

/// The scene a version saves: its elements, settings and the image entries its elements use, and a tombstone of each
/// element deleted in the last 30 days (`deleted`: its id, type, revision and time), which [`seed`] reads back as
/// deleted. A tombstone with no time (from before times were kept) stays.
pub(crate) fn render(doc: &Doc) -> String {
    render_at(doc, now_millis())
}

fn now_millis() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_millis() as f64)
}

fn render_at(doc: &Doc, now: f64) -> String {
    let tx = doc.transact();
    let elements = elements(&tx)
        .into_iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(",\n");
    let deleted: Vec<Value> = winners(&tx)
        .into_values()
        .filter(|e| e["isDeleted"] == true)
        .filter(|e| e["updated"].as_f64().is_none_or(|at| now - at < TOMBSTONE_MILLIS))
        .map(|e| {
            let mut stone =
                json!({"id": e["id"], "type": e["type"], "version": e["version"], "versionNonce": e["versionNonce"]});
            if e["updated"].is_number() {
                stone["updated"] = e["updated"].clone();
            }
            stone
        })
        .collect();
    let used = used_files(&tx);
    let files: BTreeMap<String, Value> = entries(&tx, "files")
        .into_iter()
        .filter(|(id, _)| used.contains(id))
        .collect();
    let app = entries(&tx, "app")
        .into_iter()
        .filter(|(key, _)| board_setting(key))
        .collect::<BTreeMap<_, _>>();
    format!("{{\"type\":\"excalidraw\",\"version\":2,\n\"elements\":[\n{elements}\n],\n\"appState\":{},\n\"files\":{},\n\"deleted\":{}\n}}\n", json!(app), json!(files), json!(deleted))
}

pub(crate) fn search_text(doc: &Doc) -> String {
    let mut elements = elements(&doc.transact());
    elements.sort_by(|a, b| {
        let position = |e: &Value, axis: &str| e[axis].as_f64().unwrap_or(0.0);
        position(a, "y")
            .total_cmp(&position(b, "y"))
            .then_with(|| position(a, "x").total_cmp(&position(b, "x")))
    });
    elements
        .iter()
        .filter(|e| e["type"] == "text")
        .filter_map(|e| e["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_drawings_follow_element_versions_in_either_delivery_order() {
        let update = |elements: Value| {
            let doc = Doc::new();
            let scene = json!({"type":"excalidraw","elements":elements});
            seed(&doc, &scene.to_string()).unwrap();
            let tx = doc.transact();
            tx.encode_state_as_update_v1(&StateVector::default())
        };
        let a = update(json!([
            {"id":"same","type":"text","text":"Old","index":"a3","version":1,"versionNonce":2},
            {"id":"gone","type":"rectangle","index":"a4","version":5,"versionNonce":100,"isDeleted":false},
            {"id":"first","type":"text","text":"Heading","index":"a0","version":1,"y":5}
        ]));
        let b = update(json!([
            {"id":"same","type":"text","text":"New","index":"a2","version":2,"versionNonce":8,"y":2},
            {"id":"gone","index":"a1","version":5,"versionNonce":99,"isDeleted":true}
        ]));
        let merged = |updates: [&[u8]; 2]| {
            let doc = Doc::new();
            for u in updates {
                doc.transact_mut().apply_update(Update::decode_v1(u).unwrap()).unwrap();
            }
            assert_eq!(search_text(&doc), "New\nHeading");
            render(&doc)
        };
        let first = merged([&a, &b]);
        assert_eq!(first, merged([&b, &a]));
        let scene: Value = serde_json::from_str(&first).unwrap();
        assert!(first.contains("Heading"));
        assert!(scene["elements"].as_array().unwrap().iter().all(|e| e["id"] != "gone"));
        assert_eq!(
            scene["deleted"],
            json!([{"id":"gone","type":null,"version":5,"versionNonce":99}])
        );
        assert!(first.find("Heading").unwrap() < first.find("New").unwrap());
        let restored = Doc::new();
        seed(&restored, &first).unwrap();
        assert_eq!(render(&restored), first);
        // a client still holding the element before its deletion does not bring it back into the seeded board
        restored
            .transact_mut()
            .apply_update(Update::decode_v1(&a).unwrap())
            .unwrap();
        assert!(!render(&restored).contains("\"id\":\"gone\",\"index\""));
        assert_eq!(search_text(&restored), "New\nHeading");
    }

    /// One bad piece of a saved scene is left out, not the whole board: an element with no id and an image entry that
    /// is not a page attachment. A scene that is not one is still refused.
    #[test]
    fn a_seed_leaves_out_a_bad_element_or_image_and_keeps_the_rest() {
        let doc = Doc::new();
        let scene = json!({"type":"excalidraw","elements":[
            {"type":"text","text":"No id"},
            {"id":"kept","type":"text","text":"Kept","index":"a0","version":1},
            {"id":"picture","type":"image","fileId":"photo","index":"a1","version":1}
        ],"files":{
            "inline":{"dataURL":"data:image/png;base64,AA=="},
            "photo":{"attachmentName":"photo.png","mimeType":"image/png"}
        }});
        seed(&doc, &scene.to_string()).unwrap();
        assert_eq!(search_text(&doc), "Kept");
        let saved: Value = serde_json::from_str(&render(&doc)).unwrap();
        assert_eq!(
            saved["files"],
            json!({"photo":{"attachmentName":"photo.png","mimeType":"image/png"}})
        );
        assert!(seed(&Doc::new(), "{not json").is_err());
        assert!(seed(&Doc::new(), r#"{"type":"other","elements":[]}"#).is_err());
    }

    /// An element that runs a page of its own, or carries generated HTML, never reaches a board: a seed leaves it out
    /// and an update that adds it is refused, as is an update that sets an editor setting other than the board's.
    #[test]
    fn a_board_refuses_embedded_pages_generated_html_and_editor_settings() {
        let doc = Doc::new();
        let scene = json!({"type":"excalidraw","elements":[
            {"id":"kept","type":"text","text":"Kept","index":"a0","version":1},
            {"id":"page","type":"iframe","index":"a1","version":1},
            {"id":"embed","type":"embeddable","link":"https://example.com","index":"a2","version":1},
            {"id":"magic","type":"rectangle","customData":{"generationData":{"status":"done","html":"<script>x()</script>"}},"index":"a3","version":1}
        ]});
        seed(&doc, &scene.to_string()).unwrap();
        let saved = render(&doc);
        assert!(saved.contains("Kept"));
        assert!(!saved.contains("iframe") && !saved.contains("embeddable") && !saved.contains("generationData"));

        let edit = |change: &dyn Fn(&Doc)| {
            let other = Doc::new();
            let state = doc.transact().encode_state_as_update_v1(&StateVector::default());
            other
                .transact_mut()
                .apply_update(Update::decode_v1(&state).unwrap())
                .unwrap();
            let before = other.transact().state_vector();
            change(&other);
            let update = other.transact().encode_state_as_update_v1(&before);
            validate_update(&doc, &mut None, &update)
        };
        let add = |el: Value| {
            move |d: &Doc| {
                let map = d.get_or_insert_map("elements");
                map.insert(&mut d.transact_mut(), json!([el["id"], 1, 0]).to_string(), any(&el));
            }
        };
        let page = add(json!({"id":"late","type":"iframe","version":1}));
        assert_eq!(edit(&page).unwrap_err().code, 422);
        let html =
            add(json!({"id":"late","type":"rectangle","version":1,"customData":{"generationData":{"html":"<b>"}}}));
        assert_eq!(edit(&html).unwrap_err().code, 422);
        let shape = add(json!({"id":"late","type":"rectangle","version":1}));
        assert!(edit(&shape).is_ok());
        let toast = |d: &Doc| {
            d.get_or_insert_map("app")
                .insert(&mut d.transact_mut(), "errorMessage", "Sign in again");
        };
        assert_eq!(edit(&toast).unwrap_err().code, 422);
        let grid = |d: &Doc| {
            d.get_or_insert_map("app")
                .insert(&mut d.transact_mut(), "gridModeEnabled", true);
        };
        assert!(edit(&grid).is_ok());
    }

    /// A saved scene keeps only the images its elements show, and tombstones for 30 days; an image entry left in the
    /// map by a deleted element does not count against the image limit.
    #[test]
    fn a_version_keeps_the_images_in_use_and_recent_tombstones() {
        let day = 24.0 * 3600.0 * 1000.0;
        let now = 100.0 * day;
        let doc = Doc::new();
        let scene = json!({"type":"excalidraw","elements":[
            {"id":"shown","type":"image","fileId":"a","index":"a0","version":1},
            {"id":"gone","type":"image","fileId":"b","index":"a1","version":2,"isDeleted":true,"updated":now - day},
            {"id":"old","type":"text","index":"a2","version":2,"isDeleted":true,"updated":now - 40.0 * day}
        ],"files":{
            "a":{"attachmentName":"a.png","mimeType":"image/png"},
            "b":{"attachmentName":"b.png","mimeType":"image/png"}
        }});
        seed(&doc, &scene.to_string()).unwrap();
        let saved: Value = serde_json::from_str(&render_at(&doc, now)).unwrap();
        assert_eq!(
            saved["files"],
            json!({"a":{"attachmentName":"a.png","mimeType":"image/png"}})
        );
        let stones: Vec<_> = saved["deleted"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["id"].clone())
            .collect();
        assert_eq!(stones, vec![json!("gone")]);
        assert_eq!(used_files(&doc.transact()).len(), 1);
    }
}
