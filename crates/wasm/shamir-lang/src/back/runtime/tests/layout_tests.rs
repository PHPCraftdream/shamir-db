use super::super::layout::LayoutError;
use super::super::{Layout, MAX_STATIC_BYTES};

#[test]
fn layout_places_workspace_and_heap_on_eight_byte_boundaries() {
    let layout = Layout::calculate(3, 0).expect("small static section");
    assert_eq!(layout.workspace_start % 8, 0);
    assert_eq!(layout.heap_base % 8, 0);
    assert_eq!(layout.workspace_end - layout.workspace_start, 65_536);
    assert!(layout.minimum_pages <= layout.maximum_pages);
}

#[test]
fn layout_reserves_the_cell_budget_below_the_heap() {
    let layout = Layout::calculate(0, 128).expect("small cell budget");
    assert_eq!(layout.workspace_end - layout.workspace_start, 65_536 + 128);
    assert!(layout.heap_base >= layout.workspace_end);
    assert_eq!(layout.heap_base % 8, 0);
}

#[test]
fn layout_rejects_static_data_above_cap() {
    assert_eq!(
        Layout::calculate(MAX_STATIC_BYTES + 1, 0),
        Err(LayoutError::StaticCap)
    );
}

#[test]
fn layout_rejects_cell_budget_overflow() {
    assert_eq!(Layout::calculate(0, u64::MAX), Err(LayoutError::Overflow));
}
