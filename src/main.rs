//! TieredStore tests matching the Dafny spec.

use tieredstore::TieredStore;

fn main() {
    println!("=== TieredStore Rust Tests ===");
    println!();

    test_basic_operations();
    test_crash_recovery();
    test_readonly_mode();
    test_single_writer();

    println!();
    println!("ALL TESTS PASSED");
}

fn test_basic_operations() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/test-basic-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    let mut store = TieredStore::new(&disk, &wal, 10000, 100).unwrap();

    // SET / GET
    store.set("KEY1", "value1").unwrap();
    assert_eq!(store.get("KEY1"), Some("value1".to_string()), "get string");
    store.set("KEY2", "42").unwrap();
    assert_eq!(store.get("KEY2"), Some("42".to_string()), "get number");

    // INCREMENT
    let n = store.increment("KEY3", 5.0).unwrap();
    assert_eq!(n, 5.0, "increment 1");
    let n = store.increment("KEY3", 3.0).unwrap();
    assert_eq!(n, 8.0, "increment 2");

    // KILL
    store.set("SUB1", "a").unwrap();
    store.set("SUB1\x01X", "b").unwrap();
    store.kill("SUB1").unwrap();
    assert_eq!(store.get("SUB1"), None, "kill node");
    assert_eq!(store.get("SUB1\x01X"), None, "kill descendant");

    assert!(store.cache_size() > 0, "cache populated");
    store.close().unwrap();
    println!("test_basic_operations OK");
}

fn test_crash_recovery() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/test-crash-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    // Write and close
    {
        let mut store = TieredStore::new(&disk, &wal, 10000, 100).unwrap();
        store.set("KEY1", "value1").unwrap();
        store.set("KEY2", "42").unwrap();
        store.increment("KEY3", 5.0).unwrap();
        store.close().unwrap();
    }

    // Reopen — WAL replay recovers writes
    {
        let store = TieredStore::new(&disk, &wal, 10000, 100).unwrap();
        assert_eq!(store.get("KEY1"), Some("value1".to_string()), "recovery KEY1");
        assert_eq!(store.get("KEY2"), Some("42".to_string()), "recovery KEY2");
        assert_eq!(store.get("KEY3").map(|v| v.parse::<f64>().unwrap()), Some(5.0), "recovery KEY3");
    }

    println!("test_crash_recovery OK");
}

fn test_readonly_mode() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/test-ro-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    // Writer writes
    let mut writer = TieredStore::new(&disk, &wal, 10000, 100).unwrap();
    writer.set("KEY1", "value1").unwrap();
    writer.set("KEY2", "42").unwrap();

    // Reader opens (snapshot, no lock conflict)
    // Flush writer to disk before opening reader
    writer.flush().unwrap();
    let reader = TieredStore::open_readonly(&disk).unwrap();
    assert!(reader.is_readonly(), "reader is read-only");
    assert_eq!(reader.get("KEY1"), Some("value1".to_string()), "reader sees KEY1");
    assert_eq!(reader.get("KEY2"), Some("42".to_string()), "reader sees KEY2");

    // Reader rejects writes
    assert!(reader.set("KEY3", "nope").is_err(), "reader rejects write");
    assert!(reader.kill("KEY1").is_err(), "reader rejects kill");
    assert!(reader.increment("KEY1", 1.0).is_err(), "reader rejects increment");

    writer.close().unwrap();
    println!("test_readonly_mode OK");
}

fn test_single_writer() {
    let tmp = std::env::temp_dir();
    let disk = format!("{}/test-sw-{}.db", tmp.display(), std::process::id());
    let wal = format!("{}.wal", disk);

    // First writer opens
    let mut writer1 = TieredStore::new(&disk, &wal, 10000, 100).unwrap();
    writer1.set("KEY1", "value1").unwrap();

    // Second writer must fail (WAL lock)
    let result = TieredStore::new(&disk, &wal, 10000, 100);
    assert!(result.is_err(), "second writer blocked");

    // Flush writer before opening reader
    writer1.flush().unwrap();
    let reader = TieredStore::open_readonly(&disk).unwrap();
    assert_eq!(reader.get("KEY1"), Some("value1".to_string()), "reader works");

    writer1.close().unwrap();
    println!("test_single_writer OK");
}
