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

//! The transaction cache candidates of plans/archive/TX-Cache.md, with the cache off and on: the five services (S1 to S5)
//! called as a user, and the pieces of framework logic outside services (F1, F3, F4, F5) as their callers run them.
//! Each row is one operation per call, in its own transaction. "on" calls a service with `use_transaction_cache`, or
//! runs the logic in a `cache` frame. F1 and F3 hold their writes themselves (their own `force-cache` frames), so they
//! have an "on" row only, to compare with the baseline in plans/archive/TX-Cache.md. F2 (a `ServiceJob` run) runs on the job workers and is not timed here.
//! The factory runs async work inline: what a row's calls queue (the data feed's receive, a chat message's indexing)
//! runs after the row, outside its time and its counts.
//!
//! Out of the default `cargo test` link, and ignored so naming the binary does not start a postmaster.
//!
//! ```text
//! CARGO_PROFILE_RELEASE_DEBUG=1 cargo test --release -p moqui-host --test tx_cache_speed -- --ignored --nocapture
//! ```
//!
//! `TX_CACHE_SPEED_CALLS` (default 200) and `TX_CACHE_SPEED_BATCHES` (default 5) size it; `TX_CACHE_SPEED_ROW` runs
//! only the rows whose name contains it (for a profile). See `speed/mod.rs` for the columns: `rt` is the waited round
//! trips per call. After the table, the transaction cache's own counts per call ([`tx_cache_stats`]) for each "on"
//! row. `SPEED_PG_HOST` runs it on an external Postgres (`speed::SpeedDb`).

#![cfg(all(unix, feature = "embedded-postgres"))]

mod speed;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::{BootOptions, ExecutionContext, ExecutionContextFactory, Executor};
use moqui_core::driver::DbDriver;
use moqui_core::entity::cache::{tx_cache_stats, TxCacheStats};
use moqui_core::entity::load_feed::{conf_data_locations, load_entity_data, LoadOptions};
use moqui_core::entity::EntityFacade;
use moqui_core::service::call::{sync_with, CallOptions};
use moqui_core::transaction::{PoolConfig, TransactionAttribute};
use moqui_host::db::entity_facade::{attach, open_entity_facade};
use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use speed::{env_usize, DirGuard, Rows};

const WARMUP: usize = 8;
const USER_A: &str = "speed.a";
const USER_B: &str = "speed.b";
const PASSWORD: &str = "Speed-Secret1!";
const CHAT: &str = "org.moqui.impl.ChatServices";

#[test]
#[ignore = "transaction cache timings: cargo test --release -p moqui-host --test tx_cache_speed -- --ignored --nocapture"]
fn tx_cache_speed() {
    speed::warn_if_debug("tx_cache_speed");
    moqui_host::logging::install_with_console(Some(tracing::Level::WARN));
    let calls = env_usize("TX_CACHE_SPEED_CALLS", 200);
    let batches = env_usize("TX_CACHE_SPEED_BATCHES", 5);
    let only = std::env::var("TX_CACHE_SPEED_ROW").unwrap_or_default();
    // No scheduler passes: the seed's cron jobs would run between the rows.
    moqui_lib::env::set_property("scheduled_job_check_time", "0");
    moqui_lib::env::set_property("session_cookie_key", "CQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQk=");

    let runtime = speed::temp_dir("txc-speed-rt");
    let cluster_dir = speed::temp_dir("txc-speed-pg");
    let _guards = (DirGuard(runtime.clone()), DirGuard(cluster_dir.clone()));
    std::fs::create_dir_all(&runtime).expect("runtime dir");
    let (cluster, driver) = speed::SpeedDb::start(&cluster_dir, 20);
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    // Async work (the data feed's receive of each pending row, a chat message's indexing) runs on this thread after
    // each row, outside its time, so a row times only its own calls and its statements are only its own.
    let options = BootOptions {
        executor: Executor::Inline,
        ..BootOptions::default()
    };
    let factory = ExecutionContextFactory::boot_with_options(&runtime, &framework, options).expect("boot");
    let facade = open_entity_facade(&framework, Arc::clone(&driver), pool()).expect("open entity facade");
    attach(&factory, facade.clone());
    facade.check_all_tables().expect("check tables");

    {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        for location in framework_locations(&facade, &framework) {
            if let Err(err) = load_entity_data(&facade, &location, &LoadOptions::default()) {
                panic!("load {location}: {err}");
            }
        }
        clear_errors();
        let world = seed(&facade);
        println!();
        println!(
            "transaction cache candidates, {calls} calls x {batches} batches, warmup {WARMUP}. One transaction a \
             call. ExecutionContext entered, authorization disabled, logged in as {USER_A}."
        );
        let mut rows = Rows::default();
        let mut stats: BTreeMap<String, (u64, TxCacheStats)> = BTreeMap::new();
        one_batch(&factory, &facade, &world, WARMUP, &only, &mut rows, &mut stats);
        stats.clear();
        rows.record = true;
        for batch in 1..=batches {
            eprintln!("  batch {batch}/{batches}");
            one_batch(&factory, &facade, &world, calls, &only, &mut rows, &mut stats);
        }
        rows.print();
        print_stats(&stats);
    }

    drop(factory);
    drop(facade);
    drop(driver);
    drop(cluster);
}

struct World {
    room: Value,
    /// The room's first message, which N2 reacts to.
    first_seq: Value,
    /// The mail account's Inbox, which N1 imports into.
    inbox: String,
    /// The mail account, whose draft N4 saves.
    mail_account: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Cache {
    Off,
    On,
}

impl Cache {
    fn label(self) -> &'static str {
        match self {
            Cache::Off => "off",
            Cache::On => "on",
        }
    }
}

fn one_batch(
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    world: &World,
    calls: usize,
    only: &str,
    rows: &mut Rows,
    stats: &mut BTreeMap<String, (u64, TxCacheStats)>,
) {
    let mut row = |name: &str, cache: Cache, body: &mut dyn FnMut(usize)| {
        let name = format!("{name}, cache {}", cache.label());
        if !only.is_empty() && !name.contains(only) {
            return;
        }
        let before = tx_cache_stats();
        rows.run(&name, calls, || {
            for index in 0..calls {
                body(index);
            }
        });
        if rows.record && cache == Cache::On {
            let entry = stats.entry(name).or_default();
            entry.0 += u64::try_from(calls).unwrap_or(u64::MAX);
            entry.1.add(&tx_cache_stats().since(&before));
        }
        factory.run_pending();
        clear_errors();
    };
    for cache in [Cache::Off, Cache::On] {
        row("S1 heartbeat#UserStatus", cache, &mut |_| {
            service(
                &format!("{CHAT}.heartbeat#UserStatus"),
                cache,
                [("active", Value::Bool(true))],
            );
        });
        row("S3 send#Message", cache, &mut |index| {
            service(
                &format!("{CHAT}.send#Message"),
                cache,
                [
                    ("chatRoomId", world.room.clone()),
                    ("body", Value::string(format!("<p>message {index}</p>"))),
                ],
            );
        });
        row("S2 mark#Read", cache, &mut |_| {
            service(
                &format!("{CHAT}.mark#Read"),
                cache,
                [("chatRoomId", world.room.clone())],
            );
        });
        row("S4 update#WikiPage", cache, &mut |index| {
            service(
                "org.moqui.impl.WikiServices.update#WikiPage",
                cache,
                [
                    ("wikiSpaceId", Value::string("SPEED")),
                    ("pageName", Value::string("Page1")),
                    ("pageText", Value::string(format!("text {index}"))),
                ],
            );
        });
        row("S5 set#MessageFlags", cache, &mut |index| {
            service(
                "org.moqui.impl.EmailClientServices.set#MessageFlags",
                cache,
                [
                    ("emailMessageIdList", Value::List(vec![Value::string("SPEEDM1")])),
                    ("read", Value::Bool(index % 2 == 0)),
                ],
            );
        });
        // F1 and F3 hold their writes themselves (force-cache) since Phase 5: one row, compared with the baseline.
        if cache == Cache::On {
            row("F1 notification persist, 3 users", cache, &mut |index| {
                let mut note = moqui_core::notify::NotificationMessage::new();
                note.topic("SpeedTopic")
                    .user_id(USER_A)
                    .user_id(USER_B)
                    .user_id("speed.c")
                    .persist(true)
                    .title(format!("note {index}"));
                note.send().expect("notification");
            });
        }
        row("F4 audited update UserAccount.username", cache, &mut |index| {
            in_frame(facade, "moqui.security.UserAccount", cache, || {
                let mut value = facade.make_value("moqui.security.UserAccount")?;
                value.set("userId", Value::string("SPEED_AUDIT"));
                value.set("username", Value::string(format!("speed.audit.{}", index % 2)));
                value.update(facade)
            });
        });
        row("F5 fed update EmailMessage.isFlagged", cache, &mut |index| {
            in_frame(facade, "moqui.basic.email.EmailMessage", cache, || {
                let mut value = facade.make_value("moqui.basic.email.EmailMessage")?;
                value.set("emailMessageId", Value::string("SPEEDM2"));
                value.set("isFlagged", Value::string(if index % 2 == 0 { "Y" } else { "N" }));
                value.update(facade)
            });
        });
        // N1 to N3 (plans/archive/Profiling.md, Round 19): framework paths the cache was applied to beyond the first set. N1's
        // frame is the code's own (force-cache), so its "off" row turns the cache off for the whole manager.
        let tx = facade
            .transactions_for("moqui.basic.email.EmailMessage")
            .expect("transactions");
        tx.set_use_transaction_cache(cache == Cache::On);
        row("N1 mail store, 2 attachments, a cursor", cache, &mut |index| {
            service(
                "org.moqui.impl.EmailClientServices.import#EmailMessages",
                cache,
                [
                    ("emailFolderId", Value::string(&world.inbox)),
                    ("messageText", Value::string(mail_text(index))),
                ],
            );
        });
        tx.set_use_transaction_cache(true);
        row("N2 react#Message", cache, &mut |_| {
            service(
                &format!("{CHAT}.react#Message"),
                cache,
                [
                    ("chatRoomId", world.room.clone()),
                    ("eventSeq", world.first_seq.clone()),
                    ("key", Value::string("+1")),
                ],
            );
        });
        let ids = unviewed_notifications(facade, calls * 5);
        let mut chunks = ids.chunks(5);
        row("N3 mark#NotificationsViewed of 5", cache, &mut |_| {
            let chunk = chunks.next().unwrap_or_default();
            service(
                "org.moqui.impl.NotificationServices.mark#NotificationsViewed",
                cache,
                [("notificationMessageIdList", Value::List(chunk.to_vec()))],
            );
        });
        // N4: an autosave of a draft, the draft made first.
        let draft = service_out(
            "org.moqui.impl.EmailClientServices.save#Draft",
            [
                ("emailAccountId", Value::string(&world.mail_account)),
                ("toAddresses", Value::string("someone@speed.test")),
                ("subject", Value::string("Speed draft")),
                ("body", Value::string("<p>draft</p>")),
            ],
        )
        .get("emailMessageId")
        .cloned()
        .expect("draft emailMessageId");
        row("N4 save#Draft, an autosave", cache, &mut |index| {
            service(
                "org.moqui.impl.EmailClientServices.save#Draft",
                cache,
                [
                    ("emailMessageId", draft.clone()),
                    ("emailAccountId", Value::string(&world.mail_account)),
                    ("toAddresses", Value::string("someone@speed.test")),
                    ("subject", Value::string("Speed draft")),
                    ("body", Value::string(format!("<p>draft {index}</p>"))),
                ],
            );
        });
        if cache == Cache::On {
            row("F3 session start and password login", cache, &mut |_| {
                let guard = moqui_core::user::bind(factory, None, None, false).expect("bind");
                moqui_core::user::login_user(USER_A, PASSWORD).expect("login");
                drop(guard);
            });
            moqui_core::user::internal_login_user(USER_A).expect("login again");
        }
    }
    let errors = take_errors();
    assert!(errors.is_empty(), "errors: {errors:?}");
}

/// One service call in its own transaction, with the cache as asked; its errors fail the test.
fn service<const N: usize>(name: &str, cache: Cache, parameters: [(&str, Value); N]) {
    let parameters: BTreeMap<String, Value> = parameters
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect();
    let options = CallOptions {
        use_transaction_cache: Some(cache == Cache::On),
        ..CallOptions::default()
    };
    sync_with(name, &parameters, &options).unwrap_or_else(|err| panic!("{name}: {err}"));
    let errors = take_errors();
    assert!(errors.is_empty(), "{name}: {errors:?}");
}

fn in_frame(
    facade: &EntityFacade,
    entity: &str,
    cache: Cache,
    body: impl FnOnce() -> Result<(), moqui_lib::error::Error>,
) {
    let attribute = match cache {
        Cache::Off => TransactionAttribute::UseOrBegin,
        Cache::On => TransactionAttribute::Cache,
    };
    let tx = facade.transactions_for(entity).expect("transactions");
    tx.run(attribute, None, body)
        .unwrap_or_else(|err| panic!("{entity}: {err}"));
}

/// The users, a chat room of the two with a message, a wiki space, an email account with two messages, and the audited
/// account; then logged in as user A.
fn seed(facade: &EntityFacade) -> World {
    let password = moqui_core::user::hash_password(PASSWORD).expect("hash");
    for (user_id, username) in [
        ("SPEED_A", USER_A),
        ("SPEED_B", USER_B),
        ("SPEED_C", "speed.c"),
        ("SPEED_AUDIT", "speed.audit.0"),
    ] {
        let mut row = facade.make_value("moqui.security.UserAccount").expect("user");
        row.set("userId", Value::string(user_id));
        row.set("username", Value::string(username));
        row.set("userFullName", Value::string(format!("Speed {username}")));
        row.set("emailAddress", Value::string(format!("{username}@speed.test")));
        row.set("currentPassword", Value::string(&password));
        row.set("passwordHashType", Value::string("argon2"));
        row.create(facade).expect("create user");
    }
    moqui_core::user::internal_login_user(USER_A).expect("login");
    let room = service_out(
        &format!("{CHAT}.create#Room"),
        [
            ("roomTypeEnumId", Value::string("private")),
            ("roomName", Value::string("Speed")),
            ("memberUserIdList", Value::List(vec![Value::string("SPEED_B")])),
        ],
    )
    .get("chatRoomId")
    .cloned()
    .expect("chatRoomId");
    let first_seq = service_out(
        &format!("{CHAT}.send#Message"),
        [("chatRoomId", room.clone()), ("body", Value::string("<p>first</p>"))],
    )
    .get("eventSeq")
    .cloned()
    .expect("eventSeq");
    service_out(
        "org.moqui.impl.WikiServices.create#WikiSpace",
        [
            ("wikiSpaceId", Value::string("SPEED")),
            ("description", Value::string("Speed space")),
        ],
    );
    service_out(
        "org.moqui.impl.WikiServices.update#WikiPage",
        [
            ("wikiSpaceId", Value::string("SPEED")),
            ("pageName", Value::string("Page1")),
            ("pageText", Value::string("first")),
        ],
    );
    let mut account = facade.make_value("moqui.basic.email.EmailAccount").expect("account");
    account.set("emailAccountId", Value::string("SPEEDACCT"));
    account.set("ownerUserId", Value::string("SPEED_A"));
    account.set("emailAddress", Value::string("speed.a@speed.test"));
    account.create(facade).expect("create account");
    for id in ["SPEEDM1", "SPEEDM2"] {
        let mut message = facade.make_value("moqui.basic.email.EmailMessage").expect("message");
        message.set("emailMessageId", Value::string(id));
        message.set("emailAccountId", Value::string("SPEEDACCT"));
        message.set("statusId", Value::string("ES_RECEIVED"));
        message.set("subject", Value::string(format!("Speed {id}")));
        message.set("isRead", Value::string("N"));
        message.set("isFlagged", Value::string("N"));
        message.create(facade).expect("create message");
    }
    // N1: a mail account of user A's, and a mail change cursor of A's, so each write of a stored message makes a change
    // row (note#MessageChange reads the account's users and their states for each).
    let mail = service_out(
        "org.moqui.impl.EmailClientServices.create#EmailAccount",
        [
            ("emailAddress", Value::string("speed.mail@speed.test")),
            ("accountName", Value::string("Speed mail")),
        ],
    );
    let mail_account = mail
        .get("emailAccountId")
        .map(moqui_lib::object::to_plain_string)
        .expect("emailAccountId");
    let inbox = facade
        .find("moqui.basic.email.EmailFolder")
        .expect("folder")
        .eq("emailAccountId", Value::string(&mail_account))
        .eq("folderKindEnumId", Value::string("EfkInbox"))
        .one()
        .expect("inbox")
        .and_then(|row| row.get("emailFolderId").map(moqui_lib::object::to_plain_string))
        .expect("an Inbox");
    let mut state = facade.make_value("moqui.basic.email.EmailUserState").expect("state");
    state.set("userId", Value::string("SPEED_A"));
    state.set("lastChangeSeq", Value::I64(0));
    state.set("purgedThruSeq", Value::I64(0));
    state.create(facade).expect("create state");
    World {
        room,
        first_seq,
        inbox,
        mail_account,
    }
}

/// A received message with two attachments, of a Message-ID of its own.
fn mail_text(index: usize) -> String {
    format!(
        "From: sender@speed.test\r\nTo: speed.mail@speed.test\r\nSubject: Speed {index}\r\n\
         Message-ID: <speed-{index}-{nanos}@speed.test>\r\nDate: Fri, 6 Nov 2026 10:00:00 +0000\r\nMIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nBody {index}\r\n\
         --b\r\nContent-Type: text/plain; name=\"one.txt\"\r\nContent-Disposition: attachment; filename=\"one.txt\"\r\n\r\n\
         one\r\n--b\r\nContent-Type: text/plain; name=\"two.txt\"\r\nContent-Disposition: attachment; filename=\"two.txt\"\r\n\
         \r\ntwo\r\n--b--\r\n",
        nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos())
    )
}

/// `count` notifications of user A, unviewed, for N3 to mark viewed five a call.
fn unviewed_notifications(facade: &EntityFacade, count: usize) -> Vec<Value> {
    let tx = facade
        .transactions_for("moqui.security.user.NotificationMessage")
        .expect("transactions");
    tx.run(TransactionAttribute::UseOrBegin, None, || {
        let mut ids = Vec::with_capacity(count);
        for _ in 0..count {
            let id = moqui_core::entity::uuid_v4()?;
            let mut message = facade.make_value("moqui.security.user.NotificationMessage")?;
            message.set("notificationMessageId", Value::string(&id));
            message.set("topic", Value::string("SpeedTopic"));
            message.create(facade)?;
            let mut user = facade.make_value("moqui.security.user.NotificationMessageUser")?;
            user.set("notificationMessageId", Value::string(&id));
            user.set("userId", Value::string("SPEED_A"));
            user.create(facade)?;
            ids.push(Value::string(id));
        }
        Ok(ids)
    })
    .expect("notifications")
}

fn service_out<const N: usize>(name: &str, parameters: [(&str, Value); N]) -> BTreeMap<String, Value> {
    let parameters: BTreeMap<String, Value> = parameters
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect();
    let out = moqui_core::service::sync(name, &parameters).unwrap_or_else(|err| panic!("{name}: {err}"));
    let errors = take_errors();
    assert!(errors.is_empty(), "{name}: {errors:?}");
    out
}

fn take_errors() -> Vec<String> {
    ExecutionContext::with_current(|ctx| {
        let mut message = ctx.message();
        let errors = message.errors().to_vec();
        message.clear_errors();
        errors
    })
    .unwrap_or_default()
}

fn clear_errors() {
    let errors = take_errors();
    if !errors.is_empty() {
        eprintln!("errors from the data load (ignored): {errors:?}");
    }
}

/// The transaction cache's own counts per call of each "on" row, summed over the batches.
fn print_stats(stats: &BTreeMap<String, (u64, TxCacheStats)>) {
    if stats.is_empty() {
        return;
    }
    println!();
    println!(
        "  {:<46} {:>7} {:>7} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "transaction cache per call", "frames", "held", "mem rd", "db rd", "early", "batch st", "batch rt"
    );
    for (name, (calls, stats)) in stats {
        let calls = (*calls).max(1) as f64;
        let per = |count: u64| count as f64 / calls;
        println!(
            "  {:<46} {:>7.2} {:>7.2} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>8.2}",
            name,
            per(stats.frames),
            per(stats.held()),
            per(stats.memory_reads),
            per(stats.db_reads),
            per(stats.early_flush_count()),
            per(stats.batch_statements),
            per(stats.batch_round_trips),
        );
    }
}

/// What `moqui load` reads for the framework, as `data_load_speed` reads it.
fn framework_locations(facade: &EntityFacade, framework: &Path) -> Vec<String> {
    let conf = MNode::parse_file(framework.join("conf/MoquiDefaultConf.xml")).expect("conf");
    let entity_facade = conf.first_named("entity-facade").expect("entity-facade");
    let mut locations: Vec<String> = entity_facade
        .children_named("load-entity")
        .into_iter()
        .filter_map(|node| node.attribute("location"))
        .filter(|location| !location.ends_with(".eecas.xml"))
        .collect();
    let mut rest: Vec<String> = std::fs::read_dir(framework.join("entity"))
        .expect("entity dir")
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.ends_with(".xml") && !name.ends_with(".eecas.xml"))
        .map(|name| format!("component://framework/entity/{name}"))
        .filter(|location| !locations.contains(location))
        .collect();
    rest.sort();
    locations.extend(rest);
    locations.extend(conf_data_locations(facade).expect("conf data locations"));
    locations
}

fn pool() -> PoolConfig {
    PoolConfig {
        min: 1,
        max: 4,
        reserve: 1,
        acquire_timeout: Duration::from_secs(30),
        ..Default::default()
    }
}
