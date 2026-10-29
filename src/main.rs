//! TieredStore property-verification driver.
//!
//! The checks live in `tieredstore::property` (so `cargo test` runs them);
//! this binary runs the same checks in order for CI and human inspection.

use tieredstore::property;

fn main() {
    println!("=== TieredStore Property Verification ===");
    println!();

    for (name, test) in property::ALL {
        test();
        println!("{} OK", name);
    }

    println!();
    println!("ALL PROPERTY TESTS PASSED");
}
