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
    for el in elements {
        let id = el["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("board element needs an id")?;
        let (v, Reverse(nonce)) = rank(el);
        let key = json!([id, v, nonce]).to_string();
        element_map.insert(&mut tx, key, any(el));
    }
    if let Some(entries) = scene["files"].as_object() {
        if entries.len() > limit("board@max-images", 200) {
            return Err("board exceeds its image limit".into());
        }
        for (id, file) in entries {
            let file = any(file);
            if !attachment(&file) {
                return Err("board images must be stored as attachments".into());
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
        if let Some(files) = tx.get_map("files") {
            if files.len(&tx) as usize > limit("board@max-images", 200) {
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

fn elements(tx: &impl ReadTxn) -> Vec<Value> {
    let mut winners: BTreeMap<String, Value> = BTreeMap::new();
    for el in entries(tx, "elements").into_values() {
        let Some(id) = el["id"].as_str() else { continue };
        if winners.get(id).is_none_or(|old| rank(&el) > rank(old)) {
            winners.insert(id.to_string(), el);
        }
    }
    let mut elements: Vec<_> = winners.into_values().filter(|e| e["isDeleted"] != true).collect();
    elements.sort_by(|a, b| {
        a["index"]
            .as_str()
            .cmp(&b["index"].as_str())
            .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
    });
    elements
}

pub(crate) fn render(doc: &Doc) -> String {
    let tx = doc.transact();
    let elements = elements(&tx)
        .into_iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(",\n");
    let app = entries(&tx, "app")
        .into_iter()
        .filter(|(key, _)| board_setting(key))
        .collect::<BTreeMap<_, _>>();
    format!("{{\"type\":\"excalidraw\",\"version\":2,\n\"elements\":[\n{elements}\n],\n\"appState\":{},\n\"files\":{}\n}}\n", json!(app), json!(entries(&tx, "files")))
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
            {"id":"gone","index":"a4","version":5,"versionNonce":100,"isDeleted":false},
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
        assert!(first.contains("Heading") && !first.contains("gone"));
        assert!(first.find("Heading").unwrap() < first.find("New").unwrap());
        let restored = Doc::new();
        seed(&restored, &first).unwrap();
        assert_eq!(render(&restored), first);
    }
}
