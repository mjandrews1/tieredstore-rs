//! LRU cache with access-order tracking.

use std::collections::{HashMap, VecDeque};

pub struct LruCache {
    map: HashMap<String, String>,
    order: VecDeque<String>,
    max_size: usize,
}

impl LruCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            max_size,
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

        // Evict if over limit
        while self.map.len() > self.max_size {
            self.evict_oldest();
        }
    }

    pub fn remove(&mut self, key: &str) {
        self.map.remove(key);
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

    pub fn contains_key(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn keys(&self) -> Vec<&String> {
        self.map.keys().collect()
    }

    fn touch(&mut self, key: &str) {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            let k = self.order.remove(pos).unwrap();
            self.order.push_back(k);
        }
    }

    fn evict_oldest(&mut self) {
        if let Some(key) = self.order.pop_front() {
            self.map.remove(&key);
        }
    }
}
