use super::super::{emit_allocator, Layout};

#[test]
fn allocator_emitter_accepts_layout_and_indices() {
    let layout = Layout::calculate(3, 0).expect("layout");
    let _function = emit_allocator(layout, 0, 0);
    assert_eq!(layout.heap_base % 8, 0);
}

#[test]
fn allocator_body_is_structurally_sound() {
    let layout = Layout::calculate(3, 0).expect("layout");
    let body = emit_allocator(layout, 0, 0).into_raw_body();
    // every function body ends with the implicit `end` opcode
    assert_eq!(*body.last().expect("encoded allocator body"), 0x0b);
    // the allocator must grow memory (memory.grow = 0x40) before publishing
    assert!(
        body.contains(&0x40),
        "allocator body must contain the memory.grow opcode"
    );
}
