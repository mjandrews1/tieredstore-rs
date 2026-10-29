//! Randomized property test: random op sequences against a reference model.
//!
//! The scenario tests in `src/property.rs` verify each Dafny lemma on fixed
//! scripts; this test verifies the composite invariants — replayDecompose,
//! replayRecoversSetOps, lastValueForPreserves, appendKillPreservesWriteAhead,
//! and evictionSafety under a cache smaller than the key space — on random
//! SET/KILL sequences. After flush + reopen (consistencyAfterFlush: zero
//! loss), the store state must equal the model exactly.

use proptest::prelude::*;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use tieredstore::TieredStore;

static CASE: AtomicUsize = AtomicUsize::new(0);

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn random_ops_match_reference_model(
        ops in prop::collection::vec((0usize..12, prop::bool::ANY, 0u32..1000), 1..64)
    ) {
        let n = CASE.fetch_add(1, Ordering::Relaxed);
        let tmp = std::env::temp_dir();
        let disk = format!("{}/prop-rnd-{}-{}.db", tmp.display(), std::process::id(), n);
        let wal = format!("{}.wal", disk);

        let mut model: BTreeMap<String, String> = BTreeMap::new();
        {
            // Cache size 4 over a 12-key space: forces evictions on most runs.
            let mut store = TieredStore::new(&disk, &wal, 4, 100)
                .expect("open writer");
            for (key_idx, is_kill, val) in &ops {
                let key = format!("K{}", key_idx);
                if *is_kill {
                    store.kill(&key).expect("kill");
                    model.remove(&key);
                } else {
                    let value = format!("V{}", val);
                    store.set(&key, &value).expect("set");
                    model.insert(key, value);
                }
            }
            // Flush before close: consistencyAfterFlush gives the exact state.
            store.flush().expect("flush");
        }

        let store2 = TieredStore::new(&disk, &wal, 4, 100)
            .expect("reopen after flush");
        for (k, v) in &model {
            prop_assert_eq!(store2.get(k), Some(v.clone()), "model key {}", k);
        }
        for key_idx in 0..12 {
            let key = format!("K{}", key_idx);
            if !model.contains_key(&key) {
                prop_assert_eq!(store2.get(&key), None, "absent key {}", key);
            }
        }
    }
}
