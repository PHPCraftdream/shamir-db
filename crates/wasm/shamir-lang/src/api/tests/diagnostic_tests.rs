use crate::api::{Diagnostic, Position, Span, MAX_DIAGNOSTIC_MESSAGE_BYTES};

#[test]
fn diagnostic_message_and_related_message_are_bounded_and_utf8_safe() {
    let span = Span::new(
        2,
        4,
        Position { line: 3, column: 5 },
        Position { line: 3, column: 7 },
    );
    let long = "界".repeat(MAX_DIAGNOSTIC_MESSAGE_BYTES);
    let diagnostic = Diagnostic::new("L002", long.clone(), span).with_related(span, long);
    assert!(diagnostic.message.len() <= MAX_DIAGNOSTIC_MESSAGE_BYTES);
    assert!(diagnostic
        .message
        .is_char_boundary(diagnostic.message.len()));
    let related = diagnostic.related.as_ref().expect("related source");
    assert!(related.message.len() <= MAX_DIAGNOSTIC_MESSAGE_BYTES);
    assert!(related.message.is_char_boundary(related.message.len()));
}

#[test]
fn diagnostic_rendering_is_deterministic_and_orders_related_location_after_primary() {
    let primary = Span::new(
        0,
        2,
        Position { line: 2, column: 4 },
        Position { line: 2, column: 6 },
    );
    let secondary = Span::new(
        8,
        9,
        Position { line: 5, column: 1 },
        Position { line: 5, column: 2 },
    );
    let diagnostic =
        Diagnostic::new("P002", "duplicate entry", primary).with_related(secondary, "first entry");
    let rendered = "P002:2:4: duplicate entry\n  related 5:1: first entry";
    assert_eq!(diagnostic.to_string(), rendered);
    assert_eq!(diagnostic.to_string(), rendered);
}
