#[test]
fn rv_tunables_a3d3695d_interner_quota_env_parse() {
    use crate::instance_defaults::{
        parse_interner_max_name_bytes, parse_interner_max_names, INTERNER_MAX_NAMES,
        INTERNER_MAX_NAMES_ENV, INTERNER_MAX_NAME_BYTES, INTERNER_MAX_NAME_BYTES_ENV,
    };

    assert_eq!(parse_interner_max_names(Some(" 42 ")), 42);
    assert_eq!(parse_interner_max_names(Some("0")), INTERNER_MAX_NAMES);
    assert_eq!(parse_interner_max_names(Some("bad")), INTERNER_MAX_NAMES);
    assert_eq!(parse_interner_max_names(None), INTERNER_MAX_NAMES);
    assert_eq!(parse_interner_max_name_bytes(Some("512")), 512);
    assert_eq!(
        parse_interner_max_name_bytes(Some("0")),
        INTERNER_MAX_NAME_BYTES
    );
    assert_eq!(
        parse_interner_max_name_bytes(Some("bad")),
        INTERNER_MAX_NAME_BYTES
    );
    assert_eq!(parse_interner_max_name_bytes(None), INTERNER_MAX_NAME_BYTES);
    assert_eq!(INTERNER_MAX_NAMES, 1_048_576);
    assert_eq!(INTERNER_MAX_NAME_BYTES, 64 * 1024 * 1024);
    assert_eq!(INTERNER_MAX_NAMES_ENV, "SHAMIR_INTERNER_MAX_NAMES");
    assert_eq!(
        INTERNER_MAX_NAME_BYTES_ENV,
        "SHAMIR_INTERNER_MAX_NAME_BYTES"
    );
}
