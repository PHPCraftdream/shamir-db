//! RV-QUERY-BUILDER-E6F53E38 — conditional HMAC gating for the schema
//! function/validator builders, plus the newly gated function/validator and
//! replication drop builders. Each no-tag rejection fails on the pre-fix
//! code, where `build()` returned a `BatchOp` with `hmac: None`.

use shamir_query_types::batch::BatchOp;

use crate::ddl;
use crate::write::BuilderError;

/// Conditional gate: `definer || replace || secret_grants` requires a tag;
/// `net_grants` alone does NOT gate.
#[test]
fn rv_query_builder_e6f53e38_create_function_conditional_hmac_gate() {
    // Invoker + net_grants only: NotGated, builds without a tag.
    let op = ddl::create_function("f")
        .net_grants(["example.com"])
        .build()
        .unwrap();
    match &op {
        BatchOp::CreateFunction(c) => assert!(c.hmac.is_none()),
        _ => panic!("expected CreateFunction"),
    }
    // definer security: gated.
    let gated = ddl::create_function("f").security("definer").build().err();
    assert_eq!(gated, Some(BuilderError::MissingHmac));
    // replace: gated.
    let gated = ddl::create_function("f").replace().build().err();
    assert_eq!(gated, Some(BuilderError::MissingHmac));
    // secret grants: gated.
    let gated = ddl::create_function("f")
        .secret_grants(["TOKEN"])
        .build()
        .err();
    assert_eq!(gated, Some(BuilderError::MissingHmac));
    // Signed: same shape as the invoker build plus the tag.
    let op = ddl::create_function("f")
        .security("definer")
        .hmac("beef")
        .build()
        .unwrap();
    match op {
        BatchOp::CreateFunction(c) => {
            assert_eq!(c.security.as_deref(), Some("definer"));
            assert_eq!(c.hmac.as_deref(), Some("beef"));
        }
        _ => panic!("expected CreateFunction"),
    }
}

/// Conditional gate: `replace` requires a tag; a plain create does not.
#[test]
fn rv_query_builder_e6f53e38_create_validator_conditional_hmac_gate() {
    let op = ddl::create_validator("v")
        .source("pub fn validate() {}")
        .build()
        .unwrap();
    match &op {
        BatchOp::CreateValidator(c) => assert!(c.hmac.is_none()),
        _ => panic!("expected CreateValidator"),
    }
    let gated = ddl::create_validator("v").replace().build().err();
    assert_eq!(gated, Some(BuilderError::MissingHmac));
    let op = ddl::create_validator("v")
        .replace()
        .hmac("beef")
        .build()
        .unwrap();
    match op {
        BatchOp::CreateValidator(c) => {
            assert!(c.replace);
            assert_eq!(c.hmac.as_deref(), Some("beef"));
        }
        _ => panic!("expected CreateValidator"),
    }
}

/// The newly gated drop families reject a missing tag and build the same
/// shape with one.
#[test]
fn rv_query_builder_e6f53e38_newly_gated_drops_require_tag() {
    for res in [
        ddl::drop_function("f").build(),
        ddl::drop_validator("v").build(),
        ddl::drop_replication_profile("cluster").build(),
        ddl::drop_publication("pub").build(),
        ddl::drop_subscription("sub").build(),
    ] {
        assert_eq!(res.err(), Some(BuilderError::MissingHmac));
    }
    let op = ddl::drop_function("f").hmac("beef").build().unwrap();
    match op {
        BatchOp::DropFunction(d) => {
            assert_eq!(d.drop_function, "f");
            assert_eq!(d.hmac.as_deref(), Some("beef"));
        }
        _ => panic!("expected DropFunction"),
    }
    let op = ddl::drop_subscription("sub").hmac("beef").build().unwrap();
    match op {
        BatchOp::DropSubscription(d) => {
            assert_eq!(d.drop_subscription, "sub");
            assert_eq!(d.hmac.as_deref(), Some("beef"));
        }
        _ => panic!("expected DropSubscription"),
    }
}
