use crate::{
    new_fx_map_wc, new_fx_set_wc, new_map_wc, new_set_wc, try_new_fx_map_wc, try_new_fx_set_wc,
    try_new_map_wc, try_new_set_wc,
};

#[test]
fn rv_collections_a5293f80_fallible_capacity_constructors_reserve_and_reject_overflow() {
    let mut map = try_new_map_wc::<u8, u8>(4).expect("small map reservation");
    assert!(map.capacity() >= 4);
    map.insert(1, 2);
    assert_eq!(map.get(&1), Some(&2));
    assert!(try_new_map_wc::<u8, u8>(usize::MAX).is_err());

    let mut set = try_new_set_wc::<u8>(4).expect("small set reservation");
    assert!(set.capacity() >= 4);
    set.insert(3);
    assert!(set.contains(&3));
    assert!(try_new_set_wc::<u8>(usize::MAX).is_err());

    let mut fx_map = try_new_fx_map_wc::<u8, u8>(4).expect("small map reservation");
    assert!(fx_map.capacity() >= 4);
    fx_map.insert(4, 5);
    assert_eq!(fx_map.get(&4), Some(&5));
    assert!(try_new_fx_map_wc::<u8, u8>(usize::MAX).is_err());

    let mut fx_set = try_new_fx_set_wc::<u8>(4).expect("small set reservation");
    assert!(fx_set.capacity() >= 4);
    fx_set.insert(6);
    assert!(fx_set.contains(&6));
    assert!(try_new_fx_set_wc::<u8>(usize::MAX).is_err());

    assert_eq!(new_map_wc::<u8, u8>(0).capacity(), 0);
    assert_eq!(new_set_wc::<u8>(0).capacity(), 0);
    assert_eq!(new_fx_map_wc::<u8, u8>(0).capacity(), 0);
    assert_eq!(new_fx_set_wc::<u8>(0).capacity(), 0);
}
