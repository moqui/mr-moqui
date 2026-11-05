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

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::resource::MemoryResource;

/// Records each loaded entry. `fail_type` makes the load of that type fail.
struct ListLoader {
    loaded: Rc<RefCell<Vec<SimpleEntry>>>,
    events: Rc<RefCell<Vec<String>>>,
    fail_type: Option<String>,
}

impl Loader for ListLoader {
    fn init(&mut self, timeout: Option<u32>) {
        self.events.borrow_mut().push(format!("init {timeout:?}"));
    }

    fn load(&mut self, entry: &mut SimpleEntry) -> Result<(), Error> {
        if self.fail_type.as_deref() == Some(entry.etl_type.as_str()) {
            return Err(Error::new(format!("cannot load {}", entry.etl_type)));
        }
        self.loaded.borrow_mut().push(entry.clone());
        Ok(())
    }

    fn complete(&mut self, etl: &SimpleEtl) {
        self.events
            .borrow_mut()
            .push(format!("complete error={}", etl.has_error()));
    }
}

struct VecExtractor(Vec<SimpleEntry>);

impl Extractor for VecExtractor {
    fn extract(&mut self, etl: &mut SimpleEtl) -> Result<(), Error> {
        for entry in self.0.drain(..) {
            if etl.process_entry(entry).is_err() {
                break;
            }
        }
        Ok(())
    }
}

type Shared<T> = Rc<RefCell<Vec<T>>>;

fn loader(fail_type: Option<&str>) -> (ListLoader, Shared<SimpleEntry>, Shared<String>) {
    let loaded = Rc::new(RefCell::new(Vec::new()));
    let events = Rc::new(RefCell::new(Vec::new()));
    (
        ListLoader {
            loaded: Rc::clone(&loaded),
            events: Rc::clone(&events),
            fail_type: fail_type.map(str::to_string),
        },
        loaded,
        events,
    )
}

fn entry(etl_type: &str, pairs: &[(&str, &str)]) -> SimpleEntry {
    SimpleEntry::new(
        etl_type,
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), Value::string(*value)))
            .collect(),
    )
}

fn text_of(entry: &SimpleEntry, key: &str) -> Option<String> {
    entry.values.get(key).and_then(Value::as_str).map(str::to_string)
}

#[test]
fn flat_xml_reads_attributes_and_child_text() {
    let root = MemoryResource::new_root("mem://etl");
    let file = root.make_file("rows.xml").unwrap();
    file.write_text(
        r#"<?xml version="1.0"?>
            <rows ignored="yes">
                <!-- comment -->
                <Item itemId="A" name="first"><description>  one &amp; two  </description><note/></Item>
                <Item itemId="B"><description><![CDATA[<b>]]></description></Item>
                <Other otherId="X"/>
            </rows>"#,
    )
    .unwrap();
    let (load, loaded, events) = loader(None);
    let mut etl = SimpleEtl::new(FlatXmlExtractor::new(file), load);
    etl.process();
    assert!(
        !etl.has_error(),
        "{:?}",
        etl.single_error_cause().map(ToString::to_string)
    );
    assert_eq!(etl.extract_count(), 3);
    assert_eq!(etl.load_count(), 3);
    let loaded = loaded.borrow();
    assert_eq!(loaded[0].etl_type, "Item");
    assert_eq!(text_of(&loaded[0], "itemId").as_deref(), Some("A"));
    assert_eq!(text_of(&loaded[0], "name").as_deref(), Some("first"));
    // Child text is raw, as SAX characters give it.
    assert_eq!(text_of(&loaded[0], "description").as_deref(), Some("  one & two  "));
    assert_eq!(text_of(&loaded[0], "note").as_deref(), Some(""));
    assert_eq!(text_of(&loaded[1], "description").as_deref(), Some("<b>"));
    assert_eq!(loaded[2].etl_type, "Other");
    assert_eq!(events.borrow().as_slice(), ["init Some(3600)", "complete error=false"]);
}

#[test]
fn flat_xml_errors_name_the_location_and_missing_file_is_empty() {
    let root = MemoryResource::new_root("mem://etl");
    let file = root.make_file("bad.xml").unwrap();
    file.write_text("<rows><Item a=\"1\"><x><y>deep</y></x></Item></rows>")
        .unwrap();
    let (load, _, _) = loader(None);
    let mut etl = SimpleEtl::new(FlatXmlExtractor::new(file), load);
    etl.process();
    let message = etl.extract_error().unwrap().to_string();
    assert!(
        message.contains("Error parsing XML from mem://etl/bad.xml"),
        "{message}"
    );
    assert!(
        message.contains("Invalid close element x, was expecting Item"),
        "{message}"
    );

    let missing = root.with_location("mem://etl/none.xml");
    let (load, loaded, _) = loader(None);
    let mut etl = SimpleEtl::new(FlatXmlExtractor::new(missing), load);
    etl.process();
    assert!(!etl.has_error());
    assert!(loaded.borrow().is_empty());
}

#[test]
fn transformers_add_filter_and_run_by_type() {
    let source = vec![
        entry("Order", &[("orderId", "O1")]),
        entry("Order", &[("orderId", "SKIP")]),
        entry("Note", &[("text", "hi")]),
    ];
    let (load, loaded, _) = loader(None);
    let mut etl = SimpleEtl::new(VecExtractor(source), load);
    etl.add_transformer(|transform: &mut EntryTransform| {
        transform
            .entry_mut()
            .values
            .insert("seen".to_string(), Value::Bool(true));
        Ok(())
    });
    etl.add_type_transformer("Order", |transform: &mut EntryTransform| {
        let id = text_of(transform.entry(), "orderId").unwrap_or_default();
        if id == "SKIP" {
            transform.load_current(false);
            return Ok(());
        }
        // A new entry replaces the current one unless the current one is kept.
        transform.add_entry(entry("OrderItem", &[("orderId", &id)]));
        transform.load_current(true);
        Ok(())
    });
    etl.add_type_transformer("OrderItem", |transform: &mut EntryTransform| {
        transform
            .entry_mut()
            .values
            .insert("itemSeqId".to_string(), Value::string("01"));
        Ok(())
    });
    etl.process();
    assert!(!etl.has_error());
    assert_eq!(etl.extract_count(), 3);
    assert_eq!(etl.skip_count(), 1);
    let loaded = loaded.borrow();
    let types: Vec<&str> = loaded.iter().map(|entry| entry.etl_type.as_str()).collect();
    // The current entry goes first, then the entries its transform added.
    assert_eq!(types, ["Order", "OrderItem", "Note"]);
    assert_eq!(text_of(&loaded[1], "itemSeqId").as_deref(), Some("01"));
    assert_eq!(loaded[1].values.get("seen"), Some(&Value::Bool(true)));
    assert_eq!(loaded[2].values.get("seen"), Some(&Value::Bool(true)));
}

#[test]
fn added_entry_without_keep_replaces_current() {
    let (load, loaded, _) = loader(None);
    let mut etl = SimpleEtl::new(VecExtractor(vec![entry("Raw", &[("a", "1")])]), load);
    etl.add_transformer(|transform: &mut EntryTransform| {
        if transform.entry().etl_type == "Raw" {
            transform.add_entry(entry("Clean", &[("a", "1")]));
        }
        Ok(())
    });
    etl.process();
    let types: Vec<String> = loaded.borrow().iter().map(|entry| entry.etl_type.clone()).collect();
    assert_eq!(types, ["Clean"]);
    assert_eq!(etl.skip_count(), 0);
}

#[test]
fn transform_and_load_errors_are_kept_or_stop() {
    let source = || {
        vec![
            entry("Good", &[]),
            entry("Bad", &[]),
            entry("Broken", &[]),
            entry("Good", &[]),
        ]
    };
    let failing = |transform: &mut EntryTransform| {
        if transform.entry().etl_type == "Bad" {
            return Err(Error::new("bad entry"));
        }
        Ok(())
    };

    let (load, loaded, events) = loader(Some("Broken"));
    let mut etl = SimpleEtl::new(VecExtractor(source()), load);
    etl.add_transformer(failing);
    etl.process();
    assert_eq!(etl.extract_count(), 4);
    assert_eq!(etl.transform_errors().len(), 1);
    assert_eq!(etl.transform_errors()[0].entry.etl_type, "Bad");
    assert_eq!(etl.load_errors().len(), 1);
    assert_eq!(etl.single_error_cause().unwrap().message(), "bad entry");
    assert_eq!(loaded.borrow().len(), 2);
    assert_eq!(events.borrow().last().map(String::as_str), Some("complete error=true"));

    let (load, loaded, _) = loader(Some("Broken"));
    let mut config = TransformConfiguration::new();
    config.add_transformer(failing);
    let mut etl = SimpleEtl::new(VecExtractor(source()), load);
    etl.set_configuration(config).stop_on_error();
    etl.process();
    // The stop comes on the second entry; the extractor ends there.
    assert_eq!(etl.extract_count(), 2);
    assert_eq!(loaded.borrow().len(), 1);
    assert!(etl.load_errors().is_empty());
}

#[test]
fn copied_configuration_runs_and_is_not_changed() {
    let mut config = TransformConfiguration::new();
    config.add_transformer(|transform: &mut EntryTransform| {
        transform.load_current(false);
        Ok(())
    });
    let (load, loaded, _) = loader(None);
    let mut etl = SimpleEtl::new(VecExtractor(vec![entry("A", &[])]), load);
    etl.add_configuration(&config);
    etl.add_type_transformer("A", |_: &mut EntryTransform| Ok(()));
    etl.process();
    assert!(loaded.borrow().is_empty());
    assert_eq!(etl.skip_count(), 1);
    assert!(config.type_transformers.is_empty());
}

/// A CSV with a title line above its header: rows by header with their row numbers, quoted commas and line breaks
/// kept, empty rows and cells left out, and a rerun from a row.
#[test]
fn csv_rows_are_entries_by_header_with_their_row_numbers() {
    let text = "Exported 2026\nid,name,note\n1,Ann,\"a, b\"\n\n2,Bo,\"two\nlines\"\n3,,\n";
    let options = CsvOptions {
        header_row: 2,
        etl_type: "Row".to_string(),
        ..CsvOptions::default()
    };
    let (load, loaded, _) = loader(None);
    let extractor = CsvExtractor::new(text.as_bytes(), options.clone());
    let headers = extractor.headers();
    let mut etl = SimpleEtl::new(extractor, load);
    etl.process();
    assert_eq!(*headers.lock().unwrap(), ["id", "name", "note"]);
    let rows: Vec<(String, String, Option<String>, Value)> = loaded
        .borrow()
        .iter()
        .map(|entry| {
            (
                entry.etl_type.clone(),
                text_of(entry, "id").unwrap_or_default(),
                text_of(entry, "note"),
                entry.values[ROW_NUMBER_KEY].clone(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (
                "Row".to_string(),
                "1".to_string(),
                Some("a, b".to_string()),
                Value::I64(3)
            ),
            (
                "Row".to_string(),
                "2".to_string(),
                Some("two\nlines".to_string()),
                Value::I64(4)
            ),
            ("Row".to_string(), "3".to_string(), None, Value::I64(7)),
        ]
    );
    // a rerun from row 5, with no header: columns by number
    let (load, loaded, _) = loader(None);
    let rerun = CsvOptions {
        header_row: 0,
        start_row: 5,
        delimiter: b';',
        ..CsvOptions::default()
    };
    let mut etl = SimpleEtl::new(CsvExtractor::new("a;b\nc;d\ne;f\ng;h\ni;j\n".as_bytes(), rerun), load);
    etl.process();
    let firsts: Vec<String> = loaded.borrow().iter().filter_map(|entry| text_of(entry, "1")).collect();
    assert_eq!(firsts, ["i"]);
}
