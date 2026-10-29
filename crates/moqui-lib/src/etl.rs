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

//! Extract, transform, load: `SimpleEtl` and `FlatXmlExtractor`.
//!
//! The extractor calls [`SimpleEtl::process_entry`] for each record. Transformers run in the order
//! they were added, the ones for any type first. The loader sees each entry that is not filtered.
//! An entry is a [`SimpleEntry`]: a type and a value map. An entity find in core extracts each
//! value as an entry whose type is the entity name.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use indexmap::IndexMap;

use crate::error::Error;
use crate::node::{XmlEvent, XmlEvents};
use crate::object::Value;
use crate::resource::Resource;

/// Default transaction timeout in seconds passed to [`Loader::init`].
const DEFAULT_TIMEOUT: u32 = 3600;

/// One record: its type (an entity name for the entity loader) and its values.
#[derive(Clone, Debug, PartialEq)]
pub struct SimpleEntry {
    pub etl_type: String,
    pub values: BTreeMap<String, Value>,
}

impl SimpleEntry {
    pub fn new(etl_type: impl Into<String>, values: BTreeMap<String, Value>) -> Self {
        Self {
            etl_type: etl_type.into(),
            values,
        }
    }
}

/// Thrown out of [`SimpleEtl::process_entry`] when stop-on-error is set. The extractor stops.
#[derive(Clone, Debug)]
pub struct StopError {
    pub cause: Arc<Error>,
}

impl std::fmt::Display for StopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ETL stopped: {}", self.cause)
    }
}

impl std::error::Error for StopError {}

/// A transform or load failure and the entry it happened on.
#[derive(Clone, Debug)]
pub struct EtlError {
    pub entry: SimpleEntry,
    pub error: Arc<Error>,
}

/// The entry a transformer works on, and the entries it adds.
pub struct EntryTransform {
    entry: SimpleEntry,
    new_entries: Vec<SimpleEntry>,
    load_current: Option<bool>,
}

impl EntryTransform {
    fn new(entry: SimpleEntry) -> Self {
        Self {
            entry,
            new_entries: Vec::new(),
            load_current: None,
        }
    }

    /// The current entry, to read the type and values.
    pub fn entry(&self) -> &SimpleEntry {
        &self.entry
    }

    /// The current entry, to change its values.
    pub fn entry_mut(&mut self) -> &mut SimpleEntry {
        &mut self.entry
    }

    /// By default the current entry is loaded only if no new entries are added. False filters it
    /// even if none are added. True loads it even if some are added.
    pub fn load_current(&mut self, load: bool) -> &mut Self {
        self.load_current = Some(load);
        self
    }

    /// Add an entry to be transformed and, if not filtered, loaded.
    pub fn add_entry(&mut self, entry: SimpleEntry) -> &mut Self {
        self.new_entries.push(entry);
        self
    }

    fn loads_current(&self) -> bool {
        self.load_current.unwrap_or(self.new_entries.is_empty())
    }
}

/// Called once to start processing. Calls `etl.process_entry` for each record and closes itself.
pub trait Extractor {
    fn extract(&mut self, etl: &mut SimpleEtl) -> Result<(), Error>;
}

/// Stateless transformer and filter. Calls on [`EntryTransform`] add entries (generally of other
/// types), change the current entry's values, or filter it.
pub trait Transformer: Send + Sync {
    fn transform(&self, transform: &mut EntryTransform) -> Result<(), Error>;
}

impl<F> Transformer for F
where
    F: Fn(&mut EntryTransform) -> Result<(), Error> + Send + Sync,
{
    fn transform(&self, transform: &mut EntryTransform) -> Result<(), Error> {
        self(transform)
    }
}

pub trait Loader {
    /// Called before processing begins. The timeout is in seconds, for a transaction.
    fn init(&mut self, timeout: Option<u32>);
    /// Load one entry, transformed if there are transformers. The loader may add values to it.
    fn load(&mut self, entry: &mut SimpleEntry) -> Result<(), Error>;
    /// Called after all entries are processed: close files, commit or roll back.
    fn complete(&mut self, etl: &SimpleEtl);
}

/// Transformers for any type and by type. Clones share the transformers.
#[derive(Clone, Default)]
pub struct TransformConfiguration {
    any_transformers: Vec<Arc<dyn Transformer>>,
    type_transformers: IndexMap<String, Vec<Arc<dyn Transformer>>>,
    has_transformers: bool,
}

impl TransformConfiguration {
    pub fn new() -> Self {
        Self::default()
    }

    /// A transformer for any type. They run in the order added.
    pub fn add_transformer(&mut self, transformer: impl Transformer + 'static) -> &mut Self {
        self.any_transformers.push(Arc::new(transformer));
        self.has_transformers = true;
        self
    }

    /// A transformer for one type. They run in order for entries of the type, after the any-type ones.
    pub fn add_type_transformer(&mut self, etl_type: &str, transformer: impl Transformer + 'static) -> &mut Self {
        self.type_transformers
            .entry(etl_type.to_string())
            .or_default()
            .push(Arc::new(transformer));
        self.has_transformers = true;
        self
    }

    fn copy_from(&mut self, other: &TransformConfiguration) {
        self.any_transformers.extend(other.any_transformers.iter().cloned());
        for (etl_type, transformers) in &other.type_transformers {
            self.type_transformers
                .entry(etl_type.clone())
                .or_default()
                .extend(transformers.iter().cloned());
        }
        // Java leaves the flag as it was, so a configuration that only copied another never ran.
        self.has_transformers |= other.has_transformers;
    }

    fn run_transformers(
        &self,
        errors: &mut Vec<EtlError>,
        stop_on_error: bool,
        transform: &mut EntryTransform,
        load_entries: &mut Vec<SimpleEntry>,
    ) -> Result<(), StopError> {
        for transformer in &self.any_transformers {
            transform_entry(errors, stop_on_error, transformer.as_ref(), transform)?;
        }
        if !transform.entry.etl_type.is_empty() {
            if let Some(transformers) = self.type_transformers.get(&transform.entry.etl_type) {
                for transformer in transformers {
                    transform_entry(errors, stop_on_error, transformer.as_ref(), transform)?;
                }
            }
        }
        // New entries are transformed in turn, then loaded if not filtered.
        for new_entry in std::mem::take(&mut transform.new_entries).into_iter() {
            let mut new_transform = EntryTransform::new(new_entry);
            self.run_transformers(errors, stop_on_error, &mut new_transform, load_entries)?;
            if new_transform.loads_current() {
                load_entries.push(new_transform.entry.clone());
            }
            transform.new_entries.push(new_transform.entry);
        }
        Ok(())
    }
}

fn transform_entry(
    errors: &mut Vec<EtlError>,
    stop_on_error: bool,
    transformer: &dyn Transformer,
    transform: &mut EntryTransform,
) -> Result<(), StopError> {
    if let Err(err) = transformer.transform(transform) {
        let error = Arc::new(err);
        errors.push(EtlError {
            entry: transform.entry.clone(),
            error: Arc::clone(&error),
        });
        if stop_on_error {
            return Err(StopError { cause: error });
        }
        transform.load_current(false);
    }
    Ok(())
}

/// One run: an extractor, optional transformers, and a loader.
pub struct SimpleEtl {
    extractor: Option<Box<dyn Extractor>>,
    loader: Option<Box<dyn Loader>>,
    config: Option<TransformConfiguration>,
    messages: Vec<String>,
    extract_error: Option<Arc<Error>>,
    transform_errors: Vec<EtlError>,
    load_errors: Vec<EtlError>,
    stop_on_error: bool,
    timeout: Option<u32>,
    extract_count: u64,
    skip_count: u64,
    load_count: u64,
    run_millis: u64,
}

impl SimpleEtl {
    pub fn new(extractor: impl Extractor + 'static, loader: impl Loader + 'static) -> Self {
        Self::from_boxes(Box::new(extractor), Box::new(loader))
    }

    pub fn from_boxes(extractor: Box<dyn Extractor>, loader: Box<dyn Loader>) -> Self {
        Self {
            extractor: Some(extractor),
            loader: Some(loader),
            config: None,
            messages: Vec::new(),
            extract_error: None,
            transform_errors: Vec::new(),
            load_errors: Vec::new(),
            stop_on_error: false,
            timeout: Some(DEFAULT_TIMEOUT),
            extract_count: 0,
            skip_count: 0,
            load_count: 0,
            run_millis: 0,
        }
    }

    /// A transformer for any type, run in the order added.
    pub fn add_transformer(&mut self, transformer: impl Transformer + 'static) -> &mut Self {
        self.config
            .get_or_insert_with(TransformConfiguration::new)
            .add_transformer(transformer);
        self
    }

    /// A transformer for one type, run in order for entries of that type.
    pub fn add_type_transformer(&mut self, etl_type: &str, transformer: impl Transformer + 'static) -> &mut Self {
        self.config
            .get_or_insert_with(TransformConfiguration::new)
            .add_type_transformer(etl_type, transformer);
        self
    }

    /// Copy the transformers of an external configuration, so it is not changed.
    pub fn add_configuration(&mut self, config: &TransformConfiguration) -> &mut Self {
        self.config
            .get_or_insert_with(TransformConfiguration::new)
            .copy_from(config);
        self
    }

    /// Use a configuration as it is, replacing earlier transformers. Later adds change this one.
    pub fn set_configuration(&mut self, config: TransformConfiguration) -> &mut Self {
        self.config = Some(config);
        self
    }

    pub fn stop_on_error(&mut self) -> &mut Self {
        self.stop_on_error = true;
        self
    }

    /// Seconds, passed to [`Loader::init`] for transactions.
    pub fn set_timeout(&mut self, timeout: Option<u32>) -> &mut Self {
        self.timeout = timeout;
        self
    }

    /// Run the extract. An extract error is kept, not returned. The loader always completes.
    pub fn process(&mut self) -> &mut Self {
        let started = Instant::now();
        if let Some(loader) = self.loader.as_mut() {
            loader.init(self.timeout);
        }
        if let Some(mut extractor) = self.extractor.take() {
            if let Err(err) = extractor.extract(self) {
                self.extract_error = Some(Arc::new(err));
            }
            self.extractor = Some(extractor);
        }
        if let Some(mut loader) = self.loader.take() {
            loader.complete(self);
            self.loader = Some(loader);
        }
        self.run_millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self
    }

    pub fn extractor(&self) -> Option<&dyn Extractor> {
        self.extractor.as_deref()
    }

    pub fn loader(&self) -> Option<&dyn Loader> {
        self.loader.as_deref()
    }

    pub fn add_message(&mut self, message: impl Into<String>) -> &mut Self {
        self.messages.push(message.into());
        self
    }

    pub fn messages(&self) -> &[String] {
        &self.messages
    }

    pub fn extract_count(&self) -> u64 {
        self.extract_count
    }

    pub fn skip_count(&self) -> u64 {
        self.skip_count
    }

    pub fn load_count(&self) -> u64 {
        self.load_count
    }

    /// Milliseconds the last [`Self::process`] took.
    pub fn run_time(&self) -> u64 {
        self.run_millis
    }

    pub fn extract_error(&self) -> Option<&Error> {
        self.extract_error.as_deref()
    }

    pub fn transform_errors(&self) -> &[EtlError] {
        &self.transform_errors
    }

    pub fn load_errors(&self) -> &[EtlError] {
        &self.load_errors
    }

    pub fn has_error(&self) -> bool {
        self.extract_error.is_some() || !self.transform_errors.is_empty() || !self.load_errors.is_empty()
    }

    /// The extract error, else the first transform error, else the first load error.
    pub fn single_error_cause(&self) -> Option<&Error> {
        if let Some(err) = &self.extract_error {
            return Some(err);
        }
        if let Some(first) = self.transform_errors.first() {
            return Some(&first.error);
        }
        self.load_errors.first().map(|first| first.error.as_ref())
    }

    /// Called by the extractor for each record. True when the entry loaded.
    ///
    /// # Errors
    ///
    /// [`StopError`] when stop-on-error is set and a transform or load failed. The extractor
    /// should stop and return.
    pub fn process_entry(&mut self, entry: SimpleEntry) -> Result<bool, StopError> {
        self.extract_count += 1;
        let mut load_entries = Vec::new();
        match self.config.as_ref().filter(|config| config.has_transformers) {
            Some(config) => {
                let mut transform = EntryTransform::new(entry);
                config.run_transformers(
                    &mut self.transform_errors,
                    self.stop_on_error,
                    &mut transform,
                    &mut load_entries,
                )?;
                if transform.loads_current() {
                    load_entries.insert(0, transform.entry);
                } else if transform.new_entries.is_empty() {
                    self.skip_count += 1;
                    return Ok(false);
                }
            }
            None => load_entries.push(entry),
        }
        let Some(loader) = self.loader.as_mut() else {
            return Ok(false);
        };
        for mut load_entry in load_entries {
            match loader.load(&mut load_entry) {
                Ok(()) => self.load_count += 1,
                Err(err) => {
                    let error = Arc::new(err);
                    self.load_errors.push(EtlError {
                        entry: load_entry,
                        error: Arc::clone(&error),
                    });
                    if self.stop_on_error {
                        return Err(StopError { cause: error });
                    }
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

/// Reads a flat record file: the root element holds one element per record. A record's type is
/// its element name. Its attributes, and the text of each child element, are its values.
pub struct FlatXmlExtractor<R: Resource> {
    resource: Option<R>,
}

impl<R: Resource> FlatXmlExtractor<R> {
    pub fn new(resource: R) -> Self {
        Self {
            resource: Some(resource),
        }
    }
}

impl<R: Resource> Extractor for FlatXmlExtractor<R> {
    /// A missing resource extracts nothing. A parse error names the location. A stop ends the
    /// parse and is not an error.
    fn extract(&mut self, etl: &mut SimpleEtl) -> Result<(), Error> {
        let Some(resource) = self.resource.as_ref().filter(|resource| resource.exists()) else {
            return Ok(());
        };
        let location = resource.location().to_string();
        let text = resource.read_text()?;
        flat_xml(etl, &location, &text)
            .map_err(|err| Error::new(format!("Error parsing XML from {location}")).with_source(err))
    }
}

/// The SAX handler of `FlatXmlExtractor`, over pull events.
fn flat_xml(etl: &mut SimpleEtl, location: &str, text: &str) -> Result<(), Error> {
    let mut events = XmlEvents::new(Some(location), text);
    let mut root_name: Option<String> = None;
    let mut cur_entry: Option<SimpleEntry> = None;
    let mut cur_text_name: Option<String> = None;
    let mut cur_text = String::new();
    while let Some(event) = events.next_event()? {
        match event {
            XmlEvent::Start { name, attributes } => {
                if root_name.is_none() {
                    root_name = Some(name);
                    continue;
                }
                if cur_entry.is_none() {
                    let values = attributes
                        .into_iter()
                        .map(|(key, value)| (key, Value::String(value)))
                        .collect();
                    cur_entry = Some(SimpleEntry::new(name, values));
                } else {
                    cur_text_name = Some(name);
                    cur_text.clear();
                }
            }
            XmlEvent::Text(chars) => cur_text.push_str(&chars),
            XmlEvent::End(name) => {
                // A text field closing; one is only open inside an entry.
                if let Some(text_name) = cur_text_name.take() {
                    if let Some(entry) = cur_entry.as_mut() {
                        entry
                            .values
                            .insert(text_name, Value::String(std::mem::take(&mut cur_text)));
                    }
                    continue;
                }
                let Some(entry) = cur_entry.take() else {
                    // The root element closing in a flat record file.
                    if root_name.as_deref() == Some(name.as_str()) {
                        root_name = None;
                    }
                    continue;
                };
                if name != entry.etl_type {
                    return Err(Error::new(format!(
                        "Invalid close element {name}, was expecting {}",
                        entry.etl_type
                    )));
                }
                cur_text.clear();
                if etl.process_entry(entry).is_err() {
                    // Java logs the stop and ignores the rest of the document.
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "etl_tests.rs"]
mod tests;
