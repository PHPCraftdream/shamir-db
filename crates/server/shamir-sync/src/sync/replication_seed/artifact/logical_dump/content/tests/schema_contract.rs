use super::super::definitions::{PortableSchemaItem, SchemaRule, SchemaValueKind};

#[test]
fn schema_rule_shape_has_no_catch_all_metadata() {
    let rule = SchemaRule {
        path: vec!["profile".into(), "name".into()],
        kind: SchemaValueKind::Str,
        required: true,
        nullable: false,
        unsigned: false,
        min: None,
        max: None,
        len: Some(10),
        max_len: None,
        min_len: None,
        one_of: Vec::new(),
        default: None,
        array_of: None,
        scalar: None,
        format: None,
        compare: None,
        foreign_key: None,
        unique: false,
        auto_now: false,
        auto_now_add: false,
        keyset_safe: false,
    };
    let schema = PortableSchemaItem {
        repository: "main".into(),
        table: "accounts".into(),
        rules: vec![rule],
    };
    assert_eq!(schema.rules[0].path, ["profile", "name"]);
    assert_eq!(schema.rules[0].kind, SchemaValueKind::Str);
}
