# tieredstore-rs

TieredStore in Rust: **LRU cache + disk backing + WAL crash recovery**, the Rust
sibling of mJVM's `jvmumps.store.TieredStore` (Java), with runtime property
verification matching the Dafny specification in
[`m-semantics/TieredStore.dfy`](https://github.com/mjandrews1/m-semantics).

## Architecture

```
        writes ──► WAL (append-only, CRC32 per entry)  ──► crash recovery
                    │
                LRU cache (hot keys, write-aware eviction: clean pages first)
                    │ eviction (dirty pages flushed first)
                    ▼
                disk backing (sled)
```

- **Hot keys** live in the LRU cache for fast reads/writes.
- **Cold keys** live on disk (sled) for permanence.
- **Write-behind + WAL** for crash recovery: at most one write lost.
- **Single writer + read-only readers**: writer holds the exclusive WAL lock;
  readers work from snapshots and never take the file lock.

## Modules

| Module | Role |
|---|---|
| `src/lru.rs` | LRU cache with access-order tracking, dirty-page flush on eviction |
| `src/wal.rs` | Write-ahead log: entries, ops, CRC32 integrity |
| `src/store.rs` | `TieredStore` — cache + disk + WAL coordination |
| `src/main.rs` | Property-verification driver (9 tests, one per Dafny lemma) |

## Property verification

Each test in `src/main.rs` maps to a Dafny lemma in `TieredStore.dfy`:

| Test | Dafny lemma |
|---|---|
| `test_write_ahead_property` | WriteAheadProperty (cache ⊆ WAL) |
| `test_wal_replay_correctness` | replayRecoversSetOps |
| `test_crash_safety` | crashSafety (max 1 write loss) |
| `test_eviction_safety` | evictionSafety (dirty pages flushed before eviction) |
| `test_read_only_safety` | ReadOnlySafe (read-only rejects writes) |
| `test_single_writer_exclusivity` | WriterExclusive |
| `test_snapshot_consistency` | SnapshotConsistent (no phantom data) |
| `test_consistency_after_set` | set/get round-trip |
| `test_consistency_after_flush` | flush preserves data; persistence across reopen |

Run:

```bash
cargo run
```

All 9 must print OK (asserts panic on failure → non-zero exit). CI runs the
same driver on every push/PR.

## Dependencies

- `sled` 0.34 — embedded disk store
- `crc32fast` 1.4 — WAL entry integrity

## Related

- **m-semantics** — Dafny proofs (`TieredStore.dfy`): the spec this crate verifies at runtime
- **mJVM** — the Java implementation (`jvmumps.store.TieredStore`) this is a port study for
