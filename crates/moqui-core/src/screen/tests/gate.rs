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

//! Write interception (plans/LLM-A2A-Assist.md 4.7): the transition gate holds a write and runs an approved one, the
//! read fence of a render refuses a write, and the observer sees each transition, the HTTP ones as a browser's.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use moqui_lib::object::Value;

use super::{request, World};
use crate::screen::{
    held_write_hash, install_transition_observer, make_render, serve_at, RenderOrigin, TransitionGate,
    TransitionObserver, TransitionOutcome,
};

/// A screen whose `save` writes an `EnumerationType` (its always-action marks the run too), a `saveAt` that takes the
/// id from the path, a read-only `peek` with actions, and a `go` that only redirects.
fn gate_screens(name: &str) -> (PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("moqui-screen-gate-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Gate")).unwrap();
    std::fs::write(
        dir.join("Gate.xml"),
        r#"<screen require-authentication="false">
            <always-actions><if condition="mark"><then>
                <script>const made = ec.entity.makeValue('moqui.basic.EnumerationType'); made.set('enumTypeId', mark + '_ALWAYS');
                    made.create()</script>
            </then></if></always-actions>
            <transition name="save"><service-call name="create#moqui.basic.EnumerationType"/>
                <default-response url="Done"/></transition>
            <transition name="saveAt"><path-parameter name="enumTypeId"/>
                <service-call name="create#moqui.basic.EnumerationType"/><default-response url="Done"/></transition>
            <transition name="peek" read-only="true"><actions><set field="seen" value="y"/></actions>
                <default-response url="Done"/></transition>
            <transition name="go"><default-response url="Done"/></transition>
            <transition name="lookup"><actions><entity-find-count entity-name="moqui.basic.EnumerationType" count-field="found"/>
                <script>ec.web.sendJsonResponse({found: found})</script></actions><default-response type="none"/></transition>
            <subscreens default-item="Done"/>
            <widgets><label text="gate screen"/></widgets></screen>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("Gate/Done.xml"),
        r#"<screen require-authentication="false"><widgets><label text="done"/></widgets></screen>"#,
    )
    .unwrap();
    let root = dir.join("Gate.xml").to_string_lossy().into_owned();
    (dir, root)
}

fn save_parameters(id: &str) -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("enumTypeId".to_string(), Value::string(id)),
        ("description".to_string(), Value::string("from the gate")),
        ("moquiSessionToken".to_string(), Value::string("ignored")),
    ])
}

/// The `argsHash` of a 409 answer.
fn args_hash_of(held: &crate::screen::RenderOutput) -> String {
    assert_eq!(held.status, 409, "{}", held.body_text());
    let body: serde_json::Value = serde_json::from_str(&held.body_text()).unwrap();
    body["heldWrite"]["argsHash"].as_str().unwrap().to_string()
}

fn exists(world: &World, id: &str) -> bool {
    world
        .facade
        .find("moqui.basic.EnumerationType")
        .unwrap()
        .eq("enumTypeId", Value::string(id))
        .one()
        .unwrap()
        .is_some()
}

#[test]
fn a_held_write_runs_nothing_and_an_approved_one_runs_only_when_its_hash_matches() {
    let world = World::open("");
    let (dir, root) = gate_screens("hold");
    let post = |path: &str, id: &str, gate: TransitionGate| {
        let mut parameters = save_parameters(id);
        parameters.insert("mark".to_string(), Value::string(id));
        make_render(&world.factory)
            .root_screen(root.clone())
            .screen_path(path)
            .method("POST")
            .parameters(parameters)
            .origin(RenderOrigin::Agent {
                agent_id: "assist".to_string(),
                run_id: Some("R1".to_string()),
            })
            .transition_gate(gate)
            .render()
            .unwrap()
    };

    // held: 409 with what would run, and nothing ran, not even the always-action
    let held = post("/save", "GATE_HELD", TransitionGate::Hold);
    assert_eq!(held.status, 409, "{}", held.body_text());
    let body: serde_json::Value = serde_json::from_str(&held.body_text()).unwrap();
    let write = &body["heldWrite"];
    assert_eq!(write["screenLocation"], root.as_str());
    assert_eq!(write["transition"], "save");
    assert_eq!(write["service"], "create#moqui.basic.EnumerationType");
    assert_eq!(write["authzAction"], "create");
    assert_eq!(write["method"], "POST");
    assert_eq!(write["parameters"]["enumTypeId"], "GATE_HELD");
    assert!(write["parameters"].get("moquiSessionToken").is_none(), "{write}");
    assert!(body.get("approvalMismatch").is_none(), "{body}");
    assert!(!exists(&world, "GATE_HELD") && !exists(&world, "GATE_HELD_ALWAYS"));

    // a GET link to the write is held the same way; a read-only transition and one with no actions run
    let link = make_render(&world.factory)
        .root_screen(root.clone())
        .screen_path("/save")
        .parameters(save_parameters("GATE_LINK"))
        .transition_gate(TransitionGate::Hold)
        .render()
        .unwrap();
    assert_eq!(link.status, 409, "{}", link.body_text());
    assert!(!exists(&world, "GATE_LINK"));
    for path in ["/peek", "/go"] {
        let ran = make_render(&world.factory)
            .root_screen(root.clone())
            .screen_path(path)
            .method("POST")
            .transition_gate(TransitionGate::Hold)
            .render()
            .unwrap();
        assert_eq!(
            ran.header("Location"),
            Some("/Done"),
            "{path}: {} {}",
            ran.status,
            ran.body_text()
        );
    }
    // a gate fences the rest of the render: the read-only transition's path may not write its always-action row
    let peek = make_render(&world.factory)
        .root_screen(root.clone())
        .screen_path("/peek")
        .method("POST")
        .parameters(BTreeMap::from([("mark".to_string(), Value::string("GATE_PEEK"))]))
        .transition_gate(TransitionGate::Hold)
        .render();
    let refused = peek
        .map(|output| output.body_text())
        .unwrap_or_else(|error| error.to_string());
    assert!(refused.contains(crate::entity::fence::FENCE_ERROR), "{refused}");
    assert!(!exists(&world, "GATE_PEEK_ALWAYS"));

    // approved with another write's hash: held, with the mismatch named
    let other = args_hash_of(&post("/save", "GATE_OTHER", TransitionGate::Hold));
    let mismatch = post(
        "/save",
        "GATE_OK",
        TransitionGate::Approved {
            approval_id: "AP1".to_string(),
            args_hash: other,
        },
    );
    assert_eq!(mismatch.status, 409, "{}", mismatch.body_text());
    let body: serde_json::Value = serde_json::from_str(&mismatch.body_text()).unwrap();
    assert_eq!(body["approvalMismatch"]["approvalId"], "AP1");
    assert!(!exists(&world, "GATE_OK"));

    // approved with the hash the held answer gave: it runs, once, as any transition
    let held = post("/save", "GATE_OK", TransitionGate::Hold);
    let body: serde_json::Value = serde_json::from_str(&held.body_text()).unwrap();
    let args_hash = body["heldWrite"]["argsHash"].as_str().unwrap().to_string();
    assert_eq!(body["heldWrite"]["path"], "/save", "{body}");
    let write: crate::screen::HeldWrite = crate::screen::HeldWrite {
        screen_location: root.clone(),
        transition: "save".to_string(),
        service: None,
        authz_action: String::new(),
        method: "POST".to_string(),
        path: "/save".to_string(),
        extra_path: Vec::new(),
        path_parameters: serde_json::Map::new(),
        inline_parameters: serde_json::Map::new(),
        parameters: body["heldWrite"]["parameters"].as_object().unwrap().clone(),
    };
    assert_eq!(
        args_hash,
        held_write_hash(&write),
        "the hash does not cover what is only shown"
    );
    let ran = post(
        "/save",
        "GATE_OK",
        TransitionGate::Approved {
            approval_id: "AP2".to_string(),
            args_hash,
        },
    );
    assert_eq!(
        ran.header("Location"),
        Some("/Done"),
        "{} {}",
        ran.status,
        ran.body_text()
    );
    assert!(exists(&world, "GATE_OK") && exists(&world, "GATE_OK_ALWAYS"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_fenced_render_reads_refuses_a_write_and_lets_an_approved_one_through() {
    let world = World::open("");
    let (dir, root) = gate_screens("fence");
    let fenced = |path: &str, id: &str, gate: TransitionGate| {
        make_render(&world.factory)
            .root_screen(root.clone())
            .screen_path(path)
            .method("POST")
            .parameters(save_parameters(id))
            .transition_gate(gate)
            .write_fence(true)
            .render()
    };
    // a screen renders under the fence
    let screen = make_render(&world.factory)
        .root_screen(root.clone())
        .screen_path("/Done")
        .render_mode("text")
        .write_fence(true)
        .render()
        .unwrap();
    assert_eq!(screen.status, 200, "{}", screen.body_text());
    assert!(!crate::entity::fence::active(), "the fence ends with the render");

    // with no gate the write meets the fence: it fails and nothing is written; the error is in this context's messages
    let context = world.factory.new_context();
    let entered = context.enter();
    let refused = fenced("/save", "GATE_FENCED", TransitionGate::Open);
    let text = match &refused {
        Ok(output) => format!("{} {}", output.status, output.body_text()),
        Err(error) => error.to_string(),
    };
    let errors =
        crate::context::ExecutionContext::with_current(|ec| ec.message().errors().join("\n")).unwrap_or_default();
    assert!(
        text.contains(crate::entity::fence::FENCE_ERROR) || errors.contains(crate::entity::fence::FENCE_ERROR),
        "{text} {errors}"
    );
    assert!(!exists(&world, "GATE_FENCED"));
    drop(entered);

    // approved, it runs outside the fence
    let args_hash = args_hash_of(&fenced("/save", "GATE_LIFTED", TransitionGate::Hold).unwrap());
    let approved = fenced(
        "/save",
        "GATE_LIFTED",
        TransitionGate::Approved {
            approval_id: "AP3".to_string(),
            args_hash,
        },
    )
    .unwrap();
    assert_eq!(
        approved.header("Location"),
        Some("/Done"),
        "{} {}",
        approved.status,
        approved.body_text()
    );
    assert!(exists(&world, "GATE_LIFTED"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A lookup transition with actions that only read (a drop-down's options) and no `read-only` is a write to the gate;
/// with the gate open and the fence on, as an agent's lookups render, it runs fenced and answers, while a transition
/// that writes fails at the fence (the test above).
#[test]
fn an_open_gate_with_the_fence_runs_a_lookup_that_only_reads() {
    let world = World::open("");
    let (dir, root) = gate_screens("lookup");
    let render = |gate: TransitionGate| {
        make_render(&world.factory)
            .root_screen(root.clone())
            .screen_path("/lookup")
            .header("Accept", "application/json")
            .origin(RenderOrigin::Agent {
                agent_id: "assist".to_string(),
                run_id: None,
            })
            .transition_gate(gate)
            .write_fence(true)
            .render()
            .unwrap()
    };
    let held = render(TransitionGate::Hold);
    assert_eq!(held.status, 409, "the gate would hold it: {}", held.body_text());
    let looked = render(TransitionGate::Open);
    assert_eq!(looked.status, 200, "{}", looked.body_text());
    let body: serde_json::Value = serde_json::from_str(&looked.body_text()).unwrap();
    assert!(body["found"].is_number(), "{body}");
    assert!(!crate::entity::fence::active());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_approval_covers_its_extra_path_and_inline_parameters() {
    let world = World::open("");
    let (dir, root) = gate_screens("path");
    let render = |path: &str, gate: TransitionGate| {
        make_render(&world.factory)
            .root_screen(root.clone())
            .screen_path(path)
            .method("POST")
            .parameters(BTreeMap::from([("description".to_string(), Value::string("by path"))]))
            .transition_gate(gate)
            .render()
            .unwrap()
    };
    let approved = |args_hash: &str| TransitionGate::Approved {
        approval_id: "AP_PATH".to_string(),
        args_hash: args_hash.to_string(),
    };

    // the held answer shows the extra path, the path parameter it binds and the inline parameters
    let held = render("/saveAt/GATE_P1", TransitionGate::Hold);
    let body: serde_json::Value = serde_json::from_str(&held.body_text()).unwrap();
    let write = &body["heldWrite"];
    assert_eq!(write["extraPath"], serde_json::json!(["GATE_P1"]), "{write}");
    assert_eq!(write["pathParameters"]["enumTypeId"], "GATE_P1", "{write}");
    let p1 = args_hash_of(&held);

    // the approval of GATE_P1 does not run GATE_P2
    let other = render("/saveAt/GATE_P2", approved(&p1));
    assert_eq!(other.status, 409, "{}", other.body_text());
    assert!(!exists(&world, "GATE_P2"));

    // an inline parameter is part of the write: an approval without it, or with another value, does not run it
    let inline_held = render("/save/~enumTypeId=GATE_INLINE", TransitionGate::Hold);
    let inline_body: serde_json::Value = serde_json::from_str(&inline_held.body_text()).unwrap();
    assert_eq!(
        inline_body["heldWrite"]["inlineParameters"]["enumTypeId"], "GATE_INLINE",
        "{inline_body}"
    );
    let inline = args_hash_of(&inline_held);
    let plain = args_hash_of(&render("/save", TransitionGate::Hold));
    assert_ne!(inline, plain);
    for path in ["/save/~enumTypeId=GATE_INLINE2", "/save?enumTypeId=GATE_INLINE2"] {
        let refused = render(path, approved(&inline));
        assert_eq!(refused.status, 409, "{path}: {}", refused.body_text());
    }
    assert!(!exists(&world, "GATE_INLINE2"));
    let refused = render("/save/~enumTypeId=GATE_INLINE", approved(&plain));
    assert_eq!(refused.status, 409, "{}", refused.body_text());
    assert!(!exists(&world, "GATE_INLINE"));

    // each runs with its own approval
    let ran = render("/saveAt/GATE_P1", approved(&p1));
    assert_eq!(
        ran.header("Location"),
        Some("/Done"),
        "{} {}",
        ran.status,
        ran.body_text()
    );
    assert!(exists(&world, "GATE_P1"));
    let ran = render("/save/~enumTypeId=GATE_INLINE", approved(&inline));
    assert_eq!(
        ran.header("Location"),
        Some("/Done"),
        "{} {}",
        ran.status,
        ran.body_text()
    );
    assert!(exists(&world, "GATE_INLINE"));
    let _ = std::fs::remove_dir_all(&dir);
}

struct Recorder {
    root: String,
    seen: Mutex<Vec<TransitionOutcome>>,
}

impl TransitionObserver for Recorder {
    fn wants(&self, _user_id: Option<&str>) -> bool {
        true
    }

    fn observed(&self, outcome: &TransitionOutcome) {
        if outcome.screen_location == self.root {
            self.seen.lock().unwrap().push(outcome.clone());
        }
    }
}

#[test]
fn the_observer_sees_held_and_run_transitions_and_an_http_request_is_a_browser_with_no_gate() {
    let world = World::open("");
    let (dir, root) = gate_screens("observer");
    let recorder = Arc::new(Recorder {
        root: root.clone(),
        seen: Mutex::new(Vec::new()),
    });
    install_transition_observer(Arc::clone(&recorder) as Arc<dyn TransitionObserver>);
    let held = make_render(&world.factory)
        .root_screen(root.clone())
        .screen_path("/save")
        .method("POST")
        .parameters(save_parameters("GATE_SEEN"))
        .origin(RenderOrigin::Agent {
            agent_id: "assist".to_string(),
            run_id: None,
        })
        .transition_gate(TransitionGate::Hold)
        .render()
        .unwrap();
    assert_eq!(held.status, 409);
    // over HTTP the origin is the browser's
    let served = serve_at(&world.factory, request("GET", "/peek"), &root);
    assert_eq!(served.status, 302, "{}", served.body_text());
    crate::screen::clear_transition_observer();
    let seen = recorder.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert!(seen[0].held && seen[0].status == 409, "{:?}", seen[0]);
    assert_eq!(seen[0].transition, "save");
    assert_eq!(seen[1].transition, "peek");
    assert_eq!(
        seen[0].parameters.get("enumTypeId").and_then(|value| value.as_str()),
        Some("GATE_SEEN")
    );
    assert!(matches!(seen[0].origin, RenderOrigin::Agent { .. }));
    assert!(!seen[1].held, "{:?}", seen[1]);
    assert_eq!(seen[1].origin, RenderOrigin::Browser);
    assert!(seen[1].origin.is_interactive() && !seen[0].origin.is_interactive());
    assert_eq!(seen[1].location.as_deref(), Some("/Done"), "{:?}", seen[1]);
    let _ = std::fs::remove_dir_all(&dir);
}
