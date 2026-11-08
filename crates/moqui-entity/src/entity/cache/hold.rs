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

//! The held writes of one transaction-cache frame (`cache` and `force-cache`), kept in flush order and indexed by
//! entity and primary key, so a write and a find touch only the rows of their entity and key, and the stored row under
//! a held update is read once per frame (plans/High-Level-Review.md HL-034).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use moqui_lib::error::Error;

use super::{write_pk, FieldMap, MemRow, Op};

/// One held write: the values as given (`plain`), as written to the database (`sealed`), and the primary key text.
pub(super) struct Held {
    pub(super) entity_name: String,
    pub(super) operation: Op,
    pub(super) plain: FieldMap,
    pub(super) sealed: FieldMap,
    pub(super) pk: String,
    /// The service on the artifact stack when the write was held, for the errors of a later find or of the flush
    /// (plans/archive/TX-Cache.md G5).
    pub(super) service: Option<Arc<str>>,
}

impl Held {
    /// What a flush or find error says of this write: its operation, entity, key and service.
    pub(super) fn describe(&self) -> String {
        let operation = match self.operation {
            Op::Create => "create",
            Op::Update => "update",
            Op::Delete => "delete",
            Op::Upsert => "upsert",
        };
        let key = self.pk.replace('\u{1f}', ", ");
        match &self.service {
            Some(service) => format!("{operation} of {} [{key}] held by {service}", self.entity_name),
            None => format!("{operation} of {} [{key}]", self.entity_name),
        }
    }
}

/// One frame's held writes.
#[derive(Default)]
pub(super) struct FrameHold {
    /// Every held write in flush order, so a child row follows its parent; `None` where a create and a later delete
    /// of its row collapsed.
    order: Vec<Option<Rc<Held>>>,
    entities: HashMap<String, EntityHold>,
    /// Rows kept in `stored` over every entity, up to [`READ_CAP`] (plans/archive/TX-Cache.md D2).
    kept: usize,
    /// The rows (entity, primary key text) a locking find of the frame read: no other transaction can delete one before
    /// the commit, so a held update of one needs no check that it changed a row.
    locked: HashSet<(String, String)>,
}

/// Rows a frame keeps of what it read; past it, nothing more is kept (plans/archive/TX-Cache.md D2). Reaching it is not a flush.
pub(super) const READ_CAP: usize = 10_000;

/// The rows of a list find as the database gave them, kept by the find's key (plans/archive/TX-Cache.md D3).
pub(super) type KeptList = Rc<Vec<Arc<FieldMap>>>;

#[derive(Default)]
struct EntityHold {
    /// Positions in `order` of the entity's writes. A collapsed write's position stays and reads `None`.
    slots: Vec<usize>,
    /// Positions in `order` of each row's writes, by primary key text.
    by_pk: HashMap<String, Vec<usize>>,
    /// Each row of the entity this frame read (by key, under a held update, or in a list), as the database has it;
    /// `None` when it has no such row (plans/archive/TX-Cache.md D2). The frame's own writes are applied on top when it reads:
    /// while it holds them they are not on the database, so these rows stay as read. A write of the frame to the
    /// database (an early flush, or any write in read-through mode) drops them.
    stored: HashMap<String, Option<Arc<FieldMap>>>,
    /// List finds of the entity with no limit or offset, by the find's key, as the database answered them
    /// (plans/archive/TX-Cache.md D3); dropped as `stored` is.
    lists: HashMap<String, KeptList>,
    /// Select-field finds of the entity (no limit, offset, lock or aggregate), by the find's key and fields, as the
    /// database answered them: the rows have those fields only, so no held write can be put on them, and they are
    /// dropped at the frame's first held write of the entity, as when `stored` is.
    projected: HashMap<String, KeptList>,
}

impl FrameHold {
    /// Hold `held` after the frame's other writes. A create of a row the frame holds (other than deleted) is refused;
    /// a delete of a row the frame created drops every held write of that row, and the delete itself.
    pub(super) fn push(&mut self, held: Held) -> Result<(), Error> {
        self.push_rc(Rc::new(held))
    }

    fn push_rc(&mut self, held: Rc<Held>) -> Result<(), Error> {
        let Self { order, entities, .. } = self;
        if !entities.contains_key(&held.entity_name) {
            entities.insert(held.entity_name.clone(), EntityHold::default());
        }
        let entity = entities
            .get_mut(&held.entity_name)
            .expect("the entity's hold was inserted just above");
        entity.projected.clear();
        let op_at = |at: Option<&usize>| at.and_then(|&at| order[at].as_ref()).map(|held| held.operation);
        let (first, last) = match entity.by_pk.get(&held.pk) {
            Some(slots) => (op_at(slots.first()), op_at(slots.last())),
            None => (None, None),
        };
        match held.operation {
            Op::Create if last.is_some_and(|op| op != Op::Delete) => {
                return Err(Error::new(format!(
                    "create of existing transaction cache row {} {}",
                    held.entity_name, held.pk
                )));
            }
            Op::Delete if first == Some(Op::Create) => {
                if let Some(slots) = entity.by_pk.remove(&held.pk) {
                    for at in slots {
                        order[at] = None;
                    }
                }
                return Ok(());
            }
            _ => {}
        }
        let at = order.len();
        entity.slots.push(at);
        match entity.by_pk.get_mut(&held.pk) {
            Some(slots) => slots.push(at),
            None => {
                entity.by_pk.insert(held.pk.clone(), vec![at]);
            }
        }
        order.push(Some(held));
        Ok(())
    }

    /// The last held operation of one row of `entity`; `None` when the frame holds none.
    pub(super) fn last_op(&self, entity: &str, pk: &str) -> Option<Op> {
        let slots = self.entities.get(entity)?.by_pk.get(pk)?;
        slots
            .iter()
            .rev()
            .find_map(|&at| self.order[at].as_ref())
            .map(|held| held.operation)
    }

    /// Whether the frame holds a write of `entity`.
    pub(super) fn holds_entity(&self, entity: &str) -> bool {
        self.entities.get(entity).is_some_and(|hold| !hold.by_pk.is_empty())
    }

    /// Take the held writes up to and including the last one of `entity`, in order, and keep holding the rest (an
    /// early flush of a selective find, plans/archive/TX-Cache.md D4). The stored rows are dropped, as the flush changes them.
    /// `None` when the frame holds no write of `entity`.
    pub(super) fn split_through(&mut self, entity: &str) -> Option<Vec<Rc<Held>>> {
        let last = self
            .entities
            .get(entity)?
            .slots
            .iter()
            .rev()
            .copied()
            .find(|&at| self.order[at].is_some())?;
        let mut order = std::mem::take(&mut self.order);
        let rest: Vec<Rc<Held>> = order.split_off(last + 1).into_iter().flatten().collect();
        self.entities.clear();
        self.kept = 0;
        for held in rest {
            // Each was accepted in this order before, so none is refused now.
            if let Err(err) = self.push_rc(held) {
                tracing::error!(error = %err, "a held write kept after a partial flush was refused");
            }
        }
        Some(order.into_iter().flatten().collect())
    }

    /// The held writes of `entity`, in order.
    pub(super) fn holds(&self, entity: &str) -> Vec<Rc<Held>> {
        self.entities
            .get(entity)
            .map(|hold| self.at(&hold.slots))
            .unwrap_or_default()
    }

    /// The held writes of one row of `entity`, in order.
    pub(super) fn holds_of(&self, entity: &str, pk: &str) -> Vec<Rc<Held>> {
        self.entities
            .get(entity)
            .and_then(|hold| hold.by_pk.get(pk))
            .map(|slots| self.at(slots))
            .unwrap_or_default()
    }

    fn at(&self, slots: &[usize]) -> Vec<Rc<Held>> {
        slots.iter().filter_map(|&at| self.order[at].clone()).collect()
    }

    /// The writes held, a collapsed one among them until the hold is written.
    pub(super) fn len(&self) -> usize {
        self.order.len()
    }

    /// Whether the frame holds no write.
    pub(super) fn is_empty(&self) -> bool {
        self.order.iter().all(Option::is_none)
    }

    /// Every held write in flush order, with the rows the frame locked.
    pub(super) fn into_parts(self) -> (impl Iterator<Item = Rc<Held>>, HashSet<(String, String)>) {
        (self.order.into_iter().flatten(), self.locked)
    }

    /// A row of `entity` a locking find read.
    pub(super) fn note_locked(&mut self, entity: &str, pk: &str) {
        self.locked.insert((entity.to_string(), pk.to_string()));
    }

    /// The stored row under a held update of `entity`, when this frame has read it: `Some(None)` when there is none.
    pub(super) fn stored(&self, entity: &str, pk: &str) -> Option<Option<Arc<FieldMap>>> {
        self.entities.get(entity)?.stored.get(pk).cloned()
    }

    /// Keep a row of `entity` as read now from the database; nothing past [`READ_CAP`] rows.
    pub(super) fn keep_stored(&mut self, entity: &str, pk: &str, row: Option<Arc<FieldMap>>) {
        if self.kept >= READ_CAP {
            return;
        }
        let hold = match self.entities.get_mut(entity) {
            Some(hold) => hold,
            None => self.entities.entry(entity.to_string()).or_default(),
        };
        if hold.stored.insert(pk.to_string(), row).is_none() {
            self.kept += 1;
        }
    }

    /// A list find of `entity` kept by its key.
    pub(super) fn kept_list(&self, entity: &str, key: &str) -> Option<KeptList> {
        self.entities.get(entity)?.lists.get(key).cloned()
    }

    /// Keep the rows of a list find of `entity`, and each row as a stored row, while the cap allows.
    pub(super) fn keep_list(&mut self, entity: &str, key: &str, rows: KeptList, pk_fields: &[String]) {
        if self.kept.saturating_add(rows.len()) > READ_CAP {
            return;
        }
        let mut buffer = String::new();
        for row in rows.iter() {
            buffer.clear();
            if super::write_pk(row, pk_fields, &mut buffer) {
                self.keep_stored(entity, &buffer, Some(Arc::clone(row)));
            }
        }
        let hold = self.entities.entry(entity.to_string()).or_default();
        hold.lists.insert(key.to_string(), rows);
    }

    /// Drop what the frame read of `entity`, or of every entity: the database changed under it.
    pub(super) fn forget_reads(&mut self, entity: Option<&str>) {
        for (name, hold) in &mut self.entities {
            if entity.is_none_or(|entity| entity == name) {
                self.kept = self.kept.saturating_sub(hold.stored.len());
                hold.stored.clear();
                hold.lists.clear();
                hold.projected.clear();
            }
        }
    }

    /// A select-field find of `entity` kept by its key, while the frame holds no write of the entity.
    pub(super) fn projected(&self, entity: &str, key: &str) -> Option<KeptList> {
        let hold = self.entities.get(entity)?;
        if !hold.slots.is_empty() {
            return None;
        }
        hold.projected.get(key).cloned()
    }

    /// Keep the rows of a select-field find of `entity` (with no held write of it), while the cap allows.
    pub(super) fn keep_projected(&mut self, entity: &str, key: String, rows: KeptList) {
        if self.kept.saturating_add(rows.len()) > READ_CAP {
            return;
        }
        let hold = self.entities.entry(entity.to_string()).or_default();
        if !hold.slots.is_empty() {
            return;
        }
        self.kept += rows.len();
        hold.projected.insert(key, rows);
    }

    /// The first held write of one row of `entity`.
    pub(super) fn first_op(&self, entity: &str, pk: &str) -> Option<Op> {
        let slots = self.entities.get(entity)?.by_pk.get(pk)?;
        slots
            .iter()
            .find_map(|&at| self.order[at].as_ref())
            .map(|held| held.operation)
    }
}

/// The loaded rows of a find with the frame's held writes of the entity to apply to them.
pub(super) struct Overlay<'h> {
    holds: &'h [Rc<Held>],
    rows: Vec<Option<MemRow>>,
    /// The row of each held primary key that is in `rows`, by position.
    at_pk: HashMap<&'h str, usize>,
    /// The first held write of each primary key.
    first: HashMap<&'h str, Op>,
}

impl<'h> Overlay<'h> {
    /// Index the loaded rows by the held primary keys. Each loaded row's key is written into one buffer, so a find
    /// allocates per held key, not per loaded row.
    pub(super) fn new(rows: Vec<MemRow>, holds: &'h [Rc<Held>], pk_fields: &[String]) -> Self {
        let mut first = HashMap::with_capacity(holds.len());
        for held in holds {
            first.entry(held.pk.as_str()).or_insert(held.operation);
        }
        let mut at_pk = HashMap::new();
        let mut buffer = String::new();
        for (index, row) in rows.iter().enumerate() {
            buffer.clear();
            if write_pk(&row.fields, pk_fields, &mut buffer) {
                if let Some((&pk, _)) = first.get_key_value(buffer.as_str()) {
                    at_pk.insert(pk, index);
                }
            }
        }
        Self {
            holds,
            rows: rows.into_iter().map(Some).collect(),
            at_pk,
            first,
        }
    }

    /// The held updates whose stored row the overlay needs: the first held write of the row is an update and the find
    /// did not load it. A row the frame created or deleted first does not start from the stored row.
    pub(super) fn unloaded_updates(&self) -> Vec<&'h Held> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for held in self.holds {
            let pk = held.pk.as_str();
            if matches!(self.first.get(pk), Some(Op::Update | Op::Upsert))
                && !self.at_pk.contains_key(pk)
                && seen.insert(pk)
            {
                out.push(held.as_ref());
            }
        }
        out
    }

    /// The loaded rows under a held update, by primary key, as stored rows to keep for the frame.
    pub(super) fn loaded_updates(&self) -> impl Iterator<Item = (&'h str, &Arc<FieldMap>)> + '_ {
        self.at_pk.iter().filter_map(|(&pk, &index)| {
            let update = matches!(self.first.get(pk), Some(Op::Update | Op::Upsert));
            let row = self.rows[index].as_ref().filter(|_| update)?;
            Some((pk, &row.fields))
        })
    }

    /// Apply the held writes in order. `stored` has the stored row of each of [`Self::unloaded_updates`]: an update
    /// starts from it, and an update of a row the database lacks (or the frame deleted) writes nothing, while an upsert
    /// of one is its whole row.
    ///
    /// A held create of a row the find loaded from the database, with no held delete of it before, fails here, as
    /// Java's `checkUpdateValue`, not at the commit (plans/archive/TX-Cache.md G5).
    pub(super) fn apply(mut self, stored: &HashMap<&str, Option<Arc<FieldMap>>>) -> Result<Vec<MemRow>, Error> {
        let loaded = self.rows.len();
        let mut deleted: HashSet<&str> = HashSet::new();
        for held in self.holds {
            let pk = held.pk.as_str();
            match held.operation {
                Op::Delete => {
                    if let Some(index) = self.at_pk.remove(pk) {
                        self.rows[index] = None;
                    }
                    deleted.insert(pk);
                }
                Op::Create => {
                    if let Some(index) = self.at_pk.remove(pk) {
                        if index < loaded && self.first.get(pk) == Some(&Op::Create) {
                            return Err(Error::new(format!(
                                "The {} in this transaction is of a record that is already in the database",
                                held.describe()
                            )));
                        }
                        self.rows[index] = None;
                    }
                    self.at_pk.insert(pk, self.rows.len());
                    self.rows.push(Some(MemRow {
                        fields: Arc::new(held.plain.clone()),
                        trusted: false,
                    }));
                }
                Op::Update | Op::Upsert => {
                    if let Some(row) = self.at_pk.get(pk).and_then(|&index| self.rows[index].as_mut()) {
                        let fields = Arc::make_mut(&mut row.fields);
                        for (key, value) in &held.plain {
                            fields.insert(key.clone(), value.clone());
                        }
                        row.trusted = false;
                        continue;
                    }
                    let base = if deleted.contains(pk) {
                        None
                    } else {
                        stored.get(pk).cloned().flatten()
                    };
                    let fields = match (base, held.operation) {
                        (Some(base), _) => {
                            let mut fields = FieldMap::clone(&base);
                            fields.extend(held.plain.iter().map(|(key, value)| (key.clone(), value.clone())));
                            fields
                        }
                        (None, Op::Upsert) => held.plain.clone(),
                        (None, _) => continue,
                    };
                    self.at_pk.insert(pk, self.rows.len());
                    self.rows.push(Some(MemRow {
                        fields: Arc::new(fields),
                        trusted: false,
                    }));
                }
            }
        }
        Ok(self.rows.into_iter().flatten().collect())
    }
}
