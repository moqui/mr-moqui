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

//! A map with a size cap for the compiled-code caches. When it is full, the quarter of the
//! entries used longest ago go, so the code in use stays compiled instead of all of it being
//! thrown out at once. The cap is a count of entries and, for a map made [`BoundedMap::with_bytes`],
//! also a total of the sizes given with the entries, such as the length of each compiled source.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::Hash;

pub struct BoundedMap<K, V> {
    entries: HashMap<K, Entry<V>>,
    tick: u64,
    cap: usize,
    bytes: usize,
    max_bytes: usize,
}

struct Entry<V> {
    value: V,
    used: u64,
    bytes: usize,
}

impl<K: Hash + Eq + Clone, V: Clone> BoundedMap<K, V> {
    pub fn new(cap: usize) -> Self {
        Self::with_bytes(cap, usize::MAX)
    }

    /// A map of at most `cap` entries whose sizes, as given to [`Self::insert_sized`], total at most `max_bytes`.
    pub fn with_bytes(cap: usize, max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            tick: 0,
            cap: cap.max(4),
            bytes: 0,
            max_bytes,
        }
    }

    pub fn get<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.tick += 1;
        let tick = self.tick;
        self.entries.get_mut(key).map(|entry| {
            entry.used = tick;
            entry.value.clone()
        })
    }

    pub fn insert(&mut self, key: K, value: V) {
        self.insert_sized(key, value, 0);
    }

    /// Insert an entry of `bytes`, its approximate size. An entry larger than the whole budget is not kept.
    pub fn insert_sized(&mut self, key: K, value: V, bytes: usize) {
        if bytes > self.max_bytes {
            return;
        }
        if let Some(old) = self.entries.remove(&key) {
            self.bytes -= old.bytes;
        }
        while !self.entries.is_empty()
            && (self.entries.len() >= self.cap || self.bytes.saturating_add(bytes) > self.max_bytes)
        {
            self.evict();
        }
        self.tick += 1;
        self.bytes += bytes;
        self.entries.insert(
            key,
            Entry {
                value,
                used: self.tick,
                bytes,
            },
        );
    }

    /// Drop one entry. Only the git attributes cache (moqui-git) drops one.
    pub fn remove<Q>(&mut self, key: &Q)
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        if let Some(old) = self.entries.remove(key) {
            self.bytes -= old.bytes;
        }
    }

    /// Drop every entry.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    fn evict(&mut self) {
        let mut ticks: Vec<u64> = self.entries.values().map(|entry| entry.used).collect();
        ticks.sort_unstable();
        let cutoff = ticks[(ticks.len() / 4).max(1) - 1];
        let mut freed = 0;
        self.entries.retain(|_, entry| {
            let keep = entry.used > cutoff;
            if !keep {
                freed += entry.bytes;
            }
            keep
        });
        self.bytes -= freed;
    }
}

#[cfg(test)]
mod tests {
    use super::BoundedMap;

    #[test]
    fn a_full_map_drops_the_entries_used_longest_ago() {
        let mut map = BoundedMap::new(8);
        for index in 0..8 {
            map.insert(index, index);
        }
        // 0 and 1 are used again, so 2 and 3 are the oldest quarter.
        assert_eq!(map.get(&0), Some(0));
        assert_eq!(map.get(&1), Some(1));
        map.insert(8, 8);
        assert_eq!(map.len(), 7);
        assert_eq!(map.get(&2), None);
        assert_eq!(map.get(&3), None);
        assert_eq!(map.get(&4), Some(4));
        assert_eq!(map.get(&0), Some(0));
        assert_eq!(map.get(&8), Some(8));
    }

    #[test]
    fn a_map_with_a_byte_budget_drops_entries_until_the_new_one_fits() {
        let mut map = BoundedMap::with_bytes(100, 1000);
        for index in 0..4 {
            map.insert_sized(index, index, 300);
        }
        // The fourth did not fit with the first three: the oldest quarter went until it did.
        assert!(map.bytes() <= 1000, "{}", map.bytes());
        assert_eq!(map.get(&0), None);
        assert_eq!(map.get(&3), Some(3));
        map.insert_sized(3, 3, 100);
        assert_eq!(map.get(&3), Some(3));
        assert!(map.bytes() <= 1000);
        map.insert_sized(9, 9, 2000);
        assert_eq!(map.get(&9), None, "larger than the whole budget");
        map.clear();
        assert_eq!((map.len(), map.bytes()), (0, 0));
    }
}
