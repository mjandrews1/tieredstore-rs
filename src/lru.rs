//! LRU cache with access-order tracking and dirty-page flush on eviction.

use std::collections::{HashMap, HashSet, VecDeque};

pub struct LruCache {
    map: HashMap<String, String>,
    order: VecDeque<String>,
    max_size: usize,
    disk: sled::Db,
    dirty: HashSet<String>,
}

impl LruCache {
    pub fn new(max_size: usize, disk: sled::Db) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            max_size,
            disk,
            dirty: HashSet::new(),
        }
    }

    pub fn get(&mut self, key: &str) -> Option<String> {
        let val = self.map.get(key).cloned();
        if val.is_some() {
            self.touch(key);
        }
        val
    }

    pub fn insert(&mut self, key: String, value: String) {
        if self.map.contains_key(&key) {
            self.touch(&key);
        } else {
            self.order.push_back(key.clone());
        }
        self.map.insert(key, value);

        // Evict if over limit — flush dirty pages first (evictionSafety)
        while self.map.len() > self.max_size {
            self.evict_oldest_with_flush();
        }
    }

    pub fn mark_dirty(&mut self, key: &str) {
        self.dirty.insert(key.to_string());
    }

    pub fn clear_dirty(&mut self) {
        self.dirty.clear();
    }

    pub fn remove(&mut self, key: &str) {
        self.map.remove(key);
        self.dirty.remove(key);
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
    }

    pub fn remove_prefix(&mut self, prefix: &str) {
        let keys_to_remove: Vec<String> = self.map.keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        for k in keys_to_remove {
            self.remove(&k);
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    fn touch(&mut self, key: &str) {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            let k = self.order.remove(pos).unwrap();
            self.order.push_back(k);
        }
    }

    /// Evict oldest key. Flush dirty pages before eviction.
    fn evict_oldest_with_flush(&mut self) {
        let key = match self.order.pop_front() {
            Some(k) => k,
            None => return,
        };
        // If dirty, flush to disk before evicting (evictionSafety)
        if self.dirty.contains(&key) {
            if let Some(val) = self.map.get(&key) {
                let _ = self.disk.insert(key.as_bytes(), val.as_bytes());
            }
            self.dirty.remove(&key);
        }
        self.map.remove(&key);
    }
}
