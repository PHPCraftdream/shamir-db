use super::super::helpers::HelperOrderError;
use super::super::helpers::{emit_helper_order, HelperGraph};

#[test]
fn helper_order_is_stable_and_dependency_first() {
    let mut graph = HelperGraph::default();
    let a = graph.add(&[]);
    let b = graph.add(&[a]);
    let c = graph.add(&[a]);
    assert_eq!(
        emit_helper_order(&graph, &[c, b]).expect("acyclic"),
        vec![a, b, c]
    );
}

#[test]
fn helper_order_rejects_cycles() {
    let mut graph = HelperGraph::default();
    let first = graph.add(&[]);
    let second = graph.add(&[first]);
    graph.add_dependency(first, second);
    assert_eq!(
        emit_helper_order(&graph, &[first]),
        Err(HelperOrderError::Cycle)
    );
}

#[test]
fn decoder_order_emits_every_primitive_before_the_decoder() {
    let mut graph = HelperGraph::default();
    let range = graph.add(&[]);
    let compare = graph.add(&[]);
    let read_byte = graph.add(&[]);
    let read_be = graph.add(&[read_byte]);
    let utf8 = graph.add(&[]);
    let store = graph.add(&[]);
    let decode = graph.add(&[range, read_byte, read_be, utf8, compare, store]);
    let order = emit_helper_order(&graph, &[decode]).expect("acyclic");
    let position = |helper| order.iter().position(|&id| id == helper).expect("present");
    let decode_position = position(decode);
    assert!(position(range) < decode_position);
    assert!(position(read_byte) < decode_position);
    assert!(position(read_be) < decode_position);
    assert!(position(utf8) < decode_position);
    assert!(position(compare) < decode_position);
    assert!(position(store) < decode_position);
}

#[test]
fn decoder_cycle_through_a_primitive_is_rejected() {
    let mut graph = HelperGraph::default();
    let read_byte = graph.add(&[]);
    let read_be = graph.add(&[read_byte]);
    let decode = graph.add(&[read_be]);
    graph.add_dependency(read_be, decode);
    assert_eq!(
        emit_helper_order(&graph, &[decode]),
        Err(HelperOrderError::Cycle)
    );
}

#[test]
fn encoder_order_emits_after_append_and_compare() {
    let mut graph = HelperGraph::default();
    let append = graph.add(&[]);
    let compare = graph.add(&[]);
    let decode = graph.add(&[append]);
    let encode = graph.add(&[append, compare]);
    let order = emit_helper_order(&graph, &[decode, encode]).expect("acyclic");
    let position = |helper| order.iter().position(|&id| id == helper).expect("present");
    assert!(position(append) < position(encode));
    assert!(position(compare) < position(encode));
}
