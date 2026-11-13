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

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use moqui_lib::env;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;
use moqui_lib::sync;

use super::{Member, Sender};
use crate::context::{Error, ExecutionContextFactory};
use crate::entity::EntityFacade;
use crate::notify::{self, TopicMessage};
use crate::user::{now_millis, now_value, stamp_value};

const PROPS: &[&str] = &[
    "instance_id",
    "session_cookie_key",
    "cluster_tls_cert",
    "cluster_tls_key",
    "cluster_https_port",
    "cluster_base_url",
];

#[test]
fn the_serialized_cluster_limit_includes_the_envelope_and_accepts_sixteen_mib() {
    let mut message = TopicMessage {
        origin_id: "peer".into(),
        message_id: "message".into(),
        body: serde_json::Value::String(String::new()),
    };
    let overhead = super::payload_bytes(&message).unwrap().len();
    message.body = serde_json::Value::String("x".repeat(super::MAX_CLUSTER_BODY - overhead));
    assert_eq!(super::payload_bytes(&message).unwrap().len(), super::MAX_CLUSTER_BODY);
    message.body = serde_json::Value::String("x".repeat(super::MAX_CLUSTER_BODY - overhead + 1));
    assert!(super::payload_bytes(&message).is_err());
}

#[test]
fn permanent_rejections_are_not_retried_or_counted_as_a_down_peer() {
    struct Refuse(AtomicUsize);
    impl Sender for Refuse {
        fn post(&self, _: &str, _: &str, _: &[u8]) -> Result<super::PostOutcome, Error> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(super::PostOutcome::Rejected(413))
        }
    }
    let refuse = Arc::new(Refuse(AtomicUsize::new(0)));
    let sender: Arc<dyn Sender> = refuse.clone();
    assert!(super::post_batch_with_retry(
        &sender,
        "https://peer",
        &[("collab-update", b"body".to_vec())]
    ));
    assert!(super::post_batch_with_retry(
        &sender,
        "https://peer",
        &[("entity-cache-invalidate", b"next".to_vec())]
    ));
    assert_eq!(refuse.0.load(Ordering::SeqCst), 2);
    assert!(super::PostOutcome::from_status(503).is_err());
}

/// A sender that carries a whole batch in one exchange and answers for the messages up to the first that fails.
struct Batches {
    /// The batches posted, each as its payloads.
    batches: Mutex<Vec<Vec<String>>>,
    /// Payloads that fail (busy) this many more times.
    busy: Mutex<Vec<(&'static str, usize)>>,
}

impl Sender for Batches {
    fn post(&self, _: &str, _: &str, _: &[u8]) -> Result<super::PostOutcome, Error> {
        panic!("a batch sender is posted to by batch")
    }

    fn post_batch(&self, _: &str, messages: &[(&str, &[u8])]) -> super::BatchOutcome {
        let payloads: Vec<String> = messages
            .iter()
            .map(|(_, payload)| String::from_utf8_lossy(payload).into_owned())
            .collect();
        sync::lock(&self.batches).push(payloads.clone());
        let mut outcomes = Vec::new();
        for payload in payloads {
            let mut busy = sync::lock(&self.busy);
            if let Some((_, left)) = busy.iter_mut().find(|(name, left)| *name == payload && *left > 0) {
                *left -= 1;
                return super::BatchOutcome {
                    outcomes,
                    failed: Some(Error::new("busy")),
                };
            }
            drop(busy);
            outcomes.push(super::PostOutcome::Accepted);
        }
        super::BatchOutcome { outcomes, failed: None }
    }
}

fn batches(busy: &[(&'static str, usize)]) -> Arc<Batches> {
    Arc::new(Batches {
        batches: Mutex::new(Vec::new()),
        busy: Mutex::new(busy.to_vec()),
    })
}

fn messages(names: &[&str]) -> Vec<(&'static str, Vec<u8>)> {
    names
        .iter()
        .map(|name| ("collab-update", name.as_bytes().to_vec()))
        .collect()
}

/// The default `post_batch` posts one message after another and stops at the first that fails, so the rest are sent
/// again in order rather than ahead of it.
#[test]
fn the_default_batch_stops_at_the_first_failure() {
    let scripted = Scripted {
        only: "second",
        fails_left: AtomicUsize::new(1),
        calls: AtomicUsize::new(0),
    };
    let outcome = scripted.post_batch(
        "https://peer",
        &[
            ("collab-update", b"first"),
            ("collab-update", b"second"),
            ("collab-update", b"third"),
        ],
    );
    assert_eq!(outcome.outcomes, vec![super::PostOutcome::Accepted]);
    assert!(outcome.failed.is_some());
    let outcome = scripted.post_batch(
        "https://peer",
        &[("collab-update", b"second"), ("collab-update", b"third")],
    );
    assert_eq!(outcome.outcomes.len(), 2);
    assert!(outcome.failed.is_none());
}

/// What a busy peer left unapplied is sent again from where it stopped, in order; each try that gets further starts the
/// count of tries again, and a peer that takes nothing three times running is down.
#[test]
fn a_batch_is_sent_again_from_the_first_unapplied_message() {
    let sender = batches(&[("b", 2), ("d", 1)]);
    let as_sender: Arc<dyn Sender> = sender.clone();
    assert!(super::post_batch_with_retry(
        &as_sender,
        "https://peer",
        &messages(&["a", "b", "c", "d", "e"])
    ));
    let sent = sync::lock(&sender.batches).clone();
    assert_eq!(
        sent,
        vec![
            vec!["a", "b", "c", "d", "e"],
            vec!["b", "c", "d", "e"],
            vec!["b", "c", "d", "e"],
            vec!["d", "e"],
        ]
    );
    let down = batches(&[("x", 3)]);
    let as_sender: Arc<dyn Sender> = down.clone();
    assert!(!super::post_batch_with_retry(
        &as_sender,
        "https://peer",
        &messages(&["x", "y"])
    ));
    assert_eq!(sync::lock(&down.batches).len(), 3);
}

struct Clean;

impl Drop for Clean {
    fn drop(&mut self) {
        for name in PROPS {
            env::clear_property(name);
        }
        env::clear_dotenv();
    }
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    crate::user::session::cookie_property_lock()
}

struct Recording {
    calls: Mutex<Vec<Posted>>,
}

struct Posted {
    base: String,
    topic: String,
    payload: Vec<u8>,
}

impl Sender for Recording {
    fn post(&self, base_url: &str, topic: &str, payload: &[u8]) -> Result<super::PostOutcome, Error> {
        sync::lock(&self.calls).push(Posted {
            base: base_url.to_string(),
            topic: topic.to_string(),
            payload: payload.to_vec(),
        });
        Ok(super::PostOutcome::Accepted)
    }
}

fn install_recording(factory: &ExecutionContextFactory) -> Arc<Recording> {
    let recording = Arc::new(Recording {
        calls: Mutex::new(Vec::new()),
    });
    super::install_sender(factory, Arc::clone(&recording) as Arc<dyn Sender>);
    recording
}

/// Counts and fails only posts whose payload names `only`.
struct Scripted {
    only: &'static str,
    fails_left: AtomicUsize,
    calls: AtomicUsize,
}

impl Sender for Scripted {
    fn post(&self, _base_url: &str, _topic: &str, payload: &[u8]) -> Result<super::PostOutcome, Error> {
        if !String::from_utf8_lossy(payload).contains(self.only) {
            return Ok(super::PostOutcome::Accepted);
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        let left = self.fails_left.load(Ordering::SeqCst);
        if left > 0 {
            self.fails_left.fetch_sub(1, Ordering::SeqCst);
            return Err(Error::new("post failed"));
        }
        Ok(super::PostOutcome::Accepted)
    }
}

fn preset(instance_id: &str) {
    let (cert, key) = super::generate_cert_pair().expect("cert pair");
    env::set_property("instance_id", instance_id);
    env::set_property("session_cookie_key", "session-key-for-tests");
    env::set_property("cluster_tls_cert", cert);
    env::set_property("cluster_tls_key", key);
    env::set_property("cluster_https_port", "");
    env::set_property("cluster_base_url", "");
}

fn open_facade() -> (crate::context::ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    (ctx, facade)
}

fn put_member(
    facade: &EntityFacade,
    instance_id: &str,
    base_url: &str,
    heartbeat: Value,
    session_fp: &str,
    cert_fp: &str,
) {
    super::write_member(
        facade,
        &Member {
            instance_id: instance_id.to_string(),
            base_url: base_url.to_string(),
            heartbeat,
            session_fp: session_fp.to_string(),
            cert_fp: cert_fp.to_string(),
            ssh_fp: String::new(),
        },
    )
    .unwrap();
}

/// A cached entity, whose writes peers are told of.
const ROW: &str = "moqui.basic.Enumeration";

fn write_row(facade: &EntityFacade, id: &str, commit: bool) {
    facade.begin(ROW, None).unwrap();
    let mut value = facade.make_value(ROW).unwrap();
    value.set("enumId", Value::string(id));
    value.create(facade).unwrap();
    if commit {
        facade.commit(ROW).unwrap();
    } else {
        facade.rollback(ROW).unwrap();
    }
}

#[test]
fn first_start_upserts_one_member_and_does_not_post() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-first");
    let (ctx, facade) = open_facade();
    let recording = install_recording(ctx.factory());
    let _entered = ctx.enter();
    super::start(ctx.factory()).unwrap();
    super::start(ctx.factory()).unwrap();
    let members = super::load_members(&facade).unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].instance_id, "inst-first");
    assert_eq!(members[0].base_url, "https://127.0.0.1:8444");
    assert!(sync::lock(&recording.calls).is_empty());
}

#[test]
fn commit_posts_once_and_remote_apply_does_not_repost() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-local");
    let fingerprints = super::configured_fingerprints().unwrap();
    let (ctx, facade) = open_facade();
    let recording = install_recording(ctx.factory());
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    put_member(
        &facade,
        "inst-peer",
        "https://peer.example:8444/",
        now_value(),
        &fingerprints.session,
        &fingerprints.cert,
    );
    super::start(ctx.factory()).unwrap();
    super::start(ctx.factory()).unwrap();
    assert_eq!(super::load_members(&facade).unwrap().len(), 2);
    write_row(&facade, "V1", false);
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert!(sync::lock(&recording.calls).is_empty());
    write_row(&facade, "V2", true);
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    let calls = sync::lock(&recording.calls);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].base, "https://peer.example:8444");
    assert_eq!(calls[0].topic, "entity-cache-invalidate");
    assert_eq!(
        format!("{}/cluster/{}", calls[0].base, calls[0].topic),
        "https://peer.example:8444/cluster/entity-cache-invalidate"
    );
    let json: serde_json::Value = serde_json::from_slice(&calls[0].payload).unwrap();
    assert_eq!(json["origin_id"], notify::origin_id());
    assert!(!json["message_id"].as_str().unwrap_or("").is_empty());
    assert_eq!(json["body"]["entityName"], ROW);
    assert_eq!(json["body"]["primaryKey"]["enumId"], "V2");
    let message = TopicMessage {
        origin_id: json["origin_id"].as_str().unwrap().to_string(),
        message_id: json["message_id"].as_str().unwrap().to_string(),
        body: json["body"].clone(),
    };
    drop(calls);
    super::apply_remote(&facade, "entity-cache-invalidate", &message).unwrap();
    super::apply_remote(&facade, "entity-cache-invalidate", &message).unwrap();
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert_eq!(sync::lock(&recording.calls).len(), 1);
    let err = super::apply_remote(&facade, "not-a-topic", &message).unwrap_err();
    assert!(err.to_string().contains("unknown topic"));
}

/// Ending another session of the user by its visit posts `session-end` to the peers, which may hold it live; applied
/// on a peer it is not posted again.
#[test]
fn ending_a_session_by_its_visit_posts_session_end_once() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-end");
    let fingerprints = super::configured_fingerprints().unwrap();
    let (ctx, facade) = open_facade();
    let recording = install_recording(ctx.factory());
    let _entered = ctx.enter();
    put_member(
        &facade,
        "inst-end-peer",
        "https://end-peer.example:8444/",
        now_value(),
        &fingerprints.session,
        &fingerprints.cert,
    );
    super::start(ctx.factory()).unwrap();
    {
        let _authz = ctx.disable_authz();
        let mut visit = facade.make_value("moqui.server.Visit").unwrap();
        visit.set("visitId", Value::string("END_VISIT"));
        visit.set("userId", Value::string("END_USER"));
        visit.create(&facade).unwrap();
        let mut row = facade.make_value("moqui.server.HttpSession").unwrap();
        row.set("sessionId", Value::string("END_SESSION"));
        row.set("visitId", Value::string("END_VISIT"));
        row.set("userId", Value::string("END_USER"));
        row.create(&facade).unwrap();
    }
    crate::user::session::set_user("END_USER", "end-user");
    assert_eq!(crate::user::session::end_user_visit("END_VISIT").unwrap(), 1);
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    let session_ends = |recording: &Recording| -> Vec<serde_json::Value> {
        sync::lock(&recording.calls)
            .iter()
            .filter(|call| call.topic == notify::SESSION_END)
            .map(|call| serde_json::from_slice(&call.payload).unwrap())
            .collect()
    };
    let posted = session_ends(&recording);
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0]["body"]["sessionIds"], serde_json::json!(["END_SESSION"]));
    let message = TopicMessage {
        origin_id: "inst-end-peer-origin".to_string(),
        message_id: posted[0]["message_id"].as_str().unwrap().to_string(),
        body: posted[0]["body"].clone(),
    };
    super::apply_remote(&facade, notify::SESSION_END, &message).unwrap();
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert_eq!(session_ends(&recording).len(), 1, "a remote apply is not posted again");
}

/// A message published without a `message_id` (as co-editing's updates were) is posted with one: a peer refuses a
/// post without it.
#[test]
fn a_message_without_an_id_is_posted_with_one() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-noid");
    let fingerprints = super::configured_fingerprints().unwrap();
    let (ctx, facade) = open_facade();
    let recording = install_recording(ctx.factory());
    let _entered = ctx.enter();
    put_member(
        &facade,
        "inst-noid-peer",
        "https://noid-peer.example:8444/",
        now_value(),
        &fingerprints.session,
        &fingerprints.cert,
    );
    super::start(ctx.factory()).unwrap();
    let message = TopicMessage {
        origin_id: notify::origin_id().to_string(),
        message_id: String::new(),
        body: serde_json::json!({ "doc": "D1" }),
    };
    super::publish(&facade, notify::COLLAB_UPDATE, &message).unwrap();
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    let posted: Vec<serde_json::Value> = sync::lock(&recording.calls)
        .iter()
        .filter(|call| call.topic == notify::COLLAB_UPDATE)
        .map(|call| serde_json::from_slice(&call.payload).unwrap())
        .collect();
    assert_eq!(posted.len(), 1);
    assert!(!posted[0]["message_id"].as_str().unwrap_or_default().is_empty());
    assert_eq!(posted[0]["body"]["doc"], "D1");
}

#[test]
fn a_second_factory_does_not_post_through_the_first_ones_sender() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-two");
    let fingerprints = super::configured_fingerprints().unwrap();
    let (first, first_facade) = open_facade();
    let first_recording = install_recording(first.factory());
    let (second, second_facade) = open_facade();
    let second_recording = install_recording(second.factory());
    for (ctx, facade, peer) in [
        (&first, &first_facade, "https://first-peer.example:8444"),
        (&second, &second_facade, "https://second-peer.example:8444"),
    ] {
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        put_member(
            facade,
            "inst-two-peer",
            peer,
            now_value(),
            &fingerprints.session,
            &fingerprints.cert,
        );
        super::start(ctx.factory()).unwrap();
    }
    {
        let _entered = second.enter();
        let _authz = second.disable_authz();
        write_row(&second_facade, "TWO-V1", true);
    }
    assert!(super::wait_for_sends(first.factory(), Duration::from_secs(5)));
    assert!(super::wait_for_sends(second.factory(), Duration::from_secs(5)));
    assert!(sync::lock(&first_recording.calls).is_empty());
    let calls = sync::lock(&second_recording.calls);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].base, "https://second-peer.example:8444");
}

#[test]
fn a_live_peer_with_a_different_fingerprint_fails_start() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-mismatch");
    let fingerprints = super::configured_fingerprints().unwrap();
    let cert = env::get_prop_or_env("cluster_tls_cert");
    let (ctx, facade) = open_facade();
    let _entered = ctx.enter();
    put_member(
        &facade,
        "inst-other-session",
        "https://peer.example:8444",
        now_value(),
        "different-session",
        &fingerprints.cert,
    );
    let err = super::start(ctx.factory()).unwrap_err();
    let shown = format!("{err}");
    assert!(shown.contains("session_cookie_key"));
    assert!(!shown.contains("session-key-for-tests"));
    assert!(!shown.contains(&cert));

    let (ctx, facade) = open_facade();
    let _entered = ctx.enter();
    put_member(
        &facade,
        "inst-other-cert",
        "https://peer.example:8444",
        now_value(),
        &fingerprints.session,
        "different-cert",
    );
    let err = super::start(ctx.factory()).unwrap_err();
    let shown = format!("{err}");
    assert!(shown.contains("cluster_tls_cert"));
    assert!(!shown.contains(&cert));
    assert!(!shown.contains("session-key-for-tests"));
}

#[test]
fn a_stale_peer_is_not_posted() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-stale");
    let fingerprints = super::configured_fingerprints().unwrap();
    let (ctx, facade) = open_facade();
    let recording = install_recording(ctx.factory());
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    super::start(ctx.factory()).unwrap();
    let stale = now_millis() - 4 * 60 * 1000;
    put_member(
        &facade,
        "inst-old",
        "https://old.example:8443",
        stamp_value(stale),
        &fingerprints.session,
        &fingerprints.cert,
    );
    put_member(
        &facade,
        "inst-null",
        "https://null.example:8443",
        Value::Null,
        &fingerprints.session,
        &fingerprints.cert,
    );
    write_row(&facade, "V3", true);
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert!(sync::lock(&recording.calls).is_empty());
}

fn heartbeat_of(facade: &EntityFacade, instance_id: &str) -> Option<i64> {
    super::load_members(facade)
        .unwrap()
        .into_iter()
        .find(|member| member.instance_id == instance_id)
        .and_then(|member| member.heartbeat)
}

fn delete_member_row(facade: &EntityFacade, instance_id: &str) {
    let mut row = facade.make_value(super::MEMBER).unwrap();
    row.set("instanceId", Value::string(instance_id));
    row.delete(facade).unwrap();
    assert!(heartbeat_of(facade, instance_id).is_none());
}

#[test]
fn a_heartbeat_moves_the_member_and_a_tick_runs_the_stand_in() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-tick");
    let pools = crate::service::ServicePools::bounded(1, 4);
    let ctx = ExecutionContextFactory::test_context_with("", pools);
    let hit = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&hit);
    ctx.factory()
        .service_pools()
        .add_tick_hook(move || flag.store(true, Ordering::SeqCst));
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    super::start(ctx.factory()).unwrap();
    let before = heartbeat_of(&facade, "inst-tick").unwrap();
    std::thread::sleep(Duration::from_millis(50));
    ctx.factory().service_pools().tick();
    assert!(hit.load(Ordering::SeqCst));
    super::heartbeat(ctx.factory());
    let after = heartbeat_of(&facade, "inst-tick").unwrap();
    assert!(after > before, "heartbeat {before} did not move to {after}");
}

/// Peers delete the row of a member whose heartbeat stopped for ten windows; its next heartbeat writes it again, with
/// the fingerprints and base URL it started with.
#[test]
fn a_member_whose_row_was_deleted_registers_again_on_the_next_heartbeat() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-gone");
    let (ctx, facade) = open_facade();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    super::start(ctx.factory()).unwrap();
    let started = super::load_members(&facade).unwrap();
    delete_member_row(&facade, "inst-gone");
    super::heartbeat(ctx.factory());
    let members = super::load_members(&facade).unwrap();
    let again = members.iter().find(|member| member.instance_id == "inst-gone").unwrap();
    let first = started.iter().find(|member| member.instance_id == "inst-gone").unwrap();
    assert!(again.heartbeat.is_some());
    assert_eq!(again.base_url, first.base_url);
    assert_eq!(again.session_fp, first.session_fp);
    assert_eq!(again.cert_fp, first.cert_fp);
}

/// The heartbeat has a thread of its own: a scheduler hook that does not return (a long job pass, a slow session
/// flush) does not stop it. The row is deleted while the hook is held, so only a heartbeat brings it back.
#[test]
fn the_heartbeat_runs_while_a_scheduler_hook_is_blocked() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-busy");
    let pools = crate::service::ServicePools::bounded(1, 4);
    let ctx = ExecutionContextFactory::test_context_with("", pools);
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = Mutex::new(release_rx);
    ctx.factory().service_pools().add_tick_hook(move || {
        let _ = entered_tx.send(());
        let _ = sync::lock(&release_rx).recv();
    });
    super::set_heartbeat_interval(ctx.factory(), Duration::from_millis(50));
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    super::start(ctx.factory()).unwrap();
    let ticking = {
        let factory = Arc::clone(ctx.factory());
        std::thread::spawn(move || {
            factory.service_pools().tick();
        })
    };
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    delete_member_row(&facade, "inst-busy");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while heartbeat_of(&facade, "inst-busy").is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "no heartbeat while the hook was held"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    release_tx.send(()).unwrap();
    ticking.join().unwrap();
}

#[test]
fn a_sender_retries_three_times_and_an_always_failing_sender_does_not_panic() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-retry");
    let fingerprints = super::configured_fingerprints().unwrap();
    let scripted = Arc::new(Scripted {
        only: "RETRY-V4",
        fails_left: AtomicUsize::new(2),
        calls: AtomicUsize::new(0),
    });
    let (ctx, facade) = open_facade();
    super::install_sender(ctx.factory(), Arc::clone(&scripted) as Arc<dyn Sender>);
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    super::start(ctx.factory()).unwrap();
    put_member(
        &facade,
        "inst-retry-peer",
        "https://retry.example:8443",
        now_value(),
        &fingerprints.session,
        &fingerprints.cert,
    );
    write_row(&facade, "RETRY-V4", true);
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert_eq!(scripted.calls.load(Ordering::SeqCst), 3);

    let always = Arc::new(Scripted {
        only: "RETRY-V5",
        fails_left: AtomicUsize::new(9),
        calls: AtomicUsize::new(0),
    });
    super::install_sender(ctx.factory(), Arc::clone(&always) as Arc<dyn Sender>);
    write_row(&facade, "RETRY-V5", true);
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert_eq!(always.calls.load(Ordering::SeqCst), 3);

    *super::lock(&super::ClusterState::of(ctx.factory()).sender) = None;
    write_row(&facade, "RETRY-V6", true);
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
}

#[test]
fn many_writes_in_one_wait_reach_a_peer_as_one_invalidation_covering_them_all_and_the_commit_does_not_wait() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-batch");
    let fingerprints = super::configured_fingerprints().unwrap();
    // A peer that takes a while to answer: the commit returns long before the posts are done.
    struct Slow(Mutex<Vec<String>>);
    impl Sender for Slow {
        fn post(&self, _base_url: &str, _topic: &str, payload: &[u8]) -> Result<super::PostOutcome, Error> {
            std::thread::sleep(Duration::from_millis(200));
            sync::lock(&self.0).push(String::from_utf8_lossy(payload).to_string());
            Ok(super::PostOutcome::Accepted)
        }
    }
    let slow = Arc::new(Slow(Mutex::new(Vec::new())));
    let (ctx, facade) = open_facade();
    super::install_sender(ctx.factory(), Arc::clone(&slow) as Arc<dyn Sender>);
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    super::start(ctx.factory()).unwrap();
    put_member(
        &facade,
        "inst-batch-peer",
        "https://batch.example:8443",
        now_value(),
        &fingerprints.session,
        &fingerprints.cert,
    );
    let started = std::time::Instant::now();
    facade.begin(ROW, None).unwrap();
    for index in 0..200 {
        let mut value = facade.make_value(ROW).unwrap();
        value.set("enumId", Value::string(format!("BATCH-{index}")));
        value.create(&facade).unwrap();
    }
    facade.commit(ROW).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the commit waited on the peer: {:?}",
        started.elapsed()
    );
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(10)));
    let posted = sync::lock(&slow.0);
    let bodies: Vec<serde_json::Value> = posted
        .iter()
        .map(|text| serde_json::from_str::<serde_json::Value>(text).unwrap()["body"].clone())
        .filter(|body| body["entityName"] == ROW)
        .collect();
    // The first commit callback may be taken alone before the rest are queued; the rest arrive together.
    assert!((1..=2).contains(&bodies.len()), "{} posts for 200 writes", bodies.len());
    // Every write is covered: by its key, or by a message for the whole entity (past the keys one message carries).
    let whole = bodies
        .iter()
        .any(|body| body.get("primaryKey").is_none() && body.get("primaryKeys").is_none());
    let mut keys: std::collections::HashSet<String> = std::collections::HashSet::new();
    for body in &bodies {
        if let Some(key) = body["primaryKey"]["enumId"].as_str() {
            keys.insert(key.to_string());
        }
        for key in body["primaryKeys"].as_array().into_iter().flatten() {
            keys.insert(key["enumId"].as_str().unwrap().to_string());
        }
    }
    assert!(
        whole || (0..200).all(|index| keys.contains(&format!("BATCH-{index}"))),
        "not every write is covered: {bodies:?}"
    );
    assert!(
        whole,
        "200 keys are past what one message carries, so the entity goes whole"
    );
}

/// Peers are posted to at once: one that is slow to answer does not hold up the others. Two peers that each take
/// 600 ms are both done in well under the 1.2 s one after the other would take, whichever is first.
#[test]
fn a_slow_peer_does_not_hold_up_the_others() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-parallel");
    let fingerprints = super::configured_fingerprints().unwrap();
    struct Timed {
        started: std::time::Instant,
        posts: Mutex<Vec<(String, Duration)>>,
    }
    impl Sender for Timed {
        fn post(&self, base_url: &str, _topic: &str, _payload: &[u8]) -> Result<super::PostOutcome, Error> {
            std::thread::sleep(Duration::from_millis(600));
            sync::lock(&self.posts).push((base_url.to_string(), self.started.elapsed()));
            Ok(super::PostOutcome::Accepted)
        }
    }
    let (ctx, facade) = open_facade();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    for (id, url) in [
        ("inst-par-one", "https://one.example:8444"),
        ("inst-par-two", "https://two.example:8444"),
    ] {
        put_member(&facade, id, url, now_value(), &fingerprints.session, &fingerprints.cert);
    }
    super::start(ctx.factory()).unwrap();
    let timed = Arc::new(Timed {
        started: std::time::Instant::now(),
        posts: Mutex::new(Vec::new()),
    });
    super::install_sender(ctx.factory(), Arc::clone(&timed) as Arc<dyn Sender>);
    write_row(&facade, "PAR-V1", true);
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    let posts = sync::lock(&timed.posts);
    let visitor: Vec<&(String, Duration)> = posts.iter().collect();
    assert_eq!(visitor.len(), 2, "{posts:?}");
    let last = visitor.iter().map(|(_, at)| *at).max().unwrap_or_default();
    assert!(
        last < Duration::from_millis(1_100),
        "the peers were posted to one after the other: {posts:?}"
    );
}

#[test]
fn cluster_tls_pair_is_stable_and_a_cert_without_a_key_fails() {
    let _lock = lock();
    let _clean = Clean;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let _env = ConfGuard::new(root.clone());
    let _example = crate::testing::ExampleOff::new();
    let before = repo_env_stamp(&root);
    let temp = TempRuntime::new("cluster-cert");
    crate::context::layout::link_base_set(&root.join("runtime/components/base"), &temp.path()).unwrap();
    let framework = root.join("runtime/components/base/framework");
    let factory = ExecutionContextFactory::boot_with_framework(temp.path(), &framework).unwrap();
    let cert = env::get_prop_or_env("cluster_tls_cert");
    let key = env::get_prop_or_env("cluster_tls_key");
    assert_one_line_der(&cert);
    assert_one_line_der(&key);
    assert_ne!(cert, key);
    let env_text = std::fs::read_to_string(temp.path().join(".env")).unwrap();
    assert_consecutive_pair(&env_text);
    factory.set_entity(crate::testing::open());
    super::start(&factory).unwrap();
    let instance = super::instance_id(&factory).unwrap();
    assert!(env::get_prop_or_env("instance_id").is_empty());
    assert_eq!(instance.len(), 36);
    assert!(instance.contains('-'));
    let names = factory.property_names().to_vec();
    drop(factory);
    for name in &names {
        env::clear_property(name);
    }
    env::clear_dotenv();
    let again = ExecutionContextFactory::boot_with_framework(temp.path(), &framework).unwrap();
    assert_eq!(env::get_prop_or_env("cluster_tls_cert"), cert);
    assert_eq!(env::get_prop_or_env("cluster_tls_key"), key);
    // The instance id is the instance's own, in runtime/db, not in the .env the members share.
    assert!(!std::fs::read_to_string(temp.path().join(".env"))
        .unwrap()
        .contains("instance_id"));
    assert_eq!(
        std::fs::read_to_string(temp.path().join("db/instance_id"))
            .unwrap()
            .trim(),
        instance
    );
    again.set_entity(crate::testing::open());
    super::start(&again).unwrap();
    assert_eq!(super::instance_id(&again).unwrap(), instance);
    let env_again = std::fs::read_to_string(temp.path().join(".env")).unwrap();
    assert_eq!(
        env_again
            .lines()
            .filter(|line| line.starts_with("cluster_tls_cert="))
            .count(),
        1
    );
    assert_eq!(
        env_again
            .lines()
            .filter(|line| line.starts_with("cluster_tls_key="))
            .count(),
        1
    );
    assert_eq!(
        env_again
            .lines()
            .filter(|line| line.starts_with("instance_id="))
            .count(),
        0
    );
    assert_consecutive_pair(&env_again);
    for name in again.property_names() {
        env::clear_property(name);
    }
    env::clear_dotenv();

    let blocked = TempRuntime::new("cluster-half");
    crate::context::layout::link_base_set(&root.join("runtime/components/base"), blocked.path()).unwrap();
    env::set_property("cluster_tls_cert", "configured-cert-value");
    let err = match ExecutionContextFactory::boot_with_framework(blocked.path(), &framework) {
        Ok(_) => panic!("boot succeeded with a certificate and no key"),
        Err(err) => err,
    };
    let shown = format!("{err} {err:?}");
    assert!(shown.contains("cluster_tls_cert"));
    assert!(shown.contains("cluster_tls_key"));
    assert!(!shown.contains("CHANGEME"));
    assert!(!shown.contains("configured-cert-value"));
    assert_eq!(repo_env_stamp(&root), before);
}

fn assert_one_line_der(text: &str) {
    assert!(!text.is_empty());
    assert!(!text.contains('\n'));
    assert!(!text.contains('\r'));
    assert!(text.chars().all(|ch| !ch.is_whitespace()));
    assert!(!text.contains("CHANGEME"));
    let der = STANDARD.decode(text).unwrap();
    assert!(!der.is_empty());
    assert_eq!(der[0], 0x30);
}

fn assert_consecutive_pair(text: &str) {
    let lines: Vec<&str> = text.lines().collect();
    let cert_at = lines
        .iter()
        .position(|line| line.starts_with("cluster_tls_cert="))
        .unwrap();
    assert!(lines[cert_at + 1].starts_with("cluster_tls_key="));
}

struct ConfGuard {
    root: PathBuf,
}

impl ConfGuard {
    fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl Drop for ConfGuard {
    fn drop(&mut self) {
        for relative in [
            "runtime/components/base/framework/conf/MoquiDefaultConf.xml",
            "runtime/conf/MoquiDevConf.xml",
        ] {
            let Ok(node) = MNode::parse_file(self.root.join(relative)) else {
                continue;
            };
            for property in node.named_iter("default-property") {
                if let Some(name) = property.attribute("name") {
                    env::clear_property(&name);
                }
            }
        }
        env::clear_dotenv();
    }
}

struct TempRuntime(PathBuf);

impl TempRuntime {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("moqui-cluster-{label}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(path.join("conf")).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRuntime {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn repo_env_stamp(root: &Path) -> Option<(u64, SystemTime)> {
    let meta = std::fs::metadata(root.join("runtime/.env")).ok()?;
    Some((meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
}

#[test]
fn a_direct_send_goes_to_its_member_only_and_to_this_member_locally() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-direct");
    let fingerprints = super::configured_fingerprints().unwrap();
    let (ctx, facade) = open_facade();
    let recording = install_recording(ctx.factory());
    let _entered = ctx.enter();
    for (id, url) in [
        ("inst-direct-media", "https://media.example:8444"),
        ("inst-direct-other", "https://other.example:8444"),
    ] {
        put_member(&facade, id, url, now_value(), &fingerprints.session, &fingerprints.cert);
    }
    put_member(
        &facade,
        "inst-direct-stale",
        "https://stale.example:8444",
        Value::I64(1),
        &fingerprints.session,
        &fingerprints.cert,
    );
    super::start(ctx.factory()).unwrap();
    let local = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&local);
    let _subscription = notify::subscribe(
        notify::RTC_SIGNAL,
        Arc::new(move |message: &TopicMessage| sync::lock(&seen).push(message.body.clone())),
    )
    .unwrap();
    let message = |target: &str| TopicMessage {
        origin_id: notify::origin_id().to_string(),
        message_id: crate::entity::uuid_v4().unwrap(),
        body: serde_json::json!({ "target": target, "kind": "frame" }),
    };
    super::send_to(
        &facade,
        "inst-direct-media",
        notify::RTC_SIGNAL,
        &message("inst-direct-media"),
    )
    .unwrap();
    super::send_to(&facade, "inst-direct", notify::RTC_SIGNAL, &message("inst-direct")).unwrap();
    assert!(super::send_to(
        &facade,
        "inst-direct-stale",
        notify::RTC_SIGNAL,
        &message("inst-direct-stale")
    )
    .is_err());
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while sync::lock(&recording.calls)
        .iter()
        .all(|c| c.topic != notify::RTC_SIGNAL)
        && std::time::Instant::now() < until
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    let posted: Vec<String> = sync::lock(&recording.calls)
        .iter()
        .filter(|c| c.topic == notify::RTC_SIGNAL)
        .map(|c| c.base.clone())
        .collect();
    assert_eq!(posted, vec!["https://media.example:8444".to_string()]);
    let local = sync::lock(&local).clone();
    assert_eq!(local.len(), 1, "{local:?}");
    assert_eq!(local[0]["target"], "inst-direct");
    assert!(super::for_this_member(ctx.factory(), &message("inst-direct")));
    assert!(!super::for_this_member(ctx.factory(), &message("inst-direct-media")));
}

fn invalidation(entity: &str, key: Option<&str>) -> TopicMessage {
    let primary_key = key.map_or_else(|| serde_json::json!({}), |id| serde_json::json!({ "enumId": id }));
    TopicMessage {
        origin_id: "peer".into(),
        message_id: format!("m-{entity}-{}", key.unwrap_or("")),
        body: serde_json::json!({ "entityName": entity, "primaryKey": primary_key }),
    }
}

/// HL-017: an invalidation joins the one waiting for its entity and carries every key, up to the cap; past it, or
/// with a write that had no usable key, the whole entity. Past the entities bound the invalidations become one clear of
/// every entity cache, which covers what comes after; other topics are bounded apart and do not push invalidations out.
#[test]
fn queued_invalidations_keep_every_key_and_widen_past_their_bounds() {
    let limits = super::Limits {
        others: 1,
        entities: 2,
        keys: 2,
    };
    let mut queue = super::Queue::default();
    let take =
        |queue: &mut super::Queue, topic, message| assert_eq!(queue.push(topic, message, limits), super::Queued::Taken);
    take(&mut queue, "collab-update", invalidation("doc", None));
    assert_eq!(
        queue.push("collab-update", invalidation("doc2", None), limits),
        super::Queued::Dropped,
        "other topics are bounded on their own"
    );
    take(&mut queue, "entity-cache-invalidate", invalidation("E1", Some("A")));
    take(&mut queue, "entity-cache-invalidate", invalidation("E1", Some("B")));
    take(&mut queue, "entity-cache-invalidate", invalidation("E1", Some("A")));
    take(&mut queue, "entity-cache-invalidate", invalidation("E2", None));
    take(&mut queue, "entity-cache-invalidate", invalidation("E2", Some("C")));
    let bodies: Vec<serde_json::Value> = queue.messages.iter().map(|(_, message)| message.body.clone()).collect();
    assert_eq!(bodies.len(), 3);
    assert_eq!(
        bodies[1]["primaryKeys"],
        serde_json::json!([{ "enumId": "A" }, { "enumId": "B" }])
    );
    assert!(bodies[1].get("primaryKey").is_none());
    assert!(
        bodies[2].get("primaryKey").is_none() && bodies[2].get("primaryKeys").is_none(),
        "a write with no key makes the entity whole, and a later key does not narrow it: {}",
        bodies[2]
    );
    take(&mut queue, "entity-cache-invalidate", invalidation("E1", Some("D")));
    let e1 = &queue.messages[1].1.body;
    assert!(
        e1.get("primaryKeys").is_none(),
        "past the cap the entity goes whole: {e1}"
    );
    // A third entity is past the entities bound.
    take(&mut queue, "entity-cache-invalidate", invalidation("E3", Some("X")));
    take(&mut queue, "entity-cache-invalidate", invalidation("E4", Some("Y")));
    let batch = queue.take();
    assert_eq!(batch.len(), 2, "the other topic and one clear of every entity cache");
    assert_eq!(batch[0].0, "collab-update");
    assert_eq!(batch[1].1.body, serde_json::json!({ "clearAll": true }));
    assert!(queue.messages.is_empty() && queue.invalidations.is_empty() && queue.clear_all.is_none());
}

/// HL-033: a bulk delete's invalidation carries its chunk's keys in `primaryKeys`; it joins a waiting one with each
/// of them, up to the cap, and one past the cap invalidates the entity whole.
#[test]
fn an_invalidation_of_several_keys_joins_with_each_key() {
    let limits = super::Limits {
        others: 1,
        entities: 2,
        keys: 4,
    };
    let several = |ids: &[&str]| TopicMessage {
        origin_id: "peer".into(),
        message_id: format!("m-{}", ids.join(",")),
        body: serde_json::json!({
            "entityName": "E1",
            "primaryKeys": ids.iter().map(|id| serde_json::json!({ "enumId": id })).collect::<Vec<_>>(),
        }),
    };
    let mut queue = super::Queue::default();
    assert_eq!(
        queue.push("entity-cache-invalidate", invalidation("E1", Some("A")), limits),
        super::Queued::Taken
    );
    queue.push("entity-cache-invalidate", several(&["B", "A", "C"]), limits);
    assert_eq!(
        queue.messages[0].1.body["primaryKeys"],
        serde_json::json!([{ "enumId": "A" }, { "enumId": "B" }, { "enumId": "C" }])
    );
    queue.push("entity-cache-invalidate", several(&["D", "E"]), limits);
    let body = &queue.messages[0].1.body;
    assert!(
        body.get("primaryKeys").is_none() && body.get("primaryKey").is_none(),
        "past the cap the entity goes whole: {body}"
    );
    // A first invalidation of several keys is queued as it is.
    let mut queue = super::Queue::default();
    queue.push("entity-cache-invalidate", several(&["A", "B"]), limits);
    assert_eq!(
        queue.messages[0].1.body["primaryKeys"],
        serde_json::json!([{ "enumId": "A" }, { "enumId": "B" }])
    );
    // The body a chunk's deletes make: every key, or past the cap none.
    let pk = ["enumId".to_string()];
    let keys: Vec<Arc<crate::entity::FieldMap>> = ["A", "B", "A"]
        .iter()
        .map(|id| Arc::new(std::iter::once((Arc::from("enumId"), Value::string(*id))).collect()))
        .collect();
    assert_eq!(
        super::keys_body("E1", &pk, &keys, 2),
        serde_json::json!({ "entityName": "E1", "primaryKeys": [{ "enumId": "A" }, { "enumId": "B" }] })
    );
    assert_eq!(
        super::keys_body("E1", &pk, &keys, 1),
        serde_json::json!({ "entityName": "E1" })
    );
}

/// The peer: a facade of its own, whose database the test changes directly, as a shared database is changed by the
/// sending member, so only the invalidations it is sent keep its cache fresh.
struct Remote {
    facade: EntityFacade,
}

impl Remote {
    fn new() -> Self {
        Self {
            facade: crate::testing::open(),
        }
    }

    /// The row's description as a cached find reads it.
    fn cached(&self, id: &str) -> Option<String> {
        self.facade
            .find(ROW)
            .unwrap()
            .eq("enumId", Value::string(id))
            .use_cache(true)
            .one()
            .unwrap()
            .map(|row| row.get("description").and_then(Value::as_str).unwrap_or("").to_string())
    }

    /// Write the row in the database without this facade's cache knowing.
    fn put(&self, id: &str, description: &str) {
        let condition = moqui_db::sql::Condition::equals("enumId", Value::string(id));
        let (insert, update) = self
            .facade
            .with_entity(ROW, |entity, dialect, _| {
                Ok((
                    moqui_db::sql::insert(entity, dialect, &["enumId", "description"])?,
                    moqui_db::sql::update(entity, dialect, &["description"], Some(&condition))?,
                ))
            })
            .unwrap();
        let tx = self.facade.transactions_for(ROW).unwrap();
        let updated =
            crate::entity::exec::execute_tx(&tx, &update.sql, &[Value::string(description), Value::string(id)])
                .unwrap();
        if updated == 0 {
            crate::entity::exec::execute_tx(&tx, &insert, &[Value::string(id), Value::string(description)]).unwrap();
        }
    }

    fn apply(&self, topic: &str, body: serde_json::Value) {
        let message = TopicMessage {
            origin_id: "peer".into(),
            message_id: crate::entity::uuid_v4().unwrap(),
            body,
        };
        super::apply_remote(&self.facade, topic, &message).unwrap();
    }
}

/// HL-017: a joined invalidation applied on a peer evicts each key's cached find, a cached miss of a row then
/// inserted among them, and leaves another entity's alone.
#[test]
fn a_joined_invalidation_keeps_every_key_fresh_on_the_peer() {
    let remote = Remote::new();
    remote.put("J2", "old");
    assert_eq!(remote.cached("J1"), None);
    assert_eq!(remote.cached("J2").as_deref(), Some("old"));
    let mut queue = super::Queue::default();
    for id in ["J1", "J2"] {
        queue.push("entity-cache-invalidate", invalidation(ROW, Some(id)), super::LIMITS);
    }
    let batch = queue.take();
    assert_eq!(batch.len(), 1);
    remote.put("J1", "new");
    remote.put("J2", "new");
    assert_eq!(remote.cached("J1"), None, "stale until the invalidation");
    remote.apply("entity-cache-invalidate", batch[0].1.body.clone());
    assert_eq!(remote.cached("J1").as_deref(), Some("new"));
    assert_eq!(remote.cached("J2").as_deref(), Some("new"));
    // A clear of every entity cache, as a peer that missed invalidations is sent.
    remote.put("J2", "newer");
    assert_eq!(remote.cached("J2").as_deref(), Some("new"));
    remote.apply("entity-cache-invalidate", serde_json::json!({ "clearAll": true }));
    assert_eq!(remote.cached("J2").as_deref(), Some("newer"));
}

/// Applies what it is posted to the remote facade, unless failing or held.
struct Deliver {
    remote: EntityFacade,
    fail: AtomicBool,
    hold: AtomicBool,
    bodies: Mutex<Vec<serde_json::Value>>,
}

impl Sender for Deliver {
    fn post(&self, _base_url: &str, topic: &str, payload: &[u8]) -> Result<super::PostOutcome, Error> {
        while self.hold.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(Error::new("peer unreachable"));
        }
        let json: serde_json::Value = serde_json::from_slice(payload).unwrap();
        sync::lock(&self.bodies).push(json["body"].clone());
        let message = TopicMessage {
            origin_id: json["origin_id"].as_str().unwrap_or("").to_string(),
            message_id: json["message_id"].as_str().unwrap_or("").to_string(),
            body: json["body"].clone(),
        };
        super::apply_remote(&self.remote, topic, &message)?;
        Ok(super::PostOutcome::Accepted)
    }
}

/// A started member over `facade` with one live peer, `remote`, posted to through a [`Deliver`].
fn deliver_to(
    ctx: &crate::context::ExecutionContext,
    facade: &EntityFacade,
    remote: &Remote,
    peer_live: bool,
) -> Arc<Deliver> {
    let fingerprints = super::configured_fingerprints().unwrap();
    let deliver = Arc::new(Deliver {
        remote: remote.facade.clone(),
        fail: AtomicBool::new(false),
        hold: AtomicBool::new(false),
        bodies: Mutex::new(Vec::new()),
    });
    super::install_sender(ctx.factory(), Arc::clone(&deliver) as Arc<dyn Sender>);
    super::start(ctx.factory()).unwrap();
    let heartbeat = if peer_live {
        now_value()
    } else {
        stamp_value(now_millis() - 4 * 60 * 1000)
    };
    put_member(
        facade,
        "inst-remote",
        PEER,
        heartbeat,
        &fingerprints.session,
        &fingerprints.cert,
    );
    deliver
}

const PEER: &str = "https://remote.example:8443";

/// A write here, with the same change made to the peer's database.
fn write_both(facade: &EntityFacade, remote: &Remote, id: &str, description: &str) {
    remote.put(id, description);
    let mut value = facade.make_value(ROW).unwrap();
    value.set("enumId", Value::string(id));
    value.set("description", Value::string(description));
    value.create_or_update(facade).unwrap();
}

/// The backoff of a peer whose posts failed is over, as it is after `PEER_BACKOFF`.
fn end_backoff(ctx: &crate::context::ExecutionContext) {
    super::lock(&super::ClusterState::of(ctx.factory()).peers)
        .down_until
        .clear();
}

/// HL-017: two rows written in one transaction while a batch is on its way are both fresh on the peer, a cached miss
/// among them; a write of an entity no member caches is not posted.
#[test]
fn two_rows_changed_in_one_batch_are_both_fresh_on_the_peer() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-cover");
    let (ctx, facade) = open_facade();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let remote = Remote::new();
    let deliver = deliver_to(&ctx, &facade, &remote, true);
    write_both(&facade, &remote, "C2", "old");
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert_eq!(remote.cached("C1"), None);
    assert_eq!(remote.cached("C2").as_deref(), Some("old"));
    // The sender is held on another write, so the two below wait together.
    deliver.hold.store(true, Ordering::SeqCst);
    write_both(&facade, &remote, "C0", "first");
    facade.begin(ROW, None).unwrap();
    write_both(&facade, &remote, "C1", "new");
    write_both(&facade, &remote, "C2", "new");
    facade.commit(ROW).unwrap();
    deliver.hold.store(false, Ordering::SeqCst);
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert_eq!(remote.cached("C1").as_deref(), Some("new"));
    assert_eq!(remote.cached("C2").as_deref(), Some("new"));
    let bodies = sync::lock(&deliver.bodies).clone();
    assert!(
        bodies
            .iter()
            .any(|body| body["primaryKeys"] == serde_json::json!([{ "enumId": "C1" }, { "enumId": "C2" }])),
        "{bodies:?}"
    );
    // `Visitor` is `cache="never"` and in no cacheable view: its writes are not posted.
    let before = bodies.len();
    let mut visitor = facade.make_value("moqui.server.Visitor").unwrap();
    visitor.set("visitorId", Value::string("NEVER-V1"));
    visitor.create(&facade).unwrap();
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert_eq!(sync::lock(&deliver.bodies).len(), before);
}

/// HL-018: a peer whose posts failed every try is sent a clear of every entity cache once it answers again, by the
/// heartbeat, with no other write.
#[test]
fn a_peer_that_missed_invalidations_on_failed_posts_is_cleared_without_another_write() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-gap-retry");
    let (ctx, facade) = open_facade();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let remote = Remote::new();
    let deliver = deliver_to(&ctx, &facade, &remote, true);
    assert_eq!(remote.cached("R1"), None);
    deliver.fail.store(true, Ordering::SeqCst);
    write_both(&facade, &remote, "R1", "lost");
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert_eq!(remote.cached("R1"), None, "the invalidation was lost");
    deliver.fail.store(false, Ordering::SeqCst);
    end_backoff(&ctx);
    super::heartbeat(ctx.factory());
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert_eq!(remote.cached("R1").as_deref(), Some("lost"));
    assert!(super::lock(&super::ClusterState::of(ctx.factory()).peers)
        .dirty
        .is_empty());
}

/// A message of `topic` with `body`, from this member.
fn topic_message(body: serde_json::Value) -> TopicMessage {
    TopicMessage {
        origin_id: notify::origin_id().to_string(),
        message_id: crate::entity::uuid_v4().unwrap(),
        body,
    }
}

/// Whether the peer was sent a clear of every entity cache.
fn sent_clear_all(deliver: &Deliver) -> bool {
    sync::lock(&deliver.bodies)
        .iter()
        .any(|body| body["clearAll"] == serde_json::json!(true))
}

/// HL-018: a failed post of another topic (a session end) marks the peer as well as one of an invalidation: once it
/// answers it gets a clear of every entity cache (which retires the sessions whose end it missed), with no write.
#[test]
fn a_peer_that_missed_another_topic_on_failed_posts_is_sent_its_clear() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-gap-topic");
    let (ctx, facade) = open_facade();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let remote = Remote::new();
    let deliver = deliver_to(&ctx, &facade, &remote, true);
    deliver.fail.store(true, Ordering::SeqCst);
    let ended = topic_message(serde_json::json!({ "sessionIds": ["S-LOST"] }));
    super::publish(&facade, notify::SESSION_END, &ended).unwrap();
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert!(
        super::lock(&super::ClusterState::of(ctx.factory()).peers)
            .dirty
            .contains_key(PEER),
        "the lost session end marks the peer"
    );
    deliver.fail.store(false, Ordering::SeqCst);
    end_backoff(&ctx);
    super::heartbeat(ctx.factory());
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert!(sent_clear_all(&deliver));
    assert!(super::lock(&super::ClusterState::of(ctx.factory()).peers)
        .dirty
        .is_empty());
}

/// HL-018: a message of another topic dropped from a full queue reaches no peer, so every peer is marked and gets a
/// clear of every entity cache ahead of its next batch.
#[test]
fn a_message_dropped_from_a_full_queue_marks_every_peer() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-gap-drop");
    let (ctx, facade) = open_facade();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let remote = Remote::new();
    let deliver = deliver_to(&ctx, &facade, &remote, true);
    // the sender is held on the first, having read its peers, while the queue fills
    deliver.hold.store(true, Ordering::SeqCst);
    let state = super::ClusterState::of(ctx.factory());
    super::publish(
        &facade,
        notify::NOTIFICATION_MESSAGE,
        &topic_message(serde_json::json!({})),
    )
    .unwrap();
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while super::lock(&state.peers).urls.is_empty() && std::time::Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    for n in 0..=super::OUTBOX_LIMIT {
        super::publish(
            &facade,
            notify::NOTIFICATION_MESSAGE,
            &topic_message(serde_json::json!({ "n": n })),
        )
        .unwrap();
    }
    assert!(state.dropped.load(Ordering::Relaxed) >= 1);
    assert!(
        super::lock(&state.peers).dirty.contains_key(PEER),
        "a dropped message marks the peer"
    );
    deliver.hold.store(false, Ordering::SeqCst);
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(30)));
    assert!(sent_clear_all(&deliver));
    assert!(super::lock(&state.peers).dirty.is_empty());
}

/// HL-018: a write while the peer is in its backoff is not posted to it; the peer gets its clear when the backoff ends.
#[test]
fn a_write_during_a_peers_backoff_is_recovered_without_another_write() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-gap-backoff");
    let (ctx, facade) = open_facade();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let remote = Remote::new();
    let deliver = deliver_to(&ctx, &facade, &remote, true);
    write_both(&facade, &remote, "B0", "seen");
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert_eq!(remote.cached("B1"), None);
    super::mark_peer_down(&super::ClusterState::of(ctx.factory()), PEER);
    let posted = sync::lock(&deliver.bodies).len();
    write_both(&facade, &remote, "B1", "skipped");
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert_eq!(
        sync::lock(&deliver.bodies).len(),
        posted,
        "nothing is posted to a peer in backoff"
    );
    super::heartbeat(ctx.factory());
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert_eq!(remote.cached("B1"), None, "still in backoff");
    end_backoff(&ctx);
    super::heartbeat(ctx.factory());
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert_eq!(remote.cached("B1").as_deref(), Some("skipped"));
}

/// HL-018: a peer out of the live set misses what is written meanwhile; when it is back, the heartbeat that finds it
/// sends it a clear of every entity cache.
#[test]
fn a_peer_back_in_the_live_set_is_cleared_without_another_write() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-gap-member");
    let fingerprints = super::configured_fingerprints().unwrap();
    let (ctx, facade) = open_facade();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let remote = Remote::new();
    let deliver = deliver_to(&ctx, &facade, &remote, false);
    assert_eq!(remote.cached("M1"), None);
    write_both(&facade, &remote, "M1", "while out");
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert!(sync::lock(&deliver.bodies).is_empty(), "a stale peer is not posted to");
    put_member(
        &facade,
        "inst-remote",
        PEER,
        now_value(),
        &fingerprints.session,
        &fingerprints.cert,
    );
    super::heartbeat(ctx.factory());
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert_eq!(remote.cached("M1").as_deref(), Some("while out"));
}

/// A read that found no peer is trusted only briefly: a member that joins after it gets the next write here, with a
/// clear of every entity cache first as a newly seen peer, without waiting for the heartbeat.
#[test]
fn a_peer_that_joins_after_an_empty_read_gets_the_next_write() {
    let _lock = lock();
    let _clean = Clean;
    preset("inst-alone");
    let fingerprints = super::configured_fingerprints().unwrap();
    let (ctx, facade) = open_facade();
    let recording = install_recording(ctx.factory());
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    super::start(ctx.factory()).unwrap();
    write_row(&facade, "ALONE-1", true);
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert!(sync::lock(&recording.calls).is_empty(), "no peer yet");
    put_member(
        &facade,
        "inst-joined",
        "https://joined.example:8443",
        now_value(),
        &fingerprints.session,
        &fingerprints.cert,
    );
    std::thread::sleep(Duration::from_millis(super::EMPTY_PEERS_TRUSTED_MS as u64 + 50));
    write_row(&facade, "ALONE-2", true);
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    let bodies: Vec<serde_json::Value> = sync::lock(&recording.calls)
        .iter()
        .map(|call| serde_json::from_slice::<serde_json::Value>(&call.payload).unwrap()["body"].clone())
        .collect();
    assert_eq!(
        bodies.first(),
        Some(&serde_json::json!({ "clearAll": true })),
        "{bodies:?}"
    );
    assert!(
        bodies.iter().any(|body| body["primaryKey"]["enumId"] == "ALONE-2"),
        "{bodies:?}"
    );
}
