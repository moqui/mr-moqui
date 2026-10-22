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

//! The prepared-statement LRU the MySQL and Postgres drivers share, keyed by the SQL text.
//!
//! Each use stamps the entry with a new tick and queues `(sql, tick)`; an older queue item for the same SQL is stale
//! and skipped when it reaches the front. A use is O(1), and the queue is compacted once it is twice the cap, so an
//! eviction is amortized O(1) too. The tick is a `Cell`, so a use hashes the SQL text once.

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

pub(crate) struct StatementLru<S> {
    cap: usize,
    tick: u64,
    entries: HashMap<Arc<str>, (S, Cell<u64>)>,
    order: VecDeque<(Arc<str>, u64)>,
}

impl<S> StatementLru<S> {
    /// An empty cache that holds at most `cap` statements (at least 1).
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            tick: 0,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Only the MySQL driver asks.
    #[cfg_attr(not(feature = "mysql"), allow(dead_code))]
    pub(crate) fn contains(&self, sql: &str) -> bool {
        self.entries.contains_key(sql)
    }

    /// The statement for `sql`, which becomes the most recently used.
    pub(crate) fn get(&mut self, sql: &str) -> Option<&S> {
        let tick = self.next_tick();
        self.compact();
        let (key, (statement, used)) = self.entries.get_key_value(sql)?;
        used.set(tick);
        self.order.push_back((Arc::clone(key), tick));
        Some(statement)
    }

    /// Store `statement` for `sql` as the most recently used. Returns the statement it replaced, or the least recently
    /// used one it evicted, for the caller to close.
    pub(crate) fn insert(&mut self, sql: &str, statement: S) -> Option<S> {
        let tick = self.next_tick();
        let key: Arc<str> = match self.entries.get_key_value(sql) {
            Some((key, _)) => Arc::clone(key),
            None => Arc::from(sql),
        };
        self.order.push_back((Arc::clone(&key), tick));
        if let Some((old, _)) = self.entries.insert(key, (statement, Cell::new(tick))) {
            self.compact();
            return Some(old);
        }
        let evicted = if self.entries.len() > self.cap {
            self.evict()
        } else {
            None
        };
        self.compact();
        evicted
    }

    /// Every statement, in no order.
    #[cfg_attr(not(feature = "mysql"), allow(dead_code))]
    pub(crate) fn into_values(self) -> impl Iterator<Item = S> {
        self.entries.into_values().map(|(statement, _)| statement)
    }

    fn next_tick(&mut self) -> u64 {
        self.tick = self.tick.wrapping_add(1);
        self.tick
    }

    fn evict(&mut self) -> Option<S> {
        while let Some((key, tick)) = self.order.pop_front() {
            if self.entries.get(&key).is_some_and(|(_, current)| current.get() == tick) {
                return self.entries.remove(&key).map(|(statement, _)| statement);
            }
        }
        None
    }

    fn compact(&mut self) {
        if self.order.len() > self.cap.saturating_mul(2) {
            let entries = &self.entries;
            self.order
                .retain(|(key, tick)| entries.get(key).is_some_and(|(_, current)| current.get() == *tick));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StatementLru;

    #[test]
    fn the_least_recently_used_statement_is_evicted() {
        let mut lru = StatementLru::new(2);
        assert!(lru.insert("a", 1).is_none());
        assert!(lru.insert("b", 2).is_none());
        assert_eq!(lru.get("a"), Some(&1));
        assert_eq!(lru.insert("c", 3), Some(2));
        assert!(lru.contains("a") && lru.contains("c") && !lru.contains("b"));
        assert_eq!(lru.len(), 2);
    }

    #[test]
    fn a_second_insert_replaces_and_returns_the_old_statement() {
        let mut lru = StatementLru::new(2);
        lru.insert("a", 1);
        lru.insert("b", 2);
        assert_eq!(lru.insert("a", 10), Some(1));
        assert_eq!(lru.len(), 2);
        assert_eq!(lru.insert("c", 3), Some(2));
        assert_eq!(lru.get("a"), Some(&10));
    }

    #[test]
    fn many_uses_keep_the_queue_bounded_and_the_order_right() {
        let mut lru = StatementLru::new(3);
        lru.insert("a", 1);
        lru.insert("b", 2);
        lru.insert("c", 3);
        for _ in 0..1000 {
            assert!(lru.get("a").is_some());
            assert!(lru.get("c").is_some());
        }
        assert!(lru.order.len() <= 6 + 1);
        assert_eq!(lru.insert("d", 4), Some(2));
        assert_eq!(lru.insert("e", 5), Some(1));
        let mut left: Vec<_> = lru.into_values().collect();
        left.sort_unstable();
        assert_eq!(left, vec![3, 4, 5]);
    }
}
