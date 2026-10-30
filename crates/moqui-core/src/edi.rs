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

//! `EdiHandler`: X12 and EDIFACT text to nested maps and back.
//!
//! A segment is a map with an `elements` entry, a list whose first item is the segment ID, and an
//! entry per child segment ID holding a list of those segment maps. An element is a string, or a
//! list of strings for a composite. The root map has no `elements`. Segments with no definition
//! are kept as text in `originalList` when only the envelope is loaded.
//!
//! The envelope and body structures are Bots grammars: a list of `{ID, MIN, MAX, LEVEL}` maps,
//! `LEVEL` being the child list. Java runs a Groovy script for them. Here the file at the location
//! is a JavaScript expression that evaluates to that list, the Groovy literal with `[:]` maps
//! written as `{}` objects. The handler runs on the thread's `ExecutionContext`, which is why it
//! lives in core.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};

/// One segment map: `elements` and child segment lists.
pub type EdiMap = IndexMap<String, Value>;

const ELEMENTS: &str = "elements";
const ORIGINAL_LIST: &str = "originalList";

/// One entry of a grammar structure. MIN, MAX, QUERIES, and SUBTRANSLATION are not used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentDef {
    pub id: String,
    pub level: Vec<SegmentDef>,
}

impl SegmentDef {
    /// Read a grammar list from plain values.
    ///
    /// # Errors
    ///
    /// When the value is not a list of maps with an `ID`.
    pub fn list_from_value(value: &Value) -> Result<Vec<SegmentDef>, Error> {
        let Value::List(items) = value else {
            return Err(Error::new("EDI structure is not a list"));
        };
        items.iter().map(Self::from_value).collect()
    }

    fn from_value(value: &Value) -> Result<SegmentDef, Error> {
        let Value::Map(entries) = value else {
            return Err(Error::new("EDI structure entry is not a map"));
        };
        let id = entries
            .get("ID")
            .filter(|id| !id.is_null())
            .map(to_plain_string)
            .ok_or_else(|| Error::new("EDI structure entry has no ID"))?;
        let level = match entries.get("LEVEL") {
            None | Some(Value::Null) => Vec::new(),
            Some(level) => Self::list_from_value(level)?,
        };
        Ok(SegmentDef { id, level })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentErrorType {
    UnrecognizedSegmentId,
    Unexpected,
    MandatoryMissing,
    LoopOverMax,
    ExceedsMaximumUse,
    NotDefinedInTxSet,
    NotInSequence,
    ElementErrors,
}

impl SegmentErrorType {
    /// X12 AK304 code.
    pub fn x12_code(self) -> &'static str {
        match self {
            Self::UnrecognizedSegmentId => "1",
            Self::Unexpected => "2",
            Self::MandatoryMissing => "3",
            Self::LoopOverMax => "4",
            Self::ExceedsMaximumUse => "5",
            Self::NotDefinedInTxSet => "6",
            Self::NotInSequence => "7",
            Self::ElementErrors => "8",
        }
    }

    /// The Java enum constant name.
    pub fn name(self) -> &'static str {
        match self {
            Self::UnrecognizedSegmentId => "UNRECOGNIZED_SEGMENT_ID",
            Self::Unexpected => "UNEXPECTED",
            Self::MandatoryMissing => "MANDATORY_MISSING",
            Self::LoopOverMax => "LOOP_OVER_MAX",
            Self::ExceedsMaximumUse => "EXCEEDS_MAXIMUM_USE",
            Self::NotDefinedInTxSet => "NOT_DEFINED_IN_TX_SET",
            Self::NotInSequence => "NOT_IN_SEQUENCE",
            Self::ElementErrors => "ELEMENT_ERRORS",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        [
            Self::UnrecognizedSegmentId,
            Self::Unexpected,
            Self::MandatoryMissing,
            Self::LoopOverMax,
            Self::ExceedsMaximumUse,
            Self::NotDefinedInTxSet,
            Self::NotInSequence,
            Self::ElementErrors,
        ]
        .into_iter()
        .find(|kind| kind.name() == name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElementErrorType {
    MandatoryMissing,
    ConditionalRequiredMissing,
    TooMany,
    TooShort,
    TooLong,
    InvalidChar,
    InvalidCode,
    InvalidDate,
    InvalidTime,
    ExclusionViolated,
}

impl ElementErrorType {
    const ALL: [ElementErrorType; 10] = [
        Self::MandatoryMissing,
        Self::ConditionalRequiredMissing,
        Self::TooMany,
        Self::TooShort,
        Self::TooLong,
        Self::InvalidChar,
        Self::InvalidCode,
        Self::InvalidDate,
        Self::InvalidTime,
        Self::ExclusionViolated,
    ];

    /// X12 AK403 code.
    pub fn x12_code(self) -> &'static str {
        match self {
            Self::MandatoryMissing => "1",
            Self::ConditionalRequiredMissing => "2",
            Self::TooMany => "3",
            Self::TooShort => "4",
            Self::TooLong => "5",
            Self::InvalidChar => "6",
            Self::InvalidCode => "7",
            Self::InvalidDate => "8",
            Self::InvalidTime => "9",
            Self::ExclusionViolated => "10",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::MandatoryMissing => "MANDATORY_MISSING",
            Self::ConditionalRequiredMissing => "CONDITIONAL_REQUIRED_MISSING",
            Self::TooMany => "TOO_MANY",
            Self::TooShort => "TOO_SHORT",
            Self::TooLong => "TOO_LONG",
            Self::InvalidChar => "INVALID_CHAR",
            Self::InvalidCode => "INVALID_CODE",
            Self::InvalidDate => "INVALID_DATE",
            Self::InvalidTime => "INVALID_TIME",
            Self::ExclusionViolated => "EXCLUSION_VIOLATED",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementError {
    pub error_type: ElementErrorType,
    pub element_position: i64,
    pub composite_position: Option<i64>,
    pub element_text: String,
}

impl ElementError {
    /// The X12 AK4 segment for this error.
    pub fn make_ak4(&self) -> EdiMap {
        let position = Value::string(self.element_position.to_string());
        let position = match self.composite_position.filter(|composite| *composite != 0) {
            Some(composite) => Value::List(vec![position, Value::string(composite.to_string())]),
            None => position,
        };
        let mut ak4 = EdiMap::new();
        ak4.insert(
            ELEMENTS.to_string(),
            Value::List(vec![
                Value::string("AK4"),
                position,
                Value::string(self.error_type.x12_code()),
                Value::string(self.element_text.clone()),
            ]),
        );
        ak4
    }
}

/// A segment the body structure does not define. Scripts see it as a plain map
/// ([`SegmentError::to_value`]), since it passes through service parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentError {
    pub error_type: SegmentErrorType,
    pub segment_index: i64,
    pub position_in_tx_set: i64,
    pub segment_id: String,
    pub segment_text: String,
    pub element_errors: Vec<ElementError>,
}

impl SegmentError {
    /// The X12 AK3 segment for this error, with an AK4 for each element error. Mantle
    /// `produce#X12FunctionalAck` uses this.
    pub fn make_ak3(&self) -> EdiMap {
        let mut ak3 = EdiMap::new();
        ak3.insert(
            ELEMENTS.to_string(),
            Value::List(vec![
                Value::string("AK3"),
                Value::string(self.segment_id.clone()),
                Value::string(self.position_in_tx_set.to_string()),
                Value::string(""),
                Value::string(self.error_type.x12_code()),
            ]),
        );
        if !self.element_errors.is_empty() {
            let ak4 = self
                .element_errors
                .iter()
                .map(|error| Value::Map(error.make_ak4()))
                .collect();
            ak3.insert("AK4".to_string(), Value::List(ak4));
        }
        ak3
    }

    /// The fields by their Java property names, the type as its enum name.
    pub fn to_value(&self) -> Value {
        let mut map = IndexMap::new();
        map.insert("errorType".to_string(), Value::string(self.error_type.name()));
        map.insert("segmentIndex".to_string(), Value::I64(self.segment_index));
        map.insert("positionInTxSet".to_string(), Value::I64(self.position_in_tx_set));
        map.insert("segmentId".to_string(), Value::string(self.segment_id.clone()));
        map.insert("segmentText".to_string(), Value::string(self.segment_text.clone()));
        let elements = self
            .element_errors
            .iter()
            .map(|error| {
                let mut entry = IndexMap::new();
                entry.insert("errorType".to_string(), Value::string(error.error_type.name()));
                entry.insert("elementPosition".to_string(), Value::I64(error.element_position));
                entry.insert(
                    "compositePosition".to_string(),
                    error.composite_position.map_or(Value::Null, Value::I64),
                );
                entry.insert("elementText".to_string(), Value::string(error.element_text.clone()));
                Value::Map(entry)
            })
            .collect();
        map.insert("elementErrors".to_string(), Value::List(elements));
        Value::Map(map)
    }

    /// Read back the map [`Self::to_value`] made.
    ///
    /// # Errors
    ///
    /// When the value is not such a map.
    pub fn from_value(value: &Value) -> Result<SegmentError, Error> {
        let Value::Map(map) = value else {
            return Err(Error::new("segment error is not a map"));
        };
        let error_type = map
            .get("errorType")
            .map(to_plain_string)
            .and_then(|name| SegmentErrorType::from_name(&name))
            .ok_or_else(|| Error::new("segment error has no known errorType"))?;
        let mut element_errors = Vec::new();
        if let Some(Value::List(items)) = map.get("elementErrors") {
            for item in items {
                let Value::Map(entry) = item else {
                    return Err(Error::new("element error is not a map"));
                };
                let error_type = entry
                    .get("errorType")
                    .map(to_plain_string)
                    .and_then(|name| ElementErrorType::from_name(&name))
                    .ok_or_else(|| Error::new("element error has no known errorType"))?;
                element_errors.push(ElementError {
                    error_type,
                    element_position: int_of(entry.get("elementPosition")).unwrap_or(0),
                    composite_position: int_of(entry.get("compositePosition")),
                    element_text: entry.get("elementText").map(to_plain_string).unwrap_or_default(),
                });
            }
        }
        Ok(SegmentError {
            error_type,
            segment_index: int_of(map.get("segmentIndex")).unwrap_or(0),
            position_in_tx_set: int_of(map.get("positionInTxSet")).unwrap_or(0),
            segment_id: map.get("segmentId").map(to_plain_string).unwrap_or_default(),
            segment_text: map.get("segmentText").map(to_plain_string).unwrap_or_default(),
            element_errors,
        })
    }
}

fn int_of(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Null => None,
        Value::I32(number) => Some(i64::from(*number)),
        Value::I64(number) => Some(*number),
        Value::F64(number) => Some(number.trunc() as i64),
        other => to_plain_string(other).parse().ok(),
    }
}

/// Parses and generates EDI text against an envelope structure and, optionally, a body structure.
#[derive(Clone, Debug)]
pub struct EdiHandler {
    pub segment_terminator: Option<char>,
    pub element_separator: Option<char>,
    pub component_delimiter: Option<char>,
    pub escape_character: char,
    pub segment_suffix: Option<char>,
    /// The X12 ISA segment is fixed width. Each element is padded to its width.
    pub segment_element_sizes: HashMap<String, Vec<usize>>,
    pub padding_char: char,
    pub no_escape_segments: HashSet<String>,
    envelope: Option<Vec<SegmentDef>>,
    body: Option<Vec<SegmentDef>>,
    body_root_id: Option<String>,
    known_segment_ids: HashSet<String>,
    segment_errors: Option<Vec<SegmentError>>,
}

impl Default for EdiHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl EdiHandler {
    pub fn new() -> Self {
        Self {
            segment_terminator: None,
            element_separator: None,
            component_delimiter: None,
            escape_character: '?',
            segment_suffix: Some('\n'),
            segment_element_sizes: HashMap::from([(
                "ISA".to_string(),
                vec![3, 2, 10, 2, 10, 2, 15, 2, 15, 6, 4, 1, 5, 9, 1, 1, 1],
            )]),
            padding_char: '\u{a0}',
            no_escape_segments: HashSet::from(["ISA".to_string(), "UNA".to_string()]),
            envelope: None,
            body: None,
            body_root_id: None,
            known_segment_ids: HashSet::new(),
            segment_errors: None,
        }
    }

    /// Set the separators. A `None` takes the X12 default: `~`, `*`, `:`, and `?`.
    pub fn set_chars(
        &mut self,
        segment_terminator: Option<char>,
        element_separator: Option<char>,
        component_delimiter: Option<char>,
        escape_character: Option<char>,
    ) -> &mut Self {
        self.segment_terminator = Some(segment_terminator.unwrap_or('~'));
        self.element_separator = Some(element_separator.unwrap_or('*'));
        self.component_delimiter = Some(component_delimiter.unwrap_or(':'));
        self.escape_character = escape_character.unwrap_or('?');
        self
    }

    // NOTE: common X12 component delimiters include ':', '^', '<', and '@'.
    pub fn set_x12_default_chars(&mut self) -> &mut Self {
        self.set_chars(Some('~'), Some('*'), Some(':'), Some('?'))
    }

    pub fn set_itrade_default_chars(&mut self) -> &mut Self {
        self.set_chars(Some('~'), Some('*'), Some('@'), Some('?'))
    }

    pub fn set_edifact_default_chars(&mut self) -> &mut Self {
        self.set_chars(Some('\''), Some('+'), Some(':'), Some('?'))
    }

    /// The file envelope structure (for X12 the ISA, GS, and ST segments), from the JavaScript
    /// file at `location`.
    ///
    /// # Errors
    ///
    /// When the file cannot be read or run, or is not a structure list.
    pub fn load_envelope(&mut self, location: &str) -> Result<&mut Self, Error> {
        let value = read_structure(location)?;
        self.set_envelope(&value)
    }

    /// The envelope structure from plain values, as the script at the location evaluates to.
    ///
    /// # Errors
    ///
    /// When the value is not a structure list.
    pub fn set_envelope(&mut self, structure: &Value) -> Result<&mut Self, Error> {
        let defs = SegmentDef::list_from_value(structure)?;
        extract_segment_ids(&defs, &mut self.known_segment_ids);
        self.envelope = Some(defs);
        Ok(self)
    }

    /// The body structure, used under the envelope. Its first segment is where the envelope
    /// switches to the body (ST in X12).
    ///
    /// # Errors
    ///
    /// When the file cannot be read or run, or is not a structure list with a first segment.
    pub fn load_body(&mut self, location: &str) -> Result<&mut Self, Error> {
        let value = read_structure(location)?;
        self.set_body(&value)
    }

    /// The body structure from plain values.
    ///
    /// # Errors
    ///
    /// When the value is not a structure list with a first segment.
    pub fn set_body(&mut self, structure: &Value) -> Result<&mut Self, Error> {
        let defs = SegmentDef::list_from_value(structure)?;
        extract_segment_ids(&defs, &mut self.known_segment_ids);
        let root_id = defs
            .first()
            .map(|def| def.id.clone())
            .ok_or_else(|| Error::new("EDI body structure is empty"))?;
        self.body_root_id = Some(root_id);
        self.body = Some(defs);
        Ok(self)
    }

    /// Parse EDI text into the root segment map.
    ///
    /// A segment with no definition goes to the parent's `originalList` when no body is loaded,
    /// which supports an envelope-only parse that a body parse completes later. With a body it is a
    /// segment error.
    ///
    /// # Errors
    ///
    /// When no envelope is loaded, the text is empty, the separators cannot be found, or there are
    /// fewer than two segments.
    pub fn parse_text(&mut self, edi_text: &str) -> Result<EdiMap, Error> {
        let envelope = self
            .envelope
            .clone()
            .ok_or_else(|| Error::new("Cannot parse EDI text, envelope must be loaded"))?;
        if edi_text.is_empty() {
            return Err(Error::new("No EDI text passed"));
        }
        self.segment_errors = Some(Vec::new());
        self.determine_separators(edi_text)?;
        let terminator = self.segment_terminator.unwrap_or('~');
        let segments = java_split(edi_text, terminator, self.escape_character);
        if segments.len() < 2 {
            return Err(Error::new(format!(
                "No segments found in EDI text, using segment terminator [{terminator}]"
            )));
        }
        let mut root = EdiMap::new();
        let body = self.body.clone().unwrap_or_default();
        self.parse_segments(&segments, 0, &mut root, &envelope, &body)?;
        Ok(root)
    }

    /// The errors of the last parse, `None` before the first.
    pub fn segment_errors(&self) -> Option<&[SegmentError]> {
        self.segment_errors.as_deref()
    }

    /// EDI text from the map structure a parse makes.
    ///
    /// # Errors
    ///
    /// When a separator is not set, or the structure is not segment maps in lists.
    pub fn generate_text(&self, root: &EdiMap) -> Result<String, Error> {
        if self.segment_terminator.is_none() {
            return Err(Error::new("No segment terminator specified"));
        }
        if self.element_separator.is_none() {
            return Err(Error::new("No element separator specified"));
        }
        if self.component_delimiter.is_none() {
            return Err(Error::new("No component delimiter specified"));
        }
        let mut out = String::new();
        self.generate_segment(root, &mut out)?;
        Ok(out)
    }

    fn generate_segment(&self, segment: &EdiMap, out: &mut String) -> Result<(), Error> {
        let terminator = self.segment_terminator.unwrap_or('~');
        let separator = self.element_separator.unwrap_or('*');
        let delimiter = self.component_delimiter.unwrap_or(':');
        // Every segment has elements. The root map does not.
        if let Some(Value::List(elements)) = segment.get(ELEMENTS).filter(|value| !is_empty_list(value)) {
            let segment_id = elements.first().map(to_plain_string).unwrap_or_default();
            let sizes = self.segment_element_sizes.get(&segment_id);
            let no_escape = self.no_escape_segments.contains(&segment_id);
            for (index, element) in elements.iter().enumerate() {
                if let Value::List(components) = element {
                    // A composite: each component, joined by the component delimiter.
                    for (comp_index, component) in components.iter().enumerate() {
                        if !component.is_null() {
                            out.push_str(&self.escape(&to_plain_string(component)));
                        }
                        if comp_index + 1 < components.len() {
                            out.push(delimiter);
                        }
                    }
                } else {
                    let mut text = to_plain_string(element);
                    if !no_escape {
                        text = self.escape(&text);
                    }
                    out.push_str(&text);
                    if let Some(size) = sizes.and_then(|sizes| sizes.get(index)) {
                        let mut current = text.encode_utf16().count();
                        while current < *size {
                            out.push(self.padding_char);
                            current += 1;
                        }
                    }
                }
                if index + 1 < elements.len() {
                    out.push(separator);
                }
            }
            out.push(terminator);
            if let Some(suffix) = self.suffix() {
                out.push(suffix);
            }
        }
        for (key, value) in segment {
            let Value::List(items) = value else {
                return Err(Error::new(format!(
                    "Entry value is not a list: {key}={}",
                    to_plain_string(value)
                )));
            };
            if key == ELEMENTS {
                continue;
            }
            if key == ORIGINAL_LIST {
                // Literal child segments: the whole segment text except the terminator.
                for original in items {
                    out.push_str(&to_plain_string(original));
                    out.push(terminator);
                }
                continue;
            }
            for child in items {
                let Value::Map(child) = child else {
                    return Err(Error::new(format!(
                        "Expected Map for segment, got: {}",
                        to_plain_string(child)
                    )));
                };
                self.generate_segment(child, out)?;
            }
        }
        Ok(())
    }

    /// Find the separators from an ISA or UNA header, for any not already set.
    fn determine_separators(&mut self, edi_text: &str) -> Result<(), Error> {
        let char_at = |index: usize| edi_text.chars().nth(index);
        if edi_text.starts_with("ISA") {
            // X12
            if self.segment_terminator.is_none() {
                self.segment_terminator = Some(char_at(105).ok_or_else(|| short_header("ISA"))?);
            }
            if self.element_separator.is_none() {
                self.element_separator = Some(char_at(3).ok_or_else(|| short_header("ISA"))?);
            }
            if self.component_delimiter.is_none() {
                self.component_delimiter = Some(char_at(104).ok_or_else(|| short_header("ISA"))?);
            }
        } else if edi_text.starts_with("UNA") {
            // EDIFACT
            if self.segment_terminator.is_none() {
                self.segment_terminator = Some(char_at(8).ok_or_else(|| short_header("UNA"))?);
            }
            if self.element_separator.is_none() {
                self.element_separator = Some(char_at(4).ok_or_else(|| short_header("UNA"))?);
            }
            if self.component_delimiter.is_none() {
                self.component_delimiter = Some(char_at(3).ok_or_else(|| short_header("UNA"))?);
            }
        }
        if self.segment_terminator.is_none() {
            return Err(Error::new(
                "No segment terminator specified or automatically determined",
            ));
        }
        if self.element_separator.is_none() {
            return Err(Error::new("No element separator specified or automatically determined"));
        }
        if self.component_delimiter.is_none() {
            return Err(Error::new(
                "No component delimiter specified or automatically determined",
            ));
        }
        Ok(())
    }

    fn parse_segments(
        &mut self,
        segments: &[String],
        mut index: usize,
        current: &mut EdiMap,
        level_defs: &[SegmentDef],
        body: &[SegmentDef],
    ) -> Result<usize, Error> {
        while index < segments.len() {
            let segment_text = java_trim(&segments[index]);
            let Some(segment_id) = self.segment_id(segment_text) else {
                // A terminating character after the last segment terminator.
                tracing::info!(segment = %segment_text, "No ID found for segment");
                index += 1;
                continue;
            };
            if let Some(def) = level_defs.iter().find(|def| def.id == segment_id) {
                // parse_segment returns the next segment to process.
                index = self.parse_segment(segments, index, current, def, body)?;
            } else if !self.known_segment_ids.contains(&segment_id) {
                if !body.is_empty() {
                    // TODO in Java: handle more than one transaction set position.
                    let position = i64::try_from(index).unwrap_or(i64::MAX) - 2;
                    if let Some(errors) = self.segment_errors.as_mut() {
                        errors.push(SegmentError {
                            error_type: SegmentErrorType::NotDefinedInTxSet,
                            segment_index: i64::try_from(index).unwrap_or(i64::MAX),
                            position_in_tx_set: position,
                            segment_id,
                            segment_text: segment_text.to_string(),
                            element_errors: Vec::new(),
                        });
                    }
                } else {
                    // Kept as text, so an envelope-only parse can be finished with the body.
                    let original = current
                        .entry(ORIGINAL_LIST.to_string())
                        .or_insert_with(|| Value::List(Vec::new()));
                    if let Value::List(items) = original {
                        items.push(Value::string(segment_text));
                    }
                }
                index += 1;
            } else {
                // Not at this level: return so the parent checks it.
                return Ok(index);
            }
        }
        // Only the root segment gets here, after its final child.
        Ok(index)
    }

    fn parse_segment(
        &mut self,
        segments: &[String],
        index: usize,
        current: &mut EdiMap,
        def: &SegmentDef,
        body: &[SegmentDef],
    ) -> Result<usize, Error> {
        let elements = self.segment_elements(java_trim(&segments[index]))?;
        let segment_id = elements.first().map(to_plain_string).unwrap_or_default();
        let mut new_segment = EdiMap::new();
        new_segment.insert(ELEMENTS.to_string(), Value::List(elements));
        let mut next = index + 1;
        // A segment with children recurses. The body root switches from the envelope to the body.
        let mut level = def.level.as_slice();
        if level.is_empty() && self.body_root_id.as_deref() == Some(def.id.as_str()) {
            if let Some(first) = body.first() {
                level = first.level.as_slice();
            }
        }
        if !level.is_empty() {
            next = self.parse_segments(segments, next, &mut new_segment, level, body)?;
        }
        // Java adds the segment before its children are parsed. Nothing else touches `current`
        // meanwhile, so adding it after keeps the same order.
        let list = current.entry(segment_id).or_insert_with(|| Value::List(Vec::new()));
        if let Value::List(items) = list {
            items.push(Value::Map(new_segment));
        }
        Ok(next)
    }

    fn segment_id(&self, segment_text: &str) -> Option<String> {
        let separator = self.element_separator?;
        match segment_text.split_once(separator) {
            Some((id, _)) if !id.is_empty() => Some(id.to_string()),
            _ if segment_text.encode_utf16().count() <= 3 => Some(segment_text.to_string()),
            _ => None,
        }
    }

    fn segment_elements(&self, segment_text: &str) -> Result<Vec<Value>, Error> {
        let separator = self.element_separator.unwrap_or('*');
        let delimiter = self.component_delimiter.unwrap_or(':');
        let mut elements = Vec::new();
        for original in java_split(segment_text, separator, self.escape_character) {
            // A non-breaking space becomes a space before the trim.
            let original = original.replace('\u{a0}', " ");
            let original = java_trim(&original);
            if original.encode_utf16().count() >= 3 && original.contains(delimiter) {
                let components = java_split(original, delimiter, self.escape_character);
                if let [only] = components.as_slice() {
                    elements.push(Value::String(self.unescape(only)?));
                } else {
                    let mut list = Vec::with_capacity(components.len());
                    for component in &components {
                        list.push(Value::String(self.unescape(java_trim(component))?));
                    }
                    elements.push(Value::List(list));
                }
            } else {
                elements.push(Value::String(self.unescape(original)?));
            }
        }
        Ok(elements)
    }

    /// Split a message at each `root_header_id` that follows segments with no trailer, and after
    /// each `root_trailer_id`. Segments after the last trailer are dropped, as in Java.
    ///
    /// # Errors
    ///
    /// When the separators cannot be determined.
    pub fn split_message(
        &mut self,
        root_header_id: Option<&str>,
        root_trailer_id: Option<&str>,
        edi_text: &str,
    ) -> Result<Vec<String>, Error> {
        self.determine_separators(edi_text)?;
        let terminator = self.segment_terminator.unwrap_or('~');
        let mut splits = Vec::new();
        let mut current: Option<Vec<String>> = None;
        for segment in java_split(edi_text, terminator, self.escape_character) {
            let segment_text = java_trim(&segment).to_string();
            let segment_id = self.segment_id(&segment_text);
            let header = root_header_id.filter(|id| !id.is_empty());
            let trailer = root_trailer_id.filter(|id| !id.is_empty());
            if header.is_some()
                && segment_id.as_deref() == header
                && current.as_ref().is_some_and(|list| !list.is_empty())
            {
                // A header with no trailer before it: save what there is and start again.
                let previous = current.replace(vec![segment_text]).unwrap_or_default();
                splits.push(self.combine_segments(&previous));
            } else if trailer.is_some() && segment_id.as_deref() == trailer {
                let mut list = current.take().unwrap_or_default();
                list.push(segment_text);
                splits.push(self.combine_segments(&list));
            } else {
                current.get_or_insert_with(Vec::new).push(segment_text);
            }
        }
        Ok(splits)
    }

    pub fn combine_segments(&self, segments: &[String]) -> String {
        let terminator = self.segment_terminator.unwrap_or('~');
        let mut out = String::new();
        for segment in segments {
            out.push_str(segment);
            out.push(terminator);
            if let Some(suffix) = self.suffix() {
                out.push(suffix);
            }
        }
        out
    }

    /// Segments under `map`, not counting `map` itself.
    pub fn count_segments(&self, map: &EdiMap) -> usize {
        if map.len() <= 1 {
            return 0;
        }
        let mut count = 0;
        for (key, value) in map {
            if key == ELEMENTS {
                continue;
            }
            let Value::List(items) = value else {
                continue;
            };
            for item in items {
                if let Value::Map(child) = item {
                    if !child.is_empty() {
                        count += 1;
                    }
                    if child.len() > 1 {
                        count += self.count_segments(child);
                    }
                }
            }
        }
        count
    }

    /// Groovy truth on a `Character`: null and NUL are false.
    fn suffix(&self) -> Option<char> {
        self.segment_suffix.filter(|suffix| *suffix != '\0')
    }

    fn escape(&self, original: &str) -> String {
        let mut out = String::with_capacity(original.len());
        for ch in original.chars() {
            if self.needs_escape(ch) {
                out.push(self.escape_character);
            }
            out.push(ch);
        }
        out
    }

    fn needs_escape(&self, ch: char) -> bool {
        Some(ch) == self.component_delimiter
            || Some(ch) == self.element_separator
            || ch == self.escape_character
            || Some(ch) == self.segment_terminator
    }

    fn unescape(&self, original: &str) -> Result<String, Error> {
        let mut out = String::with_capacity(original.len());
        let mut chars = original.chars();
        while let Some(ch) = chars.next() {
            if ch == self.escape_character {
                // Skip it and keep the next character, which may be the escape character.
                let next = chars
                    .next()
                    .ok_or_else(|| Error::new(format!("EDI text ends with the escape character: {original}")))?;
                out.push(next);
            } else {
                out.push(ch);
            }
        }
        Ok(out)
    }
}

fn short_header(id: &str) -> Error {
    Error::new(format!("EDI text is too short for the {id} header"))
}

fn is_empty_list(value: &Value) -> bool {
    matches!(value, Value::List(items) if items.is_empty())
}

fn extract_segment_ids(defs: &[SegmentDef], ids: &mut HashSet<String>) {
    for def in defs {
        ids.insert(def.id.clone());
        extract_segment_ids(&def.level, ids);
    }
}

/// Run the structure file at `location`. It evaluates to the structure list.
fn read_structure(location: &str) -> Result<Value, Error> {
    let path = crate::context::locate(location)?;
    let source = std::fs::read_to_string(&path)
        .map_err(|err| Error::new(format!("could not read EDI structure {location}")).with_source(err))?;
    crate::script::eval_value(&source).map_err(|err| Error::new(format!("EDI structure {location}: {err}")))
}

/// `String.trim`.
fn java_trim(text: &str) -> &str {
    text.trim_matches(|ch: char| ch <= ' ')
}

/// `String.split` on `(?<!escape)separator`: a separator right after the escape character does
/// not split, and trailing empty strings are removed.
fn java_split(text: &str, separator: char, escape: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut previous: Option<char> = None;
    for ch in text.chars() {
        if ch == separator && previous != Some(escape) {
            parts.push(std::mem::take(&mut current));
        } else {
            current.push(ch);
        }
        previous = Some(ch);
    }
    parts.push(current);
    // A text with no separator stays whole, even when empty, as Java does.
    if parts.len() > 1 {
        while parts.last().is_some_and(String::is_empty) {
            parts.pop();
        }
    }
    parts
}

#[cfg(test)]
mod tests;
