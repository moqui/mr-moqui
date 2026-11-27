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

//! The shared types an update adds, checked before it is applied (plans/archive/Security-Review-Medium.md SEC-TLS-118). yrs
//! deletes a shared type by calling itself for each type nested in it, so an update that deleted a type with tens of
//! thousands of levels below it would overflow a worker's stack and end the process; the levels cost a few bytes each,
//! so the document size limit does not bound them. An update, an editor's or another server's, is refused when it:
//!
//! - names a root other than the codec's own (a wiki page's fragment, a board's three maps, a text's one text);
//! - adds a nested type of a kind the codec does not use (a page has XML elements and texts; a board and a text none),
//!   or a sub-document;
//! - names a client twice, or skips clocks past the document's state for a client (an item yrs would put after a
//!   hole);
//! - adds an item whose place cannot be told from the update and the document (its parent, or the items it is put
//!   between), or a type that would be more than [`MAX_TYPE_DEPTH`] levels below its root.
//!
//! The parent depth of each item of the document is kept per room ([`Nesting`]), text included (a run typed in one go
//! is one entry), from the updates applied and, when an update names an item of the document that is not kept (one a
//! seed or a merge added, or one from before the room's depths were read), from a scan of the whole document's state,
//! at most once for each state. What is kept is never written over: an update that claims an item the document has
//! (yrs skips it) gets that item's true depth, never the one the update claims. The update is read with yrs's own
//! decoder, block by block, as `Update::decode` reads it, so nothing of it is integrated before it passes.

use std::collections::{BTreeMap, HashMap, HashSet};

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

/// The depth kept for an item yrs garbage-collected, or will (one whose parent is gone): it holds nothing.
const GONE: u32 = u32::MAX;

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
    /// Between its origin and its right origin: under the parent of the one yrs finds (the left one first, the right
    /// one when the left is gone); the deeper of the two is taken.
    Beside { left: Option<ID>, right: Option<ID> },
    /// A garbage-collected range: nowhere.
    Gone,
}

/// One item of an update (or a part of one): its first id and length, where it goes, and whether it is a nested type.
#[derive(Clone, Debug)]
struct Item {
    id: ID,
    len: u32,
    place: Place,
    is_type: bool,
}

/// An update that passed [`Nesting::check`]: the parent depths of the items it adds, kept by [`Nesting::keep`] once
/// the update is applied.
#[derive(Debug, Default)]
#[must_use]
pub(crate) struct Checked {
    added: Vec<(ID, u32, u32)>,
}

/// The parent depth of each known item of one room's document (0 for a root's children, [`GONE`] for a collected one).
#[derive(Debug, Default)]
pub(crate) struct Nesting {
    /// By client: the first clock of each run of items, its length and its items' parent depth.
    parents: HashMap<ClientID, BTreeMap<u32, (u32, u32)>>,
    /// The state the depths were last made complete for, encoded: a miss at this state is an item the document does
    /// not have.
    complete_at: Option<Vec<u8>>,
    /// The room's document key, for the error a read of the whole state logs.
    key: String,
}

impl Nesting {
    /// The depths of the room of document `key`, none kept yet.
    pub(crate) fn for_room(key: &str) -> Self {
        Self {
            key: key.to_string(),
            ..Self::default()
        }
    }

    /// The parent depth of the item that holds `id`, when it is kept.
    fn parent_depth(&self, id: &ID) -> Option<u32> {
        let (start, (len, depth)) = self.parents.get(&id.client)?.range(..=id.clock).next_back()?;
        (id.clock < start + len).then_some(*depth)
    }

    /// Whether every clock of `len` from `id` is kept.
    fn covers(&self, id: &ID, len: u32) -> bool {
        let Some(runs) = self.parents.get(&id.client) else {
            return false;
        };
        let end = id.clock.saturating_add(len);
        let mut at = id.clock;
        while at < end {
            match runs.range(..=at).next_back() {
                Some((start, (run, _))) if at < start + run => at = start + run,
                _ => return false,
            }
        }
        true
    }

    /// Keep `depth` for the clocks of `len` from `id` that are not kept yet: what is kept is never written over. A run
    /// that follows the one before at the same depth (text typed in one go) extends it.
    fn insert(&mut self, id: &ID, len: u32, depth: u32) {
        let runs = self.parents.entry(id.client).or_default();
        let end = id.clock.saturating_add(len);
        let mut at = id.clock;
        while at < end {
            if let Some((start, (run, _))) = runs.range(..=at).next_back() {
                if at < start + run {
                    at = start + run;
                    continue;
                }
            }
            let next = runs.range(at + 1..).next().map_or(end, |(start, _)| (*start).min(end));
            let joined = runs
                .range(..at)
                .next_back()
                .filter(|(start, (run, kept))| *start + run == at && *kept == depth)
                .map(|(start, _)| *start);
            match joined {
                Some(start) => {
                    if let Some((run, _)) = runs.get_mut(&start) {
                        *run += next - at;
                    }
                }
                None => {
                    runs.insert(at, (next - at, depth));
                }
            }
            at = next;
        }
    }

    /// Check an update against `shape` and the document `doc` it is for; the depths it adds are kept by
    /// [`Nesting::keep`] once it is applied.
    ///
    /// # Errors
    ///
    /// Why the update is refused (the caller answers 422), or that it does not read (400).
    pub(crate) fn check(&mut self, doc: &Doc, shape: Shape, update: &[u8]) -> Result<Checked, (u16, String)> {
        let items = scan(update, shape)?;
        if shape.nested == Nested::None {
            // no nested type is taken at all ([`scan`]), so nothing nests
            return Ok(Checked::default());
        }
        let state = doc.transact().state_vector();
        no_skipped_clocks(&items, &state)?;
        let mut parts = self.parts(&items, &state);
        let mut resolved = self.resolve(&parts);
        if self.misses_the_document(&parts, &resolved, &state) {
            // an item of the document that is not kept (a seed, a merge): once for each state, the whole document is
            // read; an item the document does not have either (above its state) never costs a read
            let encoded = state.encode_v1();
            if self.complete_at.as_ref() != Some(&encoded) {
                self.rebuild(doc);
                self.complete_at = Some(encoded);
                parts = self.parts(&items, &state);
                resolved = self.resolve(&parts);
            }
        }
        let mut added = Vec::new();
        for (part, depth) in parts.iter().zip(&resolved) {
            let Some(depth) = *depth else {
                return Err((
                    422,
                    "the update adds an item whose place in the document is unknown".into(),
                ));
            };
            if part.is_type && depth != GONE && depth + 1 > MAX_TYPE_DEPTH {
                return Err((422, format!("shared types nest at most {MAX_TYPE_DEPTH} levels deep")));
            }
            added.push((part.id, part.len, depth));
        }
        Ok(Checked { added })
    }

    /// Keep the depths of an update [`Nesting::check`] passed, once it is applied.
    pub(crate) fn keep(&mut self, checked: Checked) {
        for (id, len, depth) in checked.added {
            self.insert(&id, len, depth);
        }
    }

    /// The parts of `items` the document does not have kept: an item below the document's state that is kept whole is
    /// one yrs skips, and its part above the state is put after the part below, as yrs splits it.
    fn parts(&self, items: &[Item], state: &StateVector) -> Vec<Item> {
        let mut parts = Vec::new();
        for item in items {
            let known = state.get(&item.id.client);
            let end = item.id.clock.saturating_add(item.len);
            if item.id.clock < known {
                let below = known.min(end) - item.id.clock;
                if !self.covers(&item.id, below) {
                    // not kept: a hole the document has (yrs fills it), or an item not read yet ([`Self::rebuild`])
                    parts.push(Item {
                        len: below,
                        ..item.clone()
                    });
                }
                if end > known {
                    let place = match &item.place {
                        Place::Gone => Place::Gone,
                        Place::Beside { right, .. } => Place::Beside {
                            left: Some(ID::new(item.id.client, known - 1)),
                            right: *right,
                        },
                        Place::Root | Place::Under(_) => Place::Beside {
                            left: Some(ID::new(item.id.client, known - 1)),
                            right: None,
                        },
                    };
                    parts.push(Item {
                        id: ID::new(item.id.client, known),
                        len: end - known,
                        place,
                        is_type: item.is_type,
                    });
                }
            } else {
                parts.push(item.clone());
            }
        }
        parts
    }

    /// Whether `parts` name an item below the document's state that is not kept: a part itself, or what an unplaced
    /// part rests on.
    fn misses_the_document(&self, parts: &[Item], resolved: &[Option<u32>], state: &StateVector) -> bool {
        let in_document = |id: &ID| id.clock < state.get(&id.client) && self.parent_depth(id).is_none();
        parts.iter().zip(resolved).any(|(part, depth)| {
            in_document(&part.id)
                || (depth.is_none()
                    && match &part.place {
                        Place::Under(id) => in_document(id),
                        Place::Beside { left, right } => left.iter().chain(right.iter()).any(&in_document),
                        Place::Root | Place::Gone => false,
                    })
        })
    }

    /// The parent depth of each of `parts`: what is kept for an item first (the document's own, never what an update
    /// claims for it), then the update's other parts. A part may rest on one later in the update, so each is followed
    /// to what it rests on with a stack of its own (no recursion, each part once); a cycle, or an item neither kept nor
    /// in the update, resolves to none.
    fn resolve(&self, parts: &[Item]) -> Vec<Option<u32>> {
        let mut index: HashMap<ClientID, BTreeMap<u32, usize>> = HashMap::new();
        for (at, part) in parts.iter().enumerate() {
            index.entry(part.id.client).or_default().insert(part.id.clock, at);
        }
        let find = |id: &ID| -> Option<usize> {
            let (start, at) = index.get(&id.client)?.range(..=id.clock).next_back()?;
            (id.clock < start + parts[*at].len).then_some(*at)
        };
        const NEW: u8 = 0;
        const VISITING: u8 = 1;
        const DONE: u8 = 2;
        let mut depths: Vec<Option<u32>> = vec![None; parts.len()];
        let mut state = vec![NEW; parts.len()];
        for first in 0..parts.len() {
            let mut stack = vec![first];
            'next: while let Some(&at) = stack.last() {
                if state[at] == DONE {
                    stack.pop();
                    continue;
                }
                state[at] = VISITING;
                // the parent depth of the item `id` holds: kept, or of a part done (or none: a cycle, or unknown)
                let mut wait = None;
                let mut parent_depth = |id: &ID| -> Option<u32> {
                    if let Some(depth) = self.parent_depth(id) {
                        return Some(depth);
                    }
                    match find(id) {
                        Some(other) if state[other] == DONE => depths[other],
                        Some(other) if state[other] == NEW => {
                            wait = Some(other);
                            None
                        }
                        _ => None,
                    }
                };
                let depth = match &parts[at].place {
                    Place::Root => Some(0),
                    Place::Gone => Some(GONE),
                    Place::Under(parent) => {
                        parent_depth(parent).map(|depth| if depth == GONE { GONE } else { depth + 1 })
                    }
                    Place::Beside { left, right } => {
                        let sides: Vec<Option<u32>> = left.iter().chain(right.iter()).map(&mut parent_depth).collect();
                        if sides.iter().any(Option::is_none) {
                            None
                        } else {
                            let live = sides.iter().flatten().copied().filter(|depth| *depth != GONE).max();
                            Some(live.unwrap_or(GONE))
                        }
                    }
                };
                if let Some(other) = wait {
                    stack.push(other);
                    continue 'next;
                }
                depths[at] = depth;
                state[at] = DONE;
                stack.pop();
            }
        }
        depths
    }

    /// The depths of every item of the document, read from its whole state; what was kept before stays.
    fn rebuild(&mut self, doc: &Doc) {
        let state = doc.transact().encode_state_as_update_v1(&StateVector::default());
        let mut items = Vec::new();
        match read_items(&state, |item, _, _| {
            items.push(item);
            Ok(())
        }) {
            Ok(()) => {
                let resolved = self.resolve(&items);
                let mut deepest = 0;
                for (item, depth) in items.iter().zip(resolved) {
                    if let Some(depth) = depth {
                        if item.is_type && depth != GONE {
                            deepest = deepest.max(depth + 1);
                        }
                        self.insert(&item.id, item.len, depth);
                    }
                }
                // a state merged in unchecked (another server's save of an older build): a delete of its top type could
                // overflow the stack, so it is named for an administrator to repair (SEC-TLS-118)
                if deepest > MAX_TYPE_DEPTH {
                    tracing::error!(
                        "co-edited {}: its document nests shared types {deepest} levels deep, past {MAX_TYPE_DEPTH}; \
                         save the page from its text and drop the co-edited row",
                        self.key
                    );
                }
            }
            Err((_, error)) => tracing::error!("co-edited document state does not read for its nesting: {error}"),
        }
    }
}

/// Refuse `items` that skip clocks past the document's `state` for a client: yrs would put a hole before them, which a
/// later update fills with an item the depths kept do not tell.
fn no_skipped_clocks(items: &[Item], state: &StateVector) -> Result<(), (u16, String)> {
    let mut ends: HashMap<ClientID, u32> = HashMap::new();
    for item in items {
        let end = ends.entry(item.id.client).or_insert_with(|| state.get(&item.id.client));
        if item.id.clock > *end {
            return Err((
                422,
                "the update skips clocks this document does not have: load the document again".into(),
            ));
        }
        *end = (*end).max(item.id.clock.saturating_add(item.len));
    }
    Ok(())
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

/// What an item's content is, for [`scan`].
enum Kind {
    Plain,
    Doc,
    /// A nested type; true for an XML element or an XML text.
    Type(bool),
}

/// Each block of an encoded v1 update, as yrs's `Update::decode` reads it, with the root it names (when it names one)
/// and its kind: an item, or a garbage-collected range (gone); a skip block only moves the clock. A client named twice
/// refuses it. The delete set after the blocks is not read.
fn read_items(
    update: &[u8],
    mut each: impl FnMut(Item, Option<&str>, Kind) -> Result<(), (u16, String)>,
) -> Result<(), (u16, String)> {
    let malformed = |err: yrs::encoding::read::Error| (400, format!("not a Yjs update: {err}"));
    let mut decoder = DecoderV1::new(Cursor::new(update));
    let clients: u32 = decoder.read_var().map_err(malformed)?;
    let mut seen = HashSet::new();
    for _ in 0..clients {
        let blocks: u32 = decoder.read_var().map_err(malformed)?;
        let client = decoder.read_client().map_err(malformed)?;
        if !seen.insert(client) {
            return Err((400, format!("not a Yjs update: client {client} is named twice")));
        }
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
                    if len > 0 {
                        let item = Item {
                            id: ID::new(client, clock),
                            len,
                            place: Place::Gone,
                            is_type: false,
                        };
                        each(item, None, Kind::Plain)?;
                    }
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
                    let place = if origin.is_some() || right_origin.is_some() {
                        Place::Beside {
                            left: origin,
                            right: right_origin,
                        }
                    } else {
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
