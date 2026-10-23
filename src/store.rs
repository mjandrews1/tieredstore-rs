//! TieredStore: LRU cache + disk backing + WAL crash recovery.
//!
//! Preserves MStore/H2Store performance with permanence:
//! - Hot keys in LRU cache (fast reads/writes)
//! - Cold keys on disk (persistence)
//! - Write-behind + WAL for crash recovery (max 1 write loss)
//! - Write-aware LRU eviction (prefer clean pages)
//!
//! Single-writer + read-only readers:
//! - Writer: exclusive WAL lock
//! - Reader: snapshot-based (avoids file lock)

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::lru::LruCache;
use crate::wal::{Entry, Op, WriteAheadLog};

pub struct TieredStore {
    cache: Arc<Mutex<LruCache>>,
    dirty: Arc<Mutex<HashSet<String>>>,
    disk: sled::Db,
    wal: Option<Mutex<WriteAheadLog>>,
    read_only: bool,
    ops: AtomicU64,
}

use std::sync::atomic::{AtomicU64, Ordering};

impl TieredStore {
    /// Open TieredStore in write mode (single writer).
    pub fn new(disk_path: &str, wal_path: &str, max_cache_size: usize,
               fsync_interval_ms: u64) -> std::io::Result<Self> {
        let disk = sled::open(disk_path)?;
        let wal = WriteAheadLog::new(wal_path, fsync_interval_ms)?;

        let store = Self {
            cache: Arc::new(Mutex::new(LruCache::new(max_cache_size))),
            dirty: Arc::new(Mutex::new(HashSet::new())),
            disk,
            wal: Some(Mutex::new(wal)),
            read_only: false,
            ops: AtomicU64::new(0),
        };

        // Startup: replay WAL (recovery)
        store.replay_wal()?;

        Ok(store)
    }

    /// Open read-only reader (via snapshot to avoid file lock).
    pub fn open_readonly(disk_path: &str) -> std::io::Result<Self> {
        let snapshot_path = format!("{}.snapshot.{}", disk_path, std::process::id());
        // sled creates a directory — copy recursively
        if Path::new(disk_path).is_dir() {
            copy_dir_recursive(Path::new(disk_path), Path::new(&snapshot_path))?;
        } else {
            std::fs::copy(disk_path, &snapshot_path)?;
        }
        let disk = sled::open(&snapshot_path)?;

        Ok(Self {
            cache: Arc::new(Mutex::new(LruCache::new(10000))),
            dirty: Arc::new(Mutex::new(HashSet::new())),
            disk,
            wal: None,
            read_only: true,
            ops: AtomicU64::new(0),
        })
    }

    /// Get value by key.
    pub fn get(&self, key: &str) -> Option<String> {
        self.ops.fetch_add(1, Ordering::Relaxed);
        // Check cache first
        let cached = {
            let mut cache = self.cache.lock().unwrap();
            cache.get(key)
        };
        if let Some(v) = cached {
            return Some(v);
        }
        // Cache miss — read from disk
        let disk_val = self.disk.get(key).ok().flatten();
        match disk_val {
            Some(v) => {
                let s = String::from_utf8_lossy(&v).to_string();
                let mut cache = self.cache.lock().unwrap();
                cache.insert(key.to_string(), s.clone());
                Some(s)
            }
            None => None,
        }
    }

    /// Set value. Fails in read-only mode.
    pub fn set(&self, key: &str, value: &str) -> std::io::Result<()> {
        if self.read_only {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied,
                "TieredStore is read-only"));
        }
        self.ops.fetch_add(1, Ordering::Relaxed);
        // Write to cache
        {
            let mut cache = self.cache.lock().unwrap();
            cache.insert(key.to_string(), value.to_string());
        }
        {
            let mut dirty = self.dirty.lock().unwrap();
            dirty.insert(key.to_string());
        }
        // Append to WAL
        if let Some(ref wal) = self.wal {
            let mut wal = wal.lock().unwrap();
            wal.append_set(key, value)?;
        }
        Ok(())
    }

    /// Kill key and descendants. Fails in read-only mode.
    pub fn kill(&self, key: &str) -> std::io::Result<()> {
        if self.read_only {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied,
                "TieredStore is read-only"));
        }
        self.ops.fetch_add(1, Ordering::Relaxed);
        // Remove from cache
        {
            let mut cache = self.cache.lock().unwrap();
            cache.remove(key);
            cache.remove_prefix(&format!("{}{}", key, '\x01'));
        }
        {
            let mut dirty = self.dirty.lock().unwrap();
            dirty.remove(key);
            let pfx = format!("{}{}", key, '\x01');
            dirty.retain(|k| !k.starts_with(&pfx));
        }
        // Append to WAL
        if let Some(ref wal) = self.wal {
            let mut wal = wal.lock().unwrap();
            wal.append_kill(key)?;
        }
        Ok(())
    }

    /// Atomic increment. Fails in read-only mode.
    pub fn increment(&self, key: &str, amount: f64) -> std::io::Result<f64> {
        if self.read_only {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied,
                "TieredStore is read-only"));
        }
        self.ops.fetch_add(1, Ordering::Relaxed);
        // Read current value
        let current = self.get(key).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        let next = current + amount;
        let next_str = format!("{}", next as i64);
        self.set(key, &next_str)?;
        Ok(next)
    }

    /// Flush all dirty pages to disk. No-op in read-only.
    pub fn flush(&self) -> std::io::Result<()> {
        if self.read_only {
            return Ok(());
        }
        let dirty_keys: Vec<String> = {
            let dirty = self.dirty.lock().unwrap();
            dirty.iter().cloned().collect()
        };
        // Collect values from cache
        let kv_pairs: Vec<(String, String)> = {
            let mut cache = self.cache.lock().unwrap();
            dirty_keys.iter()
                .filter_map(|k| cache.get(k).map(|v| (k.clone(), v)))
                .collect()
        };
        // Write to disk
        for (k, v) in &kv_pairs {
            self.disk.insert(k.as_bytes(), v.as_bytes())?;
        }
        self.disk.flush()?;
        {
            let mut dirty = self.dirty.lock().unwrap();
            dirty.clear();
        }
        if let Some(ref wal) = self.wal {
            let mut wal = wal.lock().unwrap();
            wal.truncate()?;
        }
        Ok(())
    }

    /// Close: flush + release lock.
    pub fn close(&mut self) -> std::io::Result<()> {
        if self.read_only {
            return Ok(());
        }
        self.flush()?;
        if let Some(ref mut wal) = self.wal {
            let mut wal = wal.lock().unwrap();
            wal.fsync()?;
        }
        Ok(())
    }

    /// True if read-only mode.
    pub fn is_readonly(&self) -> bool {
        self.read_only
    }

    /// Cache size.
    pub fn cache_size(&self) -> usize {
        self.cache.lock().unwrap().len()
    }

    /// Dirty count.
    pub fn dirty_count(&self) -> usize {
        self.dirty.lock().unwrap().len()
    }

    fn replay_wal(&self) -> std::io::Result<()> {
        if let Some(ref wal) = self.wal {
            let wal = wal.lock().unwrap();
            let entries = wal.replay()?;
            let mut cache = self.cache.lock().unwrap();
            let mut dirty = self.dirty.lock().unwrap();
            for entry in entries {
                match entry.op {
                    Op::Set { key, value } => {
                        cache.insert(key.clone(), value);
                        dirty.insert(key);
                    }
                    Op::Kill { key } => {
                        cache.remove(&key);
                        dirty.remove(&key);
                        let pfx = format!("{}{}", key, '\x01');
                        cache.remove_prefix(&pfx);
                    }
                }
            }
        }
        Ok(())
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            copy_dir_recursive(&path, &dst.join(entry.file_name()))?;
        } else {
            std::fs::copy(&path, &dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}
