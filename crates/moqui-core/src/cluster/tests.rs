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
    assert!(super::post_with_retry(
        &sender,
        "https://peer",
        "collab-update",
        b"body"
    ));
    assert!(super::post_with_retry(
        &sender,
        "https://peer",
        "entity-cache-invalidate",
        b"next"
    ));
    assert_eq!(refuse.0.load(Ordering::SeqCst), 2);
    assert!(super::PostOutcome::from_status(503).is_err());
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

fn write_visitor(facade: &EntityFacade, id: &str, commit: bool) {
    facade.begin("moqui.server.Visitor", None).unwrap();
    let mut value = facade.make_value("moqui.server.Visitor").unwrap();
    value.set("visitorId", Value::string(id));
    value.create(facade).unwrap();
    if commit {
        facade.commit("moqui.server.Visitor").unwrap();
    } else {
        facade.rollback("moqui.server.Visitor").unwrap();
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
    write_visitor(&facade, "V1", false);
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert!(sync::lock(&recording.calls).is_empty());
    write_visitor(&facade, "V2", true);
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
    assert_eq!(json["body"]["entityName"], "moqui.server.Visitor");
    assert_eq!(json["body"]["primaryKey"]["visitorId"], "V2");
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
        write_visitor(&second_facade, "TWO-V1", true);
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
    write_visitor(&facade, "V3", true);
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert!(sync::lock(&recording.calls).is_empty());
}

#[test]
fn tick_runs_the_stand_in_and_moves_the_heartbeat() {
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
    let before = super::load_members(&facade)
        .unwrap()
        .into_iter()
        .find(|member| member.instance_id == "inst-tick")
        .and_then(|member| member.heartbeat)
        .unwrap();
    std::thread::sleep(Duration::from_millis(50));
    ctx.factory().service_pools().tick();
    assert!(hit.load(Ordering::SeqCst));
    let after = super::load_members(&facade)
        .unwrap()
        .into_iter()
        .find(|member| member.instance_id == "inst-tick")
        .and_then(|member| member.heartbeat)
        .unwrap();
    assert!(after > before, "heartbeat {before} did not move to {after}");
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
    write_visitor(&facade, "RETRY-V4", true);
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert_eq!(scripted.calls.load(Ordering::SeqCst), 3);

    let always = Arc::new(Scripted {
        only: "RETRY-V5",
        fails_left: AtomicUsize::new(9),
        calls: AtomicUsize::new(0),
    });
    super::install_sender(ctx.factory(), Arc::clone(&always) as Arc<dyn Sender>);
    write_visitor(&facade, "RETRY-V5", true);
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
    assert_eq!(always.calls.load(Ordering::SeqCst), 3);

    *super::lock(&super::ClusterState::of(ctx.factory()).sender) = None;
    write_visitor(&facade, "RETRY-V6", true);
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(5)));
}

#[test]
fn many_writes_in_one_wait_reach_a_peer_as_one_invalidation_and_the_commit_does_not_wait() {
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
    facade.begin("moqui.server.Visitor", None).unwrap();
    for index in 0..200 {
        let mut value = facade.make_value("moqui.server.Visitor").unwrap();
        value.set("visitorId", Value::string(format!("BATCH-{index}")));
        value.create(&facade).unwrap();
    }
    facade.commit("moqui.server.Visitor").unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the commit waited on the peer: {:?}",
        started.elapsed()
    );
    assert!(super::wait_for_sends(ctx.factory(), Duration::from_secs(10)));
    let posted = sync::lock(&slow.0);
    let visitor_posts = posted
        .iter()
        .filter(|text| text.contains("moqui.server.Visitor"))
        .count();
    // The first commit callback may be taken alone before the rest are queued; the rest arrive together.
    assert!((1..=2).contains(&visitor_posts), "{visitor_posts} posts for 200 writes");
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
    std::os::unix::fs::symlink(root.join("runtime/base-component"), temp.path().join("base-component")).unwrap();
    let framework = root.join("runtime/base-component/framework");
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
    std::os::unix::fs::symlink(
        root.join("runtime/base-component"),
        blocked.path().join("base-component"),
    )
    .unwrap();
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
            "runtime/base-component/framework/conf/MoquiDefaultConf.xml",
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
