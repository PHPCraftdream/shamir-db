use crate::filter::FilterValue;
use crate::ops::call::CallOp;

#[test]
fn constructors_match_independent_struct_literal_wire_oracles() {
    let cases = [
        (
            CallOp::new("daily", std::iter::empty::<FilterValue>()),
            CallOp {
                call: "daily".to_owned(),
                params: Vec::new(),
                repo: "main".to_owned(),
            },
        ),
        (
            CallOp::new(
                "daily",
                [FilterValue::Int(3), FilterValue::String("x".into())],
            ),
            CallOp {
                call: "daily".to_owned(),
                params: vec![FilterValue::Int(3), FilterValue::String("x".into())],
                repo: "main".to_owned(),
            },
        ),
        (
            CallOp::in_repo("daily", "archive", [FilterValue::Int(3)]),
            CallOp {
                call: "daily".to_owned(),
                params: vec![FilterValue::Int(3)],
                repo: "archive".to_owned(),
            },
        ),
    ];
    for (actual, expected) in cases {
        assert_eq!(actual, expected);
        assert_eq!(
            rmp_serde::to_vec_named(&actual).unwrap(),
            rmp_serde::to_vec_named(&expected).unwrap()
        );
    }
}
