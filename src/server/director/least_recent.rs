//! A small cache that lets go of what was read longest ago: the night
//! skies and the visible-sky grids are each kept in one.

use std::collections::HashMap;
use std::hash::Hash;

pub(super) struct LeastRecent<K, V> {
    /// Each value with the count of reads when it was last read.
    kept: HashMap<K, (V, u64)>,
    /// Counts reads, to find the least recently read.
    reads: u64,
}

impl<K, V> Default for LeastRecent<K, V> {
    fn default() -> Self {
        Self {
            kept: HashMap::new(),
            reads: 0,
        }
    }
}

impl<K: Eq + Hash + Clone, V: Clone> LeastRecent<K, V> {
    /// The value kept under `key`, read now.
    pub(super) fn get(&mut self, key: &K) -> Option<V> {
        self.reads += 1;
        let reads = self.reads;
        self.kept.get_mut(key).map(|(value, read)| {
            *read = reads;
            value.clone()
        })
    }

    /// Keep `value` under `key` unless one is kept there already, then
    /// let go of the least recently read until at most `limit` are kept.
    pub(super) fn keep(&mut self, key: K, value: V, limit: usize) {
        self.reads += 1;
        let reads = self.reads;
        self.kept.entry(key).or_insert((value, reads));
        while self.kept.len() > limit {
            let Some(oldest) = self
                .kept
                .iter()
                .min_by_key(|(_, (_, read))| *read)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.kept.remove(&oldest);
        }
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.kept.len()
    }

    #[cfg(test)]
    pub(super) fn contains(&self, key: &K) -> bool {
        self.kept.contains_key(key)
    }

    #[cfg(test)]
    pub(super) fn clear(&mut self) {
        self.kept.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_least_recently_read_goes_first_and_a_kept_value_stays() {
        let mut cache = LeastRecent::default();
        cache.keep("a", 1, 2);
        cache.keep("b", 2, 2);
        // Kept already: the first value stays.
        cache.keep("a", 9, 2);
        assert_eq!(cache.get(&"a"), Some(1));
        // `a` was read after `b`, so `b` goes.
        cache.keep("c", 3, 2);
        assert_eq!(cache.len(), 2);
        assert!(cache.contains(&"a") && cache.contains(&"c"));
        assert_eq!(cache.get(&"b"), None);
        cache.clear();
        assert_eq!(cache.len(), 0);
    }
}
