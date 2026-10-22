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

//! `MNode` parse and read timings and allocations over the XML files of `runtime/` (entity, service, screen, data
//! and conf files), with no database. The columns are per file, per node, or per element; see `speed/mod.rs`.
//!
//! Out of the default `cargo test` link, and ignored.
//!
//! ```text
//! CARGO_PROFILE_RELEASE_DEBUG=1 cargo test --release -p moqui-host --test xml_speed -- --ignored --nocapture
//! ```
//!
//! `XML_SPEED_BATCHES` (default 5) sizes it. "kept" on the parse-and-keep row is the memory the trees hold.

mod speed;

use std::hint::black_box;
use std::path::{Path, PathBuf};

use moqui_lib::node::MNode;

use speed::{env_usize, Rows};

#[test]
#[ignore = "xml timings: cargo test --release -p moqui-host --test xml_speed -- --ignored --nocapture"]
fn xml_speed() {
    speed::warn_if_debug("xml_speed");
    let batches = env_usize("XML_SPEED_BATCHES", 5);
    let runtime = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime");
    let mut paths = Vec::new();
    collect_xml(&runtime, &mut paths);
    paths.sort();
    let files: Vec<(String, String)> = paths
        .iter()
        .filter(|path| !path.ends_with("framework-catalog.xml"))
        .map(|path| {
            (
                path.display().to_string(),
                std::fs::read_to_string(path).expect("read xml"),
            )
        })
        .collect();
    let kilobytes = files.iter().map(|(_, text)| text.len()).sum::<usize>() / 1024;
    let trees = parse_all(&files);
    let nodes = trees.iter().map(count_nodes).sum::<usize>();
    drop(trees);
    let elements = record_elements(2000);

    println!();
    println!(
        "MNode, {} files ({kilobytes} KB, {nodes} nodes) x {batches} batches. Per file unless named.",
        files.len()
    );
    let mut rows = Rows::default();
    for batch in 0..=batches {
        rows.record = batch > 0;
        rows.run("parse every file, dropped", files.len(), || {
            for (location, text) in &files {
                black_box(MNode::parse_text(location, text).expect("parse"));
            }
        });
        let trees = rows.run("parse every file, kept", files.len(), || parse_all(&files));
        rows.run("read every node (name, attrs, children), per node", nodes, || {
            for tree in &trees {
                black_box(read_tree(tree));
            }
        });
        rows.run("first_named + attribute lookups, per node", nodes, || {
            for tree in &trees {
                black_box(lookup_tree(tree));
            }
        });
        rows.run("drop the kept trees", files.len(), || drop(trees));
        rows.run("parse a data record element (raw), per element", elements.len(), || {
            for element in &elements {
                black_box(MNode::parse_text_raw("data.xml", element).expect("parse"));
            }
        });
    }
    rows.print();
}

fn parse_all(files: &[(String, String)]) -> Vec<MNode> {
    files
        .iter()
        .map(|(location, text)| MNode::parse_text(location, text).expect("parse"))
        .collect()
}

fn count_nodes(node: &MNode) -> usize {
    1 + node.children().iter().map(count_nodes).sum::<usize>()
}

/// What a definition builder reads of each node: its name, every attribute, its text and its children.
fn read_tree(node: &MNode) -> usize {
    let mut total = node.name().len() + node.text().map_or(0, |text| text.len());
    for (name, value) in node.attributes() {
        total += name.len() + value.len();
    }
    for child in node.children() {
        total += read_tree(&child);
    }
    total
}

/// Lookups by name, as the screen, form and service builders make them: a few attributes and a child by name.
fn lookup_tree(node: &MNode) -> usize {
    let mut total = 0;
    for name in ["name", "type", "location", "entity-name"] {
        total += node.attribute(name).map_or(0, |value| value.len());
    }
    if node.first_named("description").is_some() {
        total += 1;
    }
    total += node.children_named("field").len();
    for child in node.children() {
        total += lookup_tree(&child);
    }
    total
}

/// Entity data elements as a data file holds them, one record each.
fn record_elements(count: usize) -> Vec<String> {
    (0..count)
        .map(|i| {
            format!(
                "<moqui.test.TestEntity testId=\"X{i}\" testMedium=\"Medium &amp; text {i}\" \
                 testLong=\"Long text for record {i}\" testIndicator=\"Y\" testDate=\"2026-10-20\" \
                 testDateTime=\"2026-10-20 12:34:56.789\" testNumberInteger=\"{i}\" testNumberDecimal=\"{i}.25\"/>"
            )
        })
        .collect()
}

fn collect_xml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_xml(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "xml") {
            out.push(path);
        }
    }
}
