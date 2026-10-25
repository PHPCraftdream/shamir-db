use crate::{new_map, new_set};

/// RV-COLLECTIONS-91305482: `shift_remove` preserves order, `swap_remove` does not.
#[test]
fn rv_collections_91305482_shift_remove_preserves_order_swap_remove_reorders() {
    // TMap: shift_remove keeps relative order of survivors.
    let mut m = new_map::<&str, u64>();
    for (k, v) in [("a", 1), ("b", 2), ("c", 3), ("d", 4)] {
        m.insert(k, v);
    }
    m.shift_remove(&"b");
    assert_eq!(m.keys().copied().collect::<Vec<_>>(), vec!["a", "c", "d"]);

    // TMap: swap_remove fills the vacated slot with the last element.
    let mut m = new_map::<&str, u64>();
    for (k, v) in [("a", 1), ("b", 2), ("c", 3), ("d", 4)] {
        m.insert(k, v);
    }
    m.swap_remove(&"b");
    assert_eq!(m.keys().copied().collect::<Vec<_>>(), vec!["a", "d", "c"]);

    // TSet mirrors the same asymmetry.
    let mut s = new_set::<&str>();
    for k in ["a", "b", "c", "d"] {
        s.insert(k);
    }
    s.shift_remove(&"b");
    assert_eq!(s.iter().copied().collect::<Vec<_>>(), vec!["a", "c", "d"]);

    let mut s = new_set::<&str>();
    for k in ["a", "b", "c", "d"] {
        s.insert(k);
    }
    s.swap_remove(&"b");
    assert_eq!(s.iter().copied().collect::<Vec<_>>(), vec!["a", "d", "c"]);
}
