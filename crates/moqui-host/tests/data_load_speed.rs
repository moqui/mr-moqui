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

//! Entity data loader timings and allocations against embedded Postgres: generated XML, CSV and JSON files of
//! `moqui.test.TestEntity` rows, nested XML records, and the framework's own data files. Each file loads in one
//! transaction, as `moqui load` does; the columns are per record.
//!
//! Out of the default `cargo test` link, and ignored so naming the binary does not start a postmaster.
//!
//! ```text
//! CARGO_PROFILE_RELEASE_DEBUG=1 cargo test --release -p moqui-host --test data_load_speed -- --ignored --nocapture
//! ```
//!
//! `DATA_LOAD_SPEED_RECORDS` (default 1000) is the records of each generated file and `DATA_LOAD_SPEED_BATCHES`
//! (default 5) the batches; see `speed/mod.rs` for the columns. Every batch writes new files with new keys, so a
//! "create" row inserts and a "reload" row updates what the batch's create wrote.
//! `SPEED_PG_HOST` runs it on a new database of an external Postgres instead of the embedded one (`speed::SpeedDb`).

#![cfg(all(unix, feature = "embedded-postgres"))]

mod speed;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::driver::DbDriver;
use moqui_core::entity::load_feed::{conf_data_locations, load_entity_data, LoadOptions};
use moqui_core::entity::EntityFacade;
use moqui_core::transaction::PoolConfig;
use moqui_host::db::entity_facade::{attach, open_entity_facade};
use moqui_lib::node::MNode;

use speed::{env_usize, DirGuard, Rows};

const TEST: &str = "moqui.test.TestEntity";
const WARMUP: usize = 50;
/// Enumerations under each nested `EnumerationType`.
const CHILDREN: usize = 9;

#[test]
#[ignore = "data load timings: cargo test --release -p moqui-host --test data_load_speed -- --ignored --nocapture"]
fn data_load_speed() {
    speed::warn_if_debug("data_load_speed");
    moqui_host::logging::install_with_console(Some(tracing::Level::WARN));
    let records = env_usize("DATA_LOAD_SPEED_RECORDS", 1000);
    let batches = env_usize("DATA_LOAD_SPEED_BATCHES", 5);

    let runtime = speed::temp_dir("load-speed-rt");
    let cluster_dir = speed::temp_dir("load-speed-pg");
    let files = speed::temp_dir("load-speed-files");
    let _guards = (
        DirGuard(runtime.clone()),
        DirGuard(cluster_dir.clone()),
        DirGuard(files.clone()),
    );
    std::fs::create_dir_all(&runtime).expect("runtime dir");
    std::fs::create_dir_all(&files).expect("files dir");
    let (cluster, driver) = speed::SpeedDb::start(&cluster_dir, 20);
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let factory = ExecutionContextFactory::boot_with_framework(&runtime, &framework).expect("boot");
    let facade = open_entity_facade(&framework, Arc::clone(&driver), pool()).expect("open entity facade");
    attach(&factory, facade.clone());
    facade.check_all_tables().expect("check tables");

    {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        println!();
        println!(
            "data load, {records} records a file x {batches} batches, warmup {WARMUP}. One transaction a file. \
             ExecutionContext entered, authorization disabled."
        );
        let mut rows = Rows::default();
        let data = framework_locations(&facade, &framework);
        // The framework data, the first time: inserts, through the ECA rules and services its files name.
        rows.record = true;
        let (measured, seed) = speed::measure(|| load_all(&facade, &data));
        let first = format!("framework data, first load ({seed} rec)");
        let reload = format!("framework data, reload ({seed} rec)");
        rows.note(&first, seed, measured);
        rows.record = false;
        one_batch(&facade, &files, "w", WARMUP, &mut rows);
        load_all(&facade, &data);
        rows.record = true;
        for batch in 1..=batches {
            eprintln!("  batch {batch}/{batches}");
            one_batch(&facade, &files, &format!("b{batch}"), records, &mut rows);
            rows.run(&reload, seed, || {
                check(load_all(&facade, &data), seed, "framework reload")
            });
        }
        rows.print();
    }

    drop(factory);
    drop(facade);
    drop(driver);
    drop(cluster);
}

fn one_batch(facade: &EntityFacade, dir: &Path, batch: &str, records: usize, rows: &mut Rows) {
    // A record's store is one upsert where that fits, else the entity-auto store.
    let store = LoadOptions::default();
    let xml = write_file(
        dir,
        &format!("{batch}-x.xml"),
        &xml_text(&format!("{batch}x"), records, "xml"),
    );
    let changed = write_file(
        dir,
        &format!("{batch}-x2.xml"),
        &xml_text(&format!("{batch}x"), records, "changed"),
    );
    let csv = write_file(dir, &format!("{batch}-c.csv"), &csv_text(&format!("{batch}c"), records));
    let json = write_file(
        dir,
        &format!("{batch}-j.json"),
        &json_text(&format!("{batch}j"), records),
    );
    let nested_count = records.div_ceil(CHILDREN + 1);
    let nested = write_file(dir, &format!("{batch}-n.xml"), &nested_text(batch, nested_count));

    rows.run("xml create (store), new rows", records, || {
        check(load(facade, &xml, &store), records, "xml")
    });
    rows.run("xml reload, same values", records, || {
        check(load(facade, &xml, &store), records, "xml")
    });
    rows.run("xml reload, changed values", records, || {
        check(load(facade, &changed, &store), records, "changed")
    });
    rows.run("csv create, new rows", records, || {
        check(load(facade, &csv, &store), records, "csv")
    });
    rows.run("json create, new rows", records, || {
        check(load(facade, &json, &store), records, "json")
    });
    let nested_records = nested_count * (CHILDREN + 1);
    rows.run(
        &format!("xml nested, type + {CHILDREN} enums, new"),
        nested_records,
        || check(load(facade, &nested, &store), nested_records, "nested"),
    );
}

fn load(facade: &EntityFacade, location: &Path, options: &LoadOptions) -> usize {
    load_location(facade, location.to_str().expect("utf-8 path"), options)
}

fn load_location(facade: &EntityFacade, location: &str, options: &LoadOptions) -> usize {
    let loaded = load_entity_data(facade, location, options).unwrap_or_else(|err| panic!("load {location}: {err}"));
    assert_eq!(loaded.failed_files, 0, "failed files in {location}");
    let errors = errors();
    assert!(errors.is_empty(), "load {location}: {errors:?}");
    usize::try_from(loaded.records).unwrap_or(usize::MAX)
}

fn load_all(facade: &EntityFacade, locations: &[String]) -> usize {
    locations
        .iter()
        .map(|location| load_location(facade, location, &LoadOptions::default()))
        .sum()
}

/// What `moqui load` reads for the framework, in its order: the `seed-data` of the entity files (the conf's
/// `load-entity` files first, then the rest by name), then the conf's `load-data` files.
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

fn check(loaded: usize, expected: usize, what: &str) {
    assert_eq!(loaded, expected, "records loaded from {what}");
}

/// The message facade's errors, cleared.
fn errors() -> Vec<String> {
    ExecutionContext::with_current(|ctx| {
        let errors = ctx.message().errors().to_vec();
        ctx.message().clear_errors();
        errors
    })
    .unwrap_or_default()
}

fn write_file(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("write data file");
    path
}

fn xml_text(prefix: &str, records: usize, medium: &str) -> String {
    let mut text = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<entity-facade-xml type=\"seed\">\n");
    for i in 0..records {
        let _ = writeln!(
            text,
            "    <{TEST} testId=\"{prefix}{i}\" testMedium=\"{medium} {i}\" testLong=\"Long text for record {i}\" \
             testIndicator=\"Y\" testDate=\"2026-10-20\" testDateTime=\"2026-10-20 12:34:56.789\" \
             testNumberInteger=\"{i}\" testNumberDecimal=\"{i}.25\" testCurrencyAmount=\"{i}.50\"/>"
        );
    }
    text.push_str("</entity-facade-xml>\n");
    text
}

fn csv_text(prefix: &str, records: usize) -> String {
    let mut text = format!(
        "{TEST},seed\ntestId,testMedium,testLong,testIndicator,testDate,testDateTime,testNumberInteger,\
         testNumberDecimal,testCurrencyAmount\n"
    );
    for i in 0..records {
        let _ = writeln!(
            text,
            "{prefix}{i},csv {i},Long text for record {i},Y,2026-10-20,2026-10-20 12:34:56.789,{i},{i}.25,{i}.50"
        );
    }
    text
}

fn json_text(prefix: &str, records: usize) -> String {
    // A leading object with only `_dataType` is the file's type, not a record.
    let mut text = String::from("[\n{\"_dataType\":\"seed\"}");
    for i in 0..records {
        text.push_str(",\n");
        let _ = write!(
            text,
            "{{\"_entity\":\"{TEST}\",\"testId\":\"{prefix}{i}\",\"testMedium\":\"json {i}\",\
             \"testLong\":\"Long text for record {i}\",\"testIndicator\":\"Y\",\"testDate\":\"2026-10-20\",\
             \"testDateTime\":\"2026-10-20 12:34:56.789\",\"testNumberInteger\":{i},\"testNumberDecimal\":\"{i}.25\",\
             \"testCurrencyAmount\":\"{i}.50\"}}"
        );
    }
    text.push_str("\n]\n");
    text
}

/// `EnumerationType` records, each with [`CHILDREN`] nested `Enumeration` records that take its key.
fn nested_text(batch: &str, types: usize) -> String {
    let mut text = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<entity-facade-xml type=\"seed\">\n");
    for i in 0..types {
        let type_id = format!("DLS_{batch}_{i}");
        let _ = writeln!(
            text,
            "    <moqui.basic.EnumerationType enumTypeId=\"{type_id}\" description=\"Load speed type {i}\">"
        );
        for child in 0..CHILDREN {
            let _ = writeln!(
                text,
                "        <moqui.basic.Enumeration enumId=\"{type_id}_{child}\" enumCode=\"C{child}\" \
                 sequenceNum=\"{child}\" description=\"Load speed enum {i}.{child}\"/>"
            );
        }
        text.push_str("    </moqui.basic.EnumerationType>\n");
    }
    text.push_str("</entity-facade-xml>\n");
    text
}

fn pool() -> PoolConfig {
    PoolConfig {
        min: 1,
        max: 2,
        reserve: 1,
        acquire_timeout: Duration::from_secs(30),
        ..Default::default()
    }
}
