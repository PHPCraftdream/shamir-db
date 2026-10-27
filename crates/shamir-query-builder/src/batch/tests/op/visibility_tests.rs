use shamir_query_types::batch::VisibilityWait;

use crate::batch::Batch;

#[test]
fn visibility_override_survives_builder_and_named_msgpack() {
    assert_eq!(Batch::new().build().visibility_wait, None);
    for wait in [
        VisibilityWait::Strict,
        VisibilityWait::Bounded { timeout_ms: 0 },
    ] {
        let mut batch = Batch::new();
        batch.visibility_wait(wait);
        let request = batch.to_request_via_msgpack().unwrap();
        assert_eq!(request.visibility_wait, Some(wait));
        assert_eq!(request, batch.build());
    }
}
