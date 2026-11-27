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

//! The shared types an editor's update adds, checked before it is applied (plans/Security-Review-Medium.md
//! SEC-TLS-118). yrs deletes a shared type by calling itself for each type nested in it, so an update that deleted a
//! type with tens of thousands of levels below it would overflow a worker's stack and end the process; the levels
//! cost a few bytes each, so the document size limit does not bound them. An update is refused when it:
//!
//! - names a root other than the codec's own (a wiki page's fragment, a board's three maps, a text's one text);
//! - adds a nested type of a kind the codec does not use (a page has XML elements and texts; a board and a text none),
//!   or a sub-document;
//! - adds a type whose place cannot be told from the update and the document (its parent, or the item it is put
//!   beside), or that would be more than [`MAX_TYPE_DEPTH`] levels below its root.
//!
//! The depth of each item's parent is kept per room ([`Nesting`]), from the scans of the updates applied and, when an
//! update names an item it does not know (one a seed, a merge or another server added), from a scan of the whole
//! document's state, at most once for each state. The update is read with yrs's own decoder, block by block, as
//! `Update::decode` reads it, so nothing of it is integrated before it passes.

use std::collections::{BTreeMap, HashMap};

use yrs::block::{
    ItemContent, BLOCK_GC_REF_NUMBER, BLOCK_SKIP_REF_NUMBER, HAS_ORIGIN, HAS_PARENT_SUB, HAS_RIGHT_ORIGIN,
};
use yrs::encoding::read::{Cursor, Read};
use yrs::types::TypeRef;
use yrs::updates::decoder::{Decoder, DecoderV1};
use yrs::updates::encoder::Encode;
use yrs::{ClientID, Doc, ReadTxn, StateVector, Transact, ID};

/// How many levels a shared type may be below its root: far more than an editor nests (a page's quotes and lists are
/// written at most [`crate::markdown::MAX_DEPTH`] deep), and far fewer than a worker's stack takes in yrs's delete.
pub const MAX_TYPE_DEPTH: u32 = 256;

/// The nested types a codec's documents hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Nested {
    /// None: a board's maps and a text hold plain values only.
    None,
    /// XML elements and XML texts, under a page's fragment.
    Xml,
}

/// What a codec's documents may hold: their roots and the kinds of nested types.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Shape {
    pub roots: &'static [&'static str],
    pub nested: Nested,
}

/// Where an item of an update goes, as its block says.
#[derive(Clone, Debug)]
enum Place {
    /// Directly under a root (depth 0).
    Root,
    /// Under the type whose item has this id.
    Under(ID),
    /// Beside the item of this id (its origin, or its right origin): under that item's parent.
    Beside(ID),
}

/// One item of an update: its first id and length, where it goes, whether it is a nested type, and whether its depth
/// is kept (a type, or deleted content, which may have been one: an item a later type is put beside).
#[derive(Clone, Debug)]
struct Item {
    id: ID,
    len: u32,
    place: Place,
    is_type: bool,
    kept: bool,
}

/// The depth of each known item's parent (0 for a root's children), and each nested type's own depth, for one room.
#[derive(Debug, Default)]
pub(crate) struct Nesting {
    /// By client: the first clock of each item, its length and its parent's depth.
    parents: HashMap<ClientID, BTreeMap<u32, (u32, u32)>>,
    /// The state the depths were last made complete for, encoded: a miss at this state is an unknown item.
    complete_at: Option<Vec<u8>>,
}

impl Nesting {
    /// The parent depth of the item that holds `id`, when it is known.
    fn parent_depth(&self, id: &ID) -> Option<u32> {
        let (start, (len, depth)) = self.parents.get(&id.client)?.range(..=id.clock).next_back()?;
        (id.clock < start + len).then_some(*depth)
    }

    fn insert(&mut self, id: &ID, len: u32, depth: u32) {
        self.parents
            .entry(id.client)
            .or_default()
            .insert(id.clock, (len, depth));
    }

    /// The depth of the nested type whose item is `id` (its parent's depth plus one).
    fn type_depth(&self, id: &ID) -> Option<u32> {
        self.parent_depth(id).map(|depth| depth + 1)
    }

    /// Check an editor's update against `shape` and the document `doc` it is for, and keep the depths of the items it
    /// adds. Nothing is kept for an update that is refused.
    ///
    /// # Errors
    ///
    /// Why the update is refused (the caller answers 422), or that it does not read (400).
    pub(crate) fn check(&mut self, doc: &Doc, shape: Shape, update: &[u8]) -> Result<(), (u16, String)> {
        let items = scan(update, shape)?;
        if shape.nested == Nested::None {
            // no nested type is taken at all ([`scan`]), so nothing nests
            return Ok(());
        }
        if items.iter().all(|item| !item.is_type) {
            // no type: plain content (text, values), which nests nothing; only a deletion is kept
            if items.iter().any(|item| item.kept) {
                let resolved = self.resolve(&items);
                self.keep(&items, &resolved);
            }
            return Ok(());
        }
        let mut resolved = self.resolve(&items);
        let unknown = |resolved: &[Option<u32>]| {
            items
                .iter()
                .zip(resolved)
                .any(|(item, depth)| item.is_type && depth.is_none())
        };
        if unknown(&resolved) {
            // an item a seed, a merge or another server added: once for each state, the whole document is read
            let state = doc.transact().state_vector().encode_v1();
            if self.complete_at.as_ref() != Some(&state) {
                self.rebuild(doc);
                self.complete_at = Some(state);
                resolved = self.resolve(&items);
            }
        }
        for (item, depth) in items.iter().zip(&resolved) {
            if !item.is_type {
                continue;
            }
            match depth {
                None => {
                    return Err((
                        422,
                        "the update adds a shared type whose place in the document is unknown".into(),
                    ))
                }
                Some(depth) if depth + 1 > MAX_TYPE_DEPTH => {
                    return Err((422, format!("shared types nest at most {MAX_TYPE_DEPTH} levels deep")))
                }
                Some(_) => {}
            }
        }
        self.keep(&items, &resolved);
        Ok(())
    }

    /// The parent depth of each of `items`, from the update itself and the depths kept, as far as they resolve. An item
    /// may be placed under or beside one later in the update, so each is followed to what it rests on with a stack of
    /// its own (no recursion, each item once); a cycle or an item nothing places resolves to none.
    fn resolve(&self, items: &[Item]) -> Vec<Option<u32>> {
        let mut index: HashMap<ClientID, BTreeMap<u32, usize>> = HashMap::new();
        for (at, item) in items.iter().enumerate() {
            index.entry(item.id.client).or_default().insert(item.id.clock, at);
        }
        let find = |id: &ID| -> Option<usize> {
            let (start, at) = index.get(&id.client)?.range(..=id.clock).next_back()?;
            (id.clock < start + items[*at].len).then_some(*at)
        };
        const NEW: u8 = 0;
        const VISITING: u8 = 1;
        const DONE: u8 = 2;
        let mut depths: Vec<Option<u32>> = vec![None; items.len()];
        let mut state = vec![NEW; items.len()];
        for first in 0..items.len() {
            let mut stack = vec![first];
            while let Some(&at) = stack.last() {
                if state[at] == DONE {
                    stack.pop();
                    continue;
                }
                state[at] = VISITING;
                // the item it rests on, in the update (its index, and whether its depth is the parent's: under a type,
                // the type's own depth; beside an item, that item's parent depth), or the depth kept for the document
                let (target, under) = match &items[at].place {
                    Place::Root => (None, false),
                    Place::Under(parent) => (Some(parent), true),
                    Place::Beside(other) => (Some(other), false),
                };
                let found = match target {
                    None => Ok(Some(0)),
                    Some(id) => match find(id) {
                        Some(other) if state[other] == DONE => Ok(depths[other].map(|depth| depth + u32::from(under))),
                        // a cycle in the update places nothing
                        Some(other) if state[other] == VISITING => Ok(None),
                        Some(other) => Err(other),
                        None if under => Ok(self.type_depth(id)),
                        None => Ok(self.parent_depth(id)),
                    },
                };
                match found {
                    Ok(depth) => {
                        depths[at] = depth;
                        state[at] = DONE;
                        stack.pop();
                    }
                    Err(next) => stack.push(next),
                }
            }
        }
        depths
    }

    /// Keep the depths of the types and deleted content of `items`: what a later type may be put under or beside. Text
    /// is not kept, so a long session of typing grows nothing here.
    fn keep(&mut self, items: &[Item], resolved: &[Option<u32>]) {
        for (item, depth) in items.iter().zip(resolved) {
            if let (true, Some(depth)) = (item.kept, depth) {
                self.insert(&item.id, item.len, *depth);
            }
        }
    }

    /// The depths of every item of the document, read from its whole state; what was kept before stays (an item a
    /// later state merged or deleted keeps its depth).
    fn rebuild(&mut self, doc: &Doc) {
        let state = doc.transact().encode_state_as_update_v1(&StateVector::default());
        let everything = Shape {
            roots: &[],
            nested: Nested::Xml,
        };
        match scan_unchecked(&state, everything) {
            Ok(items) => {
                let resolved = self.resolve(&items);
                self.keep(&items, &resolved);
            }
            Err((_, error)) => tracing::error!("co-edited document state does not read for its nesting: {error}"),
        }
    }
}

/// The items of an encoded update, checked against `shape`: a root it does not have, a nested type of a kind it does
/// not use, or a sub-document refuses the update.
fn scan(update: &[u8], shape: Shape) -> Result<Vec<Item>, (u16, String)> {
    let mut items = Vec::new();
    read_items(update, |item, root, kind| {
        if let Some(root) = root {
            if !shape.roots.contains(&root) {
                return Err((
                    422,
                    format!("the update names a root this document does not have ({root})"),
                ));
            }
        }
        match kind {
            Kind::Plain => {}
            Kind::Doc => return Err((422, "a co-edited document holds no sub-documents".into())),
            Kind::Type(allowed_xml) => {
                if shape.nested == Nested::None || !allowed_xml {
                    return Err((422, "the update adds a shared type this document does not hold".into()));
                }
            }
        }
        items.push(item);
        Ok(())
    })?;
    Ok(items)
}

/// The items of a document's own state, nothing refused.
fn scan_unchecked(state: &[u8], _shape: Shape) -> Result<Vec<Item>, (u16, String)> {
    let mut items = Vec::new();
    read_items(state, |item, _, _| {
        items.push(item);
        Ok(())
    })?;
    Ok(items)
}

/// What an item's content is, for [`scan`].
enum Kind {
    Plain,
    Doc,
    /// A nested type; true for an XML element or an XML text.
    Type(bool),
}

/// Each item block of an encoded v1 update, as yrs's `Update::decode` reads it, with the root it names (when it names
/// one) and its kind; GC and skip blocks only move the clock. The delete set after the blocks is not read.
fn read_items(
    update: &[u8],
    mut each: impl FnMut(Item, Option<&str>, Kind) -> Result<(), (u16, String)>,
) -> Result<(), (u16, String)> {
    let malformed = |err: yrs::encoding::read::Error| (400, format!("not a Yjs update: {err}"));
    let mut decoder = DecoderV1::new(Cursor::new(update));
    let clients: u32 = decoder.read_var().map_err(malformed)?;
    for _ in 0..clients {
        let blocks: u32 = decoder.read_var().map_err(malformed)?;
        let client = decoder.read_client().map_err(malformed)?;
        let mut clock: u32 = decoder.read_var().map_err(malformed)?;
        for _ in 0..blocks {
            let info = decoder.read_info().map_err(malformed)?;
            match info {
                BLOCK_SKIP_REF_NUMBER => {
                    let len: u32 = decoder.read_var().map_err(malformed)?;
                    clock = clock.saturating_add(len);
                }
                BLOCK_GC_REF_NUMBER => {
                    let len = decoder.read_len().map_err(malformed)?;
                    clock = clock.saturating_add(len);
                }
                info => {
                    let origin = if info & HAS_ORIGIN != 0 {
                        Some(decoder.read_left_id().map_err(malformed)?)
                    } else {
                        None
                    };
                    let right_origin = if info & HAS_RIGHT_ORIGIN != 0 {
                        Some(decoder.read_right_id().map_err(malformed)?)
                    } else {
                        None
                    };
                    let mut root: Option<String> = None;
                    let place = match (origin, right_origin) {
                        (Some(beside), _) | (None, Some(beside)) => Place::Beside(beside),
                        (None, None) => {
                            let place = if decoder.read_parent_info().map_err(malformed)? {
                                root = Some(decoder.read_string().map_err(malformed)?.to_string());
                                Place::Root
                            } else {
                                Place::Under(decoder.read_left_id().map_err(malformed)?)
                            };
                            if info & HAS_PARENT_SUB != 0 {
                                decoder.read_string().map_err(malformed)?;
                            }
                            place
                        }
                    };
                    let content = ItemContent::decode(&mut decoder, info).map_err(malformed)?;
                    let len = content.len_utf16();
                    let kind = match &content {
                        ItemContent::Type(branch) => {
                            Kind::Type(matches!(branch.type_ref(), TypeRef::XmlElement(_) | TypeRef::XmlText))
                        }
                        ItemContent::Doc(..) => Kind::Doc,
                        _ => Kind::Plain,
                    };
                    // an empty block takes no clock, as yrs skips it
                    if len == 0 {
                        continue;
                    }
                    let item = Item {
                        id: ID::new(client, clock),
                        len,
                        place,
                        is_type: matches!(kind, Kind::Type(_) | Kind::Doc),
                        kept: matches!(kind, Kind::Type(_) | Kind::Doc) || matches!(content, ItemContent::Deleted(_)),
                    };
                    clock = clock.saturating_add(len);
                    each(item, root.as_deref(), kind)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "nesting_tests.rs"]
mod tests;
