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

//! Stack of maps for variable scope. No script engine.
//!
//! One frame is always present. Popping that frame is an error. Get walks from
//! the top down. Put writes the top map, creating it on the first write.
//! The shared map is not part of get. When `include_context` is set, the key
//! `context` is reserved and get reports [`Lookup::Context`].

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use indexmap::IndexMap;

use crate::error::Error;
use crate::object::{java_equals, Value};

pub type Frame = Rc<RefCell<IndexMap<String, Value>>>;

/// What [`ContextStack::get`] found.
#[derive(Clone, Debug, PartialEq)]
pub enum Lookup {
    Missing,
    Value(Value),
    Context,
}

struct Saved {
    frames: Vec<Option<Frame>>,
    index: usize,
}

/// A context stack. It is not `Sync`: one execution context owns it.
pub struct ContextStack {
    frames: Vec<Option<Frame>>,
    index: usize,
    shared: Frame,
    saved: Vec<Saved>,
    include_context: bool,
}

impl ContextStack {
    pub fn new() -> Self {
        Self::with_include_context(true)
    }

    pub fn with_include_context(include_context: bool) -> Self {
        Self {
            frames: vec![None],
            index: 0,
            shared: Rc::new(RefCell::new(IndexMap::new())),
            saved: Vec::new(),
            include_context,
        }
    }

    /// The entries of the top frame only, the map the last [`Self::push`] began.
    pub fn top_entries(&self) -> IndexMap<String, Value> {
        self.frames
            .last()
            .and_then(Option::as_ref)
            .map(|frame| frame.borrow().clone())
            .unwrap_or_default()
    }

    /// The shared map. Lookups do not read it.
    pub fn shared_map(&self) -> Frame {
        Rc::clone(&self.shared)
    }

    pub fn push(&mut self) {
        self.index += 1;
        self.frames.push(None);
    }

    pub fn push_map(&mut self, map: Frame) -> Result<(), Error> {
        if self.include_context && map.borrow().contains_key("context") {
            return Err(Error::new("cannot push a map containing key 'context'"));
        }
        self.index += 1;
        self.frames.push(Some(map));
        Ok(())
    }

    pub fn push_entries(&mut self, map: IndexMap<String, Value>) -> Result<(), Error> {
        self.push_map(Rc::new(RefCell::new(map)))
    }

    /// Remove the top frame. The last frame cannot be popped.
    pub fn pop(&mut self) -> Result<Option<Frame>, Error> {
        if self.index == 0 {
            return Err(Error::new("ContextStack is empty, cannot pop the context"));
        }
        let frame = self.frames.pop().flatten();
        self.index -= 1;
        Ok(frame)
    }

    /// The saved contexts and the frames pushed on the current one: a mark for [`Self::truncate`].
    pub fn depth(&self) -> (usize, usize) {
        (self.saved.len(), self.index)
    }

    /// Pop the frames pushed on the current context since `depth` was read. Nothing changes when a different context
    /// is current than then.
    pub fn truncate(&mut self, depth: (usize, usize)) {
        let (saved, index) = depth;
        if self.saved.len() != saved {
            return;
        }
        while self.index > index {
            self.frames.pop();
            self.index -= 1;
        }
    }

    /// Save the whole stack and start an isolated empty one.
    pub fn push_context(&mut self) {
        self.saved.push(Saved {
            frames: self.frames.clone(),
            index: self.index,
        });
        self.frames = vec![None];
        self.index = 0;
    }

    pub fn pop_context(&mut self) -> Result<(), Error> {
        let Some(saved) = self.saved.pop() else {
            return Err(Error::new("cannot pop context, no context pushed"));
        };
        self.frames = saved.frames;
        self.index = saved.index;
        Ok(())
    }

    pub fn add_root_map(&mut self, map: Frame) -> Result<(), Error> {
        if self.include_context && map.borrow().contains_key("context") {
            return Err(Error::new("cannot push a map containing key 'context'"));
        }
        self.frames.insert(0, Some(map));
        self.index += 1;
        Ok(())
    }

    pub fn root_map(&self) -> Option<Frame> {
        self.frames.first().and_then(Clone::clone)
    }

    pub fn get(&self, key: &str) -> Lookup {
        for frame in self.frames[..=self.index].iter().rev() {
            let Some(frame) = frame else {
                continue;
            };
            if let Some(value) = frame.borrow().get(key) {
                return Lookup::Value(value.clone());
            }
        }
        if self.include_context && key == "context" {
            Lookup::Context
        } else {
            Lookup::Missing
        }
    }

    /// `read` of the value `key` resolves to, in place without the copy [`Self::get`] makes. `None` when no frame holds
    /// it (`context`, or a missing name). The stack stays borrowed while `read` runs.
    pub fn with_value<R>(&self, key: &str, read: impl FnOnce(&Value) -> R) -> Option<R> {
        for frame in self.frames[..=self.index].iter().rev().flatten() {
            if let Some(found) = frame.borrow().get(key) {
                return Some(read(found));
            }
        }
        None
    }

    /// Whether `key` resolves to a value equal to `value`, compared in place without the copy [`Self::get`] makes.
    pub fn holds(&self, key: &str, value: &Value) -> bool {
        for frame in self.frames[..=self.index].iter().rev().flatten() {
            if let Some(found) = frame.borrow().get(key) {
                return found == value;
            }
        }
        false
    }

    pub fn put(&mut self, key: impl Into<String>, value: Value) -> Result<Option<Value>, Error> {
        let key = key.into();
        if self.include_context && key == "context" {
            return Err(Error::new("cannot put with key 'context'"));
        }
        let frame = self.top_frame();
        let previous = frame.borrow_mut().insert(key, value);
        Ok(previous)
    }

    /// Replace `key` in the nearest frame that holds it, so a callee's change to a caller's
    /// list stays with the caller. Returns false and stores nothing when no frame holds it.
    pub fn put_existing(&mut self, key: &str, value: Value) -> bool {
        for frame in self.frames[..=self.index].iter().rev().flatten() {
            if frame.borrow().contains_key(key) {
                frame.borrow_mut().insert(key.to_string(), value);
                return true;
            }
        }
        false
    }

    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.frames
            .get(self.index)
            .and_then(Option::as_ref)
            .and_then(|frame| frame.borrow_mut().shift_remove(key))
    }

    pub fn put_all(&mut self, map: &IndexMap<String, Value>) -> Result<(), Error> {
        if self.include_context && map.contains_key("context") {
            return Err(Error::new("cannot push a map containing key 'context'"));
        }
        let frame = self.top_frame();
        frame
            .borrow_mut()
            .extend(map.iter().map(|(key, value)| (key.clone(), value.clone())));
        Ok(())
    }

    pub fn clear(&mut self) {
        if let Some(frame) = self.frames.get(self.index).and_then(Option::as_ref) {
            frame.borrow_mut().clear();
        }
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.frames[..=self.index]
            .iter()
            .rev()
            .flatten()
            .any(|frame| frame.borrow().contains_key(key))
    }

    pub fn contains_value(&self, value: &Value) -> bool {
        let mut seen = HashSet::new();
        for frame in self.frames[..=self.index].iter().rev().flatten() {
            for (key, entry) in frame.borrow().iter() {
                if !seen.insert(key.clone()) {
                    continue;
                }
                if java_equals(entry, value) {
                    return true;
                }
            }
        }
        false
    }

    pub fn is_empty(&self) -> bool {
        self.frames[..=self.index]
            .iter()
            .flatten()
            .all(|frame| frame.borrow().is_empty())
    }

    pub fn len(&self) -> usize {
        self.key_set().len()
    }

    pub fn key_set(&self) -> HashSet<String> {
        let mut keys = HashSet::new();
        for frame in self.frames[..=self.index].iter().rev().flatten() {
            for key in frame.borrow().keys() {
                keys.insert(key.clone());
            }
        }
        keys
    }

    /// The visible keys in the order of [`Self::combined_map`], without copying the values.
    pub fn combined_keys(&self) -> Vec<String> {
        let mut keys = IndexMap::new();
        for frame in self.frames[..=self.index].iter().flatten() {
            for key in frame.borrow().keys() {
                keys.insert(key.clone(), ());
            }
        }
        keys.into_keys().collect()
    }

    /// Root first, so a higher frame overrides a lower one.
    pub fn combined_map(&self) -> IndexMap<String, Value> {
        let mut combined = IndexMap::new();
        for frame in self.frames[..=self.index].iter().flatten() {
            for (key, value) in frame.borrow().iter() {
                combined.insert(key.clone(), value.clone());
            }
        }
        combined
    }

    /// Shallow copy: the frame maps are shared, the shared map is copied.
    pub fn clone_stack(&self) -> Self {
        Self {
            frames: self.frames.clone(),
            index: self.index,
            shared: Rc::new(RefCell::new(self.shared.borrow().clone())),
            saved: self
                .saved
                .iter()
                .map(|saved| Saved {
                    frames: saved.frames.clone(),
                    index: saved.index,
                })
                .collect(),
            include_context: self.include_context,
        }
    }

    fn top_frame(&mut self) -> Frame {
        Rc::clone(self.frames[self.index].get_or_insert_with(|| Rc::new(RefCell::new(IndexMap::new()))))
    }
}

impl Default for ContextStack {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_shadows_and_explicit_null_stays_visible() {
        let mut stack = ContextStack::new();
        stack.put("a", Value::string("root")).unwrap();
        stack.push();
        stack.put("a", Value::Null).unwrap();
        assert_eq!(stack.get("a"), Lookup::Value(Value::Null));
        assert!(stack.contains_key("a"));
        stack.pop().unwrap();
        assert_eq!(stack.get("a"), Lookup::Value(Value::string("root")));
        assert!(stack.pop().is_err());
    }

    #[test]
    fn context_key_is_reserved_and_shared_map_is_separate() {
        let mut stack = ContextStack::new();
        assert_eq!(stack.get("context"), Lookup::Context);
        assert!(stack.put("context", Value::string("no")).is_err());
        stack
            .shared_map()
            .borrow_mut()
            .insert("hidden".to_string(), Value::string("x"));
        assert_eq!(stack.get("hidden"), Lookup::Missing);
        let mut bad = IndexMap::new();
        bad.insert("context".to_string(), Value::Null);
        assert!(stack.push_entries(bad).is_err());
    }

    #[test]
    fn pushed_map_is_shared_and_push_context_isolates() {
        let mut stack = ContextStack::new();
        stack.put("a", Value::I32(1)).unwrap();
        let frame = Rc::new(RefCell::new(IndexMap::new()));
        stack.push_map(Rc::clone(&frame)).unwrap();
        stack.put("b", Value::I32(2)).unwrap();
        assert_eq!(frame.borrow().get("b"), Some(&Value::I32(2)));
        assert_eq!(stack.combined_keys(), vec!["a".to_string(), "b".to_string()]);
        stack.push_context();
        assert_eq!(stack.get("a"), Lookup::Missing);
        stack.put("c", Value::I32(3)).unwrap();
        assert_eq!(stack.combined_keys(), vec!["c".to_string()]);
        stack.pop_context().unwrap();
        assert_eq!(stack.get("a"), Lookup::Value(Value::I32(1)));
        assert_eq!(stack.get("b"), Lookup::Value(Value::I32(2)));
        assert_eq!(stack.get("c"), Lookup::Missing);
        assert!(stack.pop_context().is_err());
    }

    #[test]
    fn truncate_pops_back_to_a_depth_of_the_same_context() {
        let mut stack = ContextStack::new();
        stack.put("a", Value::I32(1)).unwrap();
        let mark = stack.depth();
        stack.push();
        stack.push();
        stack.put("a", Value::I32(2)).unwrap();
        stack.push_context();
        stack.push();
        stack.truncate(mark);
        assert_eq!(stack.depth(), (1, 1));
        stack.pop_context().unwrap();
        stack.truncate(mark);
        assert_eq!(stack.depth(), mark);
        assert_eq!(stack.get("a"), Lookup::Value(Value::I32(1)));
        stack.truncate((0, 5));
        assert_eq!(stack.depth(), mark);
    }
}
