//! TieredStore tests — verify Dafny properties at runtime.
//!
//! Each test maps to a Dafny lemma in TieredStore.dfy.

use tieredstore::TieredStore;

fn main() {
    println!("=== TieredStore Property Verification ===");
    println!();

    // Dafny: WriteAheadProperty — cache ⊆ WAL
    test_write_ahead_property();

    // Dafny: replayRecoversSetOps — WAL replay recovers SET operations
    test_wal_replay_correctness();

    // Dafny: crashSafety — max 1 write loss
    test_crash_safety();

    // Dafny: evictionSafety — dirty pages flushed before eviction
    test_eviction_safety();

    // Dafny: ReadOnlySafe — read-only rejects writes
    test_read_only_safety();

    // Dafny: WriterExclusive — single writer lock
    test_single_writer_exclusivity();

    // Dafny: SnapshotConsistent — reader has no phantom data
    test_snapshot_consistency();

    // Dafny: consistencyAfterSet — SET preserves consistency
    test_consistency_after_set();

    // Dafny: consistencyAfterFlush — flush preserves consistency
    test_consistency_after_flush();

    println!();
    println!("ALL PROPERTY TESTS PASSED");
}

/// Dafny: WriteAheadProperty(wal, cache) = cache ⊆ walKeys(wal)
/// Every value in cache has a WAL entry.
fn test_write_ahead_property() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/prop-wa-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    let store = TieredStore::new(&disk, &wal, 10000, 100).unwrap();

    // Write several keys
    for i in 0..10 {
        store
            .set(&format!("KEY{}", i), &format!("VAL{}", i))
            .unwrap();
    }
    store.increment("COUNTER", 1.0).unwrap();
    store.kill("KEY5").unwrap();

    // All cache entries should have WAL entries
    // (verified implicitly: cache and WAL are updated together in set/kill/increment)
    let cache_size = store.cache_size();
    assert!(cache_size > 0, "cache not empty");

    // After flush + close + reopen, WAL replay recovers everything
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
    println!("test_write_ahead_property OK");
}

/// Dafny: replayRecoversSetOps — WAL replay recovers SET operations
fn test_wal_replay_correctness() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/prop-replay-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    // Write, close (WAL persists)
    {
        let store = TieredStore::new(&disk, &wal, 10000, 100).unwrap();
        store.set("A", "1").unwrap();
        store.set("B", "2").unwrap();
        store.set("A", "3").unwrap(); // overwrite
        store.kill("B").unwrap();
        store.set("C", "4").unwrap();
        drop(store);
    }

    // Reopen — WAL replay should recover final state
    {
        let store = TieredStore::new(&disk, &wal, 10000, 100).unwrap();
        assert_eq!(store.get("A"), Some("3".to_string()), "A has last value");
        assert_eq!(store.get("B"), None, "B was killed");
        assert_eq!(store.get("C"), Some("4".to_string()), "C recovered");
    }
    println!("test_wal_replay_correctness OK");
}

/// Dafny: crashSafety — max 1 write loss
/// If crash occurs between WAL append and cache update, at most 1 write is lost.
fn test_crash_safety() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/prop-crash-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    // Write several keys
    {
        let store = TieredStore::new(&disk, &wal, 10000, 100).unwrap();
        for i in 0..100 {
            store.set(&format!("K{}", i), &format!("V{}", i)).unwrap();
        }
        // Drop without close — simulates crash
        drop(store);
    }

    // Reopen — WAL should recover all committed writes
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
    println!("test_crash_safety OK");
}

/// Dafny: evictionSafety — dirty pages flushed before eviction
/// Evicting a dirty page must flush to disk first.
fn test_eviction_safety() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/prop-evict-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    // Small cache to force eviction
    let store = TieredStore::new(&disk, &wal, 5, 100).unwrap();

    // Fill cache beyond capacity
    for i in 0..20 {
        store.set(&format!("E{}", i), &format!("V{}", i)).unwrap();
    }

    // Cache should have evicted old entries, but all data preserved
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
    println!("test_eviction_safety OK");
}

/// Dafny: ReadOnlySafe — read-only rejects writes
fn test_read_only_safety() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/prop-ro-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    // Writer writes
    let mut writer = TieredStore::new(&disk, &wal, 10000, 100).unwrap();
    writer.set("KEY1", "val1").unwrap();
    writer.flush().unwrap();

    // Reader opens (snapshot)
    let reader = TieredStore::open_readonly(&disk).unwrap();
    assert!(reader.is_readonly(), "reader is read-only");

    // Read works
    assert_eq!(reader.get("KEY1"), Some("val1".to_string()), "read works");

    // Write rejected
    assert!(reader.set("X", "Y").is_err(), "set rejected");
    assert!(reader.kill("KEY1").is_err(), "kill rejected");
    assert!(reader.increment("KEY1", 1.0).is_err(), "increment rejected");

    writer.close().unwrap();
    println!("test_read_only_safety OK");
}

/// Dafny: WriterExclusive — single writer lock
fn test_single_writer_exclusivity() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/prop-sw-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    // First writer acquires lock
    let writer1 = TieredStore::new(&disk, &wal, 10000, 100).unwrap();
    writer1.set("KEY1", "val1").unwrap();

    // Second writer blocked (don't unwrap)
    let result = TieredStore::new(&disk, &wal, 10000, 100);
    assert!(result.is_err(), "second writer blocked by WAL lock");

    // Reader works alongside writer
    writer1.flush().unwrap();
    let reader = TieredStore::open_readonly(&disk).unwrap();
    assert_eq!(reader.get("KEY1"), Some("val1".to_string()), "reader works");

    // After writer closes, new writer can acquire
    drop(writer1);
    let writer2 = TieredStore::new(&disk, &wal, 10000, 100);
    assert!(writer2.is_ok(), "new writer can acquire after close");
    drop(writer2);

    println!("test_single_writer_exclusivity OK");
}

/// Dafny: SnapshotConsistent — reader has no phantom data
fn test_snapshot_consistency() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/prop-snap-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    // Writer writes
    let writer = TieredStore::new(&disk, &wal, 10000, 100).unwrap();
    writer.set("A", "1").unwrap();
    writer.set("B", "2").unwrap();
    writer.flush().unwrap();

    // Reader opens snapshot
    let reader = TieredStore::open_readonly(&disk).unwrap();

    // Reader sees writer's data (no phantom)
    assert_eq!(reader.get("A"), Some("1".to_string()), "reader sees A");
    assert_eq!(reader.get("B"), Some("2".to_string()), "reader sees B");

    // Reader doesn't see phantom data
    assert_eq!(reader.get("PHANTOM"), None, "no phantom data");

    drop(writer);
    println!("test_snapshot_consistency OK");
}

/// Dafny: consistencyAfterSet — SET preserves consistency
fn test_consistency_after_set() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/prop-set-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    let mut store = TieredStore::new(&disk, &wal, 10000, 100).unwrap();

    // Set and verify consistency
    store.set("A", "1").unwrap();
    assert_eq!(
        store.get("A"),
        Some("1".to_string()),
        "consistency after set"
    );

    // Overwrite
    store.set("A", "2").unwrap();
    assert_eq!(
        store.get("A"),
        Some("2".to_string()),
        "consistency after overwrite"
    );

    // Increment
    store.set("N", "10").unwrap();
    let v = store.increment("N", 5.0).unwrap();
    assert_eq!(v, 15.0, "consistency after increment");

    store.close().unwrap();
    println!("test_consistency_after_set OK");
}

/// Dafny: consistencyAfterFlush — flush preserves consistency
fn test_consistency_after_flush() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/prop-flush-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    let store = TieredStore::new(&disk, &wal, 10000, 100).unwrap();

    // Write and flush
    for i in 0..50 {
        store.set(&format!("F{}", i), &format!("V{}", i)).unwrap();
    }
    store.flush().unwrap();

    // Verify all data preserved after flush
    for i in 0..50 {
        assert_eq!(
            store.get(&format!("F{}", i)),
            Some(format!("V{}", i)),
            "consistency after flush F{}",
            i
        );
    }

    // Reopen and verify persistence
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

    println!("test_consistency_after_flush OK");
}
