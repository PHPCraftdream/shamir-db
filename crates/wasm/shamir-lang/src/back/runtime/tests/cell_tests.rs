use super::super::{Cell, CellTag};

#[test]
fn cell_round_trip_preserves_tag_and_fields() {
    let cell = Cell::new(CellTag::DynamicMap, 17, 0x1020_3040_5060_7080);
    assert_eq!(Cell::decode(cell.encode()), Some(cell));
    assert_eq!(cell.tag(), CellTag::DynamicMap);
    assert_eq!(cell.auxiliary(), 17);
    assert_eq!(cell.payload(), 0x1020_3040_5060_7080);
}
