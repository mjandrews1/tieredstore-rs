//! TieredStore property verification — runtime checks of the Dafny lemmas
//! in TieredStore.dfy.
//!
//! Every function here maps to a Dafny lemma (or lemma family) and runs both
//! ways: as a `#[test]` under `cargo test`, and via the `src/main.rs` driver
//! (`cargo run --release`, which CI also runs). This module lives in the
//! library so `cargo test` sees it — a property suite invisible to the
//! standard test command is a discoverability bug.

use crate::store::TieredStore;
use crate::wal::{Op, WriteAheadLog};

/// Dafny: WriteAheadProperty(wal, cache) = cache ⊆ walKeys(wal)
/// Every value in cache has a WAL entry.
pub fn test_write_ahead_property() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/prop-wa-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    let store = TieredStore::new(&disk, &wal, 10000, 100).unwrap();

    for i in 0..10 {
        store
            .set(&format!("KEY{}", i), &format!("VAL{}", i))
            .unwrap();
    }
    store.increment("COUNTER", 1.0).unwrap();
    store.kill("KEY5").unwrap();

    let cache_size = store.cache_size();
    assert!(cache_size > 0, "cache not empty");

    drop(store);
    let store2 = TieredStore::new(&disk, &wal, 10000, 100).unwrap();
    for i in 0..10 {
        let expected = format!("VAL{}", i);
        if i != 5 {
            // KEY5 was killed
            assert_eq!(
                store2.get(&format!("KEY{}", i)),
                Some(expected),
                "WAL recovered KEY{}",
                i
            );
        }
    }
}

/// Dafny: replayRecoversSetOps — WAL replay recovers SET operations
pub fn test_wal_replay_correctness() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/prop-replay-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    {
        let store = TieredStore::new(&disk, &wal, 10000, 100).unwrap();
        store.set("A", "1").unwrap();
        store.set("B", "2").unwrap();
        store.set("A", "3").unwrap(); // overwrite
        store.kill("B").unwrap();
        store.set("C", "4").unwrap();
        drop(store);
    }

    {
        let store = TieredStore::new(&disk, &wal, 10000, 100).unwrap();
        assert_eq!(store.get("A"), Some("3".to_string()), "A has last value");
        assert_eq!(store.get("B"), None, "B was killed");
        assert_eq!(store.get("C"), Some("4".to_string()), "C recovered");
    }
}

/// Dafny: walKeysAppend — append grows the logged key set monotonically
///
/// After appending ops, the replayed key set contains every appended key;
/// appending never shrinks the logged keys (kills remove from the STORE
/// view, not from the log).
pub fn test_wal_keys_append() {
    let tmp = std::env::temp_dir();
    let wal = format!("{}/prop-walkeys-{}.wal", tmp.display(), std::process::id());

    let mut log = WriteAheadLog::new(&wal, 100).unwrap();
    let seq1 = log.append_set("K1", "V1").unwrap();
    let seq2 = log.append_set("K2", "V2").unwrap();
    log.append_kill("K1").unwrap();
    let seq4 = log.append_set("K1", "V1b").unwrap();
    assert!(seq2 > seq1, "sequence numbers increase");
    assert!(seq4 > seq2, "sequence numbers increase");
    log.fsync().unwrap();

    let entries = log.replay().unwrap();
    assert_eq!(entries.len(), 4, "all four entries replay");
    let keys: Vec<&str> = entries
        .iter()
        .map(|e| match &e.op {
            Op::Set { key, .. } => key.as_str(),
            Op::Kill { key } => key.as_str(),
        })
        .collect();
    // walKeys(wal + op) grows: every appended key appears in the log
    assert!(keys.contains(&"K1"), "K1 in walKeys");
    assert!(keys.contains(&"K2"), "K2 in walKeys");
    // Dafny lastValueFor: the last SET for a key wins
    let last_k1_set = entries
        .iter()
        .rev()
        .find_map(|e| match &e.op {
            Op::Set { key, value } if key == "K1" => Some(value.clone()),
            _ => None,
        })
        .expect("K1 has a SET entry");
    assert_eq!(last_k1_set, "V1b", "last value for K1 wins");
}

/// Dafny: crashSafety — max 1 write loss
pub fn test_crash_safety() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/prop-crash-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    {
        let store = TieredStore::new(&disk, &wal, 10000, 100).unwrap();
        for i in 0..100 {
            store.set(&format!("K{}", i), &format!("V{}", i)).unwrap();
        }
        // Drop without close — simulates crash
        drop(store);
    }

    {
        let store = TieredStore::new(&disk, &wal, 10000, 100).unwrap();
        let mut recovered = 0;
        for i in 0..100 {
            if store.get(&format!("K{}", i)).is_some() {
                recovered += 1;
            }
        }
        // At most 1 write loss allowed (the one being WAL'd at crash)
        assert!(
            recovered >= 99,
            "at most 1 write loss: {}/100 recovered",
            recovered
        );
    }
}

/// Dafny: evictionSafety — dirty pages flushed before eviction
pub fn test_eviction_safety() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/prop-evict-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    let store = TieredStore::new(&disk, &wal, 5, 100).unwrap();

    for i in 0..20 {
        store.set(&format!("E{}", i), &format!("V{}", i)).unwrap();
    }

    store.flush().unwrap();
    drop(store);

    let store2 = TieredStore::new(&disk, &wal, 5, 100).unwrap();
    for i in 0..20 {
        assert_eq!(
            store2.get(&format!("E{}", i)),
            Some(format!("V{}", i)),
            "evicted key E{} preserved",
            i
        );
    }
}

/// Dafny: readOnlyRejectsSet/Kill/Increment + readOnlyAllowsGet
pub fn test_read_only_safety() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/prop-ro-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    let mut writer = TieredStore::new(&disk, &wal, 10000, 100).unwrap();
    writer.set("KEY1", "val1").unwrap();
    writer.flush().unwrap();

    let reader = TieredStore::open_readonly(&disk).unwrap();
    assert!(reader.is_readonly(), "reader is read-only");

    assert_eq!(reader.get("KEY1"), Some("val1".to_string()), "read works");
    assert!(reader.set("X", "Y").is_err(), "set rejected");
    assert!(reader.kill("KEY1").is_err(), "kill rejected");
    assert!(reader.increment("KEY1", 1.0).is_err(), "increment rejected");

    writer.close().unwrap();
}

/// Dafny: WriterExclusive + writerRelease + concurrentWritersAtMostOne
pub fn test_single_writer_exclusivity() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/prop-sw-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    let writer1 = TieredStore::new(&disk, &wal, 10000, 100).unwrap();
    writer1.set("KEY1", "val1").unwrap();

    // Second writer blocked (don't unwrap)
    let result = TieredStore::new(&disk, &wal, 10000, 100);
    assert!(result.is_err(), "second writer blocked by WAL lock");

    // Reader works alongside writer (writerReaderCoexist)
    writer1.flush().unwrap();
    let reader = TieredStore::open_readonly(&disk).unwrap();
    assert_eq!(reader.get("KEY1"), Some("val1".to_string()), "reader works");

    // After writer closes, new writer can acquire (writerRelease)
    drop(writer1);
    let writer2 = TieredStore::new(&disk, &wal, 10000, 100);
    assert!(writer2.is_ok(), "new writer can acquire after close");
    drop(writer2);
}

/// Dafny: snapshotIsConsistent — reader has no phantom data
pub fn test_snapshot_consistency() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/prop-snap-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    let writer = TieredStore::new(&disk, &wal, 10000, 100).unwrap();
    writer.set("A", "1").unwrap();
    writer.set("B", "2").unwrap();
    writer.flush().unwrap();

    let reader = TieredStore::open_readonly(&disk).unwrap();

    assert_eq!(reader.get("A"), Some("1".to_string()), "reader sees A");
    assert_eq!(reader.get("B"), Some("2".to_string()), "reader sees B");
    assert_eq!(reader.get("PHANTOM"), None, "no phantom data");

    drop(writer);
}

/// Dafny: consistencyAfterSet — SET preserves consistency
pub fn test_consistency_after_set() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/prop-set-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    let mut store = TieredStore::new(&disk, &wal, 10000, 100).unwrap();

    store.set("A", "1").unwrap();
    assert_eq!(
        store.get("A"),
        Some("1".to_string()),
        "consistency after set"
    );

    store.set("A", "2").unwrap();
    assert_eq!(
        store.get("A"),
        Some("2".to_string()),
        "consistency after overwrite"
    );

    store.set("N", "10").unwrap();
    let v = store.increment("N", 5.0).unwrap();
    assert_eq!(v, 15.0, "consistency after increment");

    store.close().unwrap();
}

/// Dafny: consistencyAfterFlush — flush preserves consistency
pub fn test_consistency_after_flush() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/prop-flush-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    let store = TieredStore::new(&disk, &wal, 10000, 100).unwrap();

    for i in 0..50 {
        store.set(&format!("F{}", i), &format!("V{}", i)).unwrap();
    }
    store.flush().unwrap();

    for i in 0..50 {
        assert_eq!(
            store.get(&format!("F{}", i)),
            Some(format!("V{}", i)),
            "consistency after flush F{}",
            i
        );
    }

    drop(store);
    let store2 = TieredStore::new(&disk, &wal, 10000, 100).unwrap();
    for i in 0..50 {
        assert_eq!(
            store2.get(&format!("F{}", i)),
            Some(format!("V{}", i)),
            "persistence after flush F{}",
            i
        );
    }
}

/// Dafny: appendKillPreservesWriteAhead + replay kill semantics —
/// "KILL operations remove keys". Regression for the bug the randomized
/// test found: a KILL of a key already flushed to disk (evicted from cache)
/// used to resurrect the value on read/reopen.
pub fn test_kill_of_evicted_key() {
    let tmp = std::env::temp_dir();
    let disk = format!(
        "{}/prop-kill-evict-{}.db",
        tmp.display(),
        std::process::id()
    );
    let wal = format!("{}.wal", disk);

    // Cache size 1 over 3 keys: K1 is evicted (flushed to disk) by K2.
    let store = TieredStore::new(&disk, &wal, 1, 100).unwrap();
    store.set("K1", "V1").unwrap();
    store.set("K2", "V2").unwrap(); // evicts K1 to disk
    store.kill("K1").unwrap();

    // Within the session: no resurrection from disk
    assert_eq!(
        store.get("K1"),
        None,
        "killed evicted key stays dead (live)"
    );

    drop(store);
    let store2 = TieredStore::new(&disk, &wal, 1, 100).unwrap();
    assert_eq!(
        store2.get("K1"),
        None,
        "killed evicted key stays dead (reopen)"
    );
    assert_eq!(
        store2.get("K2"),
        Some("V2".to_string()),
        "other keys unaffected"
    );
}

/// All property tests, in driver order. The `src/main.rs` driver iterates
/// this so `cargo run --release` and `cargo test` execute the same checks.
pub const ALL: &[(&str, fn())] = &[
    ("test_write_ahead_property", test_write_ahead_property),
    ("test_wal_replay_correctness", test_wal_replay_correctness),
    ("test_wal_keys_append", test_wal_keys_append),
    ("test_kill_of_evicted_key", test_kill_of_evicted_key),
    ("test_crash_safety", test_crash_safety),
    ("test_eviction_safety", test_eviction_safety),
    ("test_read_only_safety", test_read_only_safety),
    (
        "test_single_writer_exclusivity",
        test_single_writer_exclusivity,
    ),
    ("test_snapshot_consistency", test_snapshot_consistency),
    ("test_consistency_after_set", test_consistency_after_set),
    ("test_consistency_after_flush", test_consistency_after_flush),
];

#[cfg(test)]
mod tests {
    #[test]
    fn write_ahead_property() {
        super::test_write_ahead_property();
    }

    #[test]
    fn wal_replay_correctness() {
        super::test_wal_replay_correctness();
    }

    #[test]
    fn wal_keys_append() {
        super::test_wal_keys_append();
    }

    #[test]
    fn kill_of_evicted_key() {
        super::test_kill_of_evicted_key();
    }

    #[test]
    fn crash_safety() {
        super::test_crash_safety();
    }

    #[test]
    fn eviction_safety() {
        super::test_eviction_safety();
    }

    #[test]
    fn read_only_safety() {
        super::test_read_only_safety();
    }

    #[test]
    fn single_writer_exclusivity() {
        super::test_single_writer_exclusivity();
    }

    #[test]
    fn snapshot_consistency() {
        super::test_snapshot_consistency();
    }

    #[test]
    fn consistency_after_set() {
        super::test_consistency_after_set();
    }

    #[test]
    fn consistency_after_flush() {
        super::test_consistency_after_flush();
    }
}
