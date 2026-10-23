use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_types::batch::{BatchError, BatchLimits, LimitKind};
use shamir_query_types::filter::FilterValue;

use crate::query::batch::batch_validate::validate_pagination;

// Red before the fix: none of these reads was checked before execution.
#[test]
fn pagination_ceiling_covers_reads_in_nested_batches_and_empty_loops() {
    let queries = [
        Query::from("t").limit(u64::MAX).build(),
        Query::from("t").offset(u64::MAX).build(),
        Query::from("t").page(u64::MAX, u64::MAX).build(),
        Query::from("t").after(Vec::new(), Some(u64::MAX)).build(),
    ];
    for query in queries {
        let mut leaf = Batch::new();
        leaf.query("r", query);
        let leaf = leaf.build();
        let mut nested = Batch::new();
        nested.sub_batch_no_bind("nested", leaf.clone());
        let mut loop_batch = Batch::new();
        loop_batch.for_each(
            "loop",
            FilterValue::Array(Vec::new()),
            "row",
            nested.build(),
        );
        for request in [leaf, loop_batch.build()] {
            let error = validate_pagination(&request.queries, &BatchLimits::default()).unwrap_err();
            match error {
                BatchError::LimitExceeded {
                    kind: LimitKind::PageSize | LimitKind::Offset,
                    ..
                } => {}
                BatchError::QueryError { code, .. } => {
                    assert_eq!(code.as_deref(), Some("pagination_too_large"));
                }
                error => panic!("unexpected pagination error: {error:?}"),
            }
        }
    }
}

#[test]
fn pagination_ceiling_preserves_normal_and_unpaginated_reads() {
    for query in [
        Query::from("t").build(),
        Query::from("t").limit(100_000).offset(1_000_000).build(),
        Query::from("t").page(3, 10).build(),
    ] {
        let mut batch = Batch::new();
        batch.query("r", query);
        assert_eq!(
            validate_pagination(&batch.build().queries, &BatchLimits::default()),
            Ok(()),
        );
    }
}
