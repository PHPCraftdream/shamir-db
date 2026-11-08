use crate::api::CompileLimits;
use crate::front::{parse, ExprKind, StatementKind, TypeKind};

const NULL_FUNCTION: &str = "@entry scalar f() -> null {}";
const RETURN_FUNCTION: &str = "@entry scalar f() -> i64 { return 1; }";

fn code(source: &str, limits: &CompileLimits) -> Result<usize, String> {
    parse(source, limits)
        .map(|module| module.node_count())
        .map_err(|diagnostic| diagnostic.code.to_owned())
}

#[test]
fn node_counter_excludes_root_counts_each_arena_once_and_observes_exact_caps() {
    let basic = parse(NULL_FUNCTION, &CompileLimits::default()).expect("minimal module");
    assert_eq!(basic.node_count(), 3);
    assert_eq!(basic.functions.len(), 1);
    assert_eq!(basic.types.len(), 1);
    assert_eq!(basic.statements.len(), 1);
    assert_eq!(basic.parameters.len(), basic.parameter_ids.len());
    assert_eq!(basic.functions.len(), basic.function_ids.len());
    assert!(basic
        .function_ids
        .iter()
        .enumerate()
        .all(|(index, id)| id.0 as usize == index));

    let with_return = parse(RETURN_FUNCTION, &CompileLimits::default()).expect("literal return");
    assert_eq!(with_return.node_count(), 5);
    assert_eq!(
        code(
            NULL_FUNCTION,
            &CompileLimits {
                max_ast_nodes: 3,
                ..CompileLimits::default()
            }
        ),
        Ok(3)
    );
    assert_eq!(
        code(
            NULL_FUNCTION,
            &CompileLimits {
                max_ast_nodes: 2,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
    assert_eq!(
        code(
            RETURN_FUNCTION,
            &CompileLimits {
                max_ast_nodes: 5,
                ..CompileLimits::default()
            }
        ),
        Ok(5)
    );
    assert_eq!(
        code(
            RETURN_FUNCTION,
            &CompileLimits {
                max_ast_nodes: 4,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
    assert_eq!(
        code(
            NULL_FUNCTION,
            &CompileLimits {
                max_ast_nodes: 0,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
}

#[test]
fn function_parameter_record_field_and_local_limits_accept_exact_and_reject_over() {
    let one_function = "@entry scalar f() -> null {}";
    let two_functions = "fn helper() -> null {} @entry scalar f() -> null {}";
    assert_eq!(
        code(
            one_function,
            &CompileLimits {
                max_functions: 0,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
    assert!(parse(
        one_function,
        &CompileLimits {
            max_functions: 1,
            ..CompileLimits::default()
        }
    )
    .is_ok());
    assert_eq!(
        code(
            two_functions,
            &CompileLimits {
                max_functions: 1,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
    assert!(parse(
        two_functions,
        &CompileLimits {
            max_functions: 2,
            ..CompileLimits::default()
        }
    )
    .is_ok());

    let one_param = "@entry scalar f(a: i64) -> null {}";
    let two_params = "@entry scalar f(a: i64, b: i64) -> null {}";
    assert!(parse(
        one_param,
        &CompileLimits {
            max_parameters: 1,
            ..CompileLimits::default()
        }
    )
    .is_ok());
    assert_eq!(
        code(
            one_param,
            &CompileLimits {
                max_parameters: 0,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
    assert_eq!(
        code(
            two_params,
            &CompileLimits {
                max_parameters: 1,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
    assert!(parse(
        two_params,
        &CompileLimits {
            max_parameters: 2,
            ..CompileLimits::default()
        }
    )
    .is_ok());

    let one_type_field = "@entry scalar f(x: { a: i64 }) -> null {}";
    let two_type_fields = "@entry scalar f(x: { a: i64, b: i64 }) -> null {}";
    assert_eq!(
        code(
            one_type_field,
            &CompileLimits {
                max_record_fields: 0,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
    assert!(parse(
        one_type_field,
        &CompileLimits {
            max_record_fields: 1,
            ..CompileLimits::default()
        }
    )
    .is_ok());
    assert_eq!(
        code(
            two_type_fields,
            &CompileLimits {
                max_record_fields: 1,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
    assert!(parse(
        two_type_fields,
        &CompileLimits {
            max_record_fields: 2,
            ..CompileLimits::default()
        }
    )
    .is_ok());

    let one_value_field = "@entry scalar f() -> null { let x = { a: 1, }; }";
    let two_value_fields = "@entry scalar f() -> null { let x = { a: 1, b: 2, }; }";
    assert_eq!(
        code(
            one_value_field,
            &CompileLimits {
                max_record_fields: 0,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
    assert!(parse(
        one_value_field,
        &CompileLimits {
            max_record_fields: 1,
            ..CompileLimits::default()
        }
    )
    .is_ok());
    assert_eq!(
        code(
            two_value_fields,
            &CompileLimits {
                max_record_fields: 1,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
    assert!(parse(
        two_value_fields,
        &CompileLimits {
            max_record_fields: 2,
            ..CompileLimits::default()
        }
    )
    .is_ok());

    let one_local = "@entry scalar f() -> null { let a = 1; }";
    let two_locals = "@entry scalar f() -> null { let a = 1; const b = 2; }";
    let loop_local = "@entry scalar f() -> null { for (i in 0..1) {} }";
    assert_eq!(
        code(
            one_local,
            &CompileLimits {
                max_locals_per_function: 0,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
    assert!(parse(
        one_local,
        &CompileLimits {
            max_locals_per_function: 1,
            ..CompileLimits::default()
        }
    )
    .is_ok());
    assert_eq!(
        code(
            two_locals,
            &CompileLimits {
                max_locals_per_function: 1,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
    assert!(parse(
        two_locals,
        &CompileLimits {
            max_locals_per_function: 2,
            ..CompileLimits::default()
        }
    )
    .is_ok());
    assert_eq!(
        code(
            loop_local,
            &CompileLimits {
                max_locals_per_function: 0,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
}

#[test]
fn type_depth_and_constructed_ast_depth_have_exact_boundaries() {
    let primitive = "@entry scalar f(x: i64) -> null {}";
    let list = "@entry scalar f(x: [i64]) -> null {}";
    let optional = "@entry scalar f(x: i64?) -> null {}";
    let nested = "@entry scalar f(x: [i64]?) -> null {}";
    let optional_element = "@entry scalar f(x: [i64?]) -> null {}";
    assert!(parse(
        primitive,
        &CompileLimits {
            max_type_depth: 1,
            ..CompileLimits::default()
        }
    )
    .is_ok());
    assert_eq!(
        code(
            primitive,
            &CompileLimits {
                max_type_depth: 0,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
    assert!(parse(
        list,
        &CompileLimits {
            max_type_depth: 2,
            ..CompileLimits::default()
        }
    )
    .is_ok());
    assert_eq!(
        code(
            list,
            &CompileLimits {
                max_type_depth: 1,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
    assert!(parse(
        optional,
        &CompileLimits {
            max_type_depth: 2,
            ..CompileLimits::default()
        }
    )
    .is_ok());
    assert_eq!(
        code(
            optional,
            &CompileLimits {
                max_type_depth: 1,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
    assert!(parse(
        nested,
        &CompileLimits {
            max_type_depth: 3,
            ..CompileLimits::default()
        }
    )
    .is_ok());
    assert_eq!(
        code(
            nested,
            &CompileLimits {
                max_type_depth: 2,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
    assert_eq!(
        code(
            optional_element,
            &CompileLimits {
                max_type_depth: 2,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );

    assert!(parse(
        NULL_FUNCTION,
        &CompileLimits {
            max_ast_depth: 2,
            ..CompileLimits::default()
        }
    )
    .is_ok());
    assert_eq!(
        code(
            NULL_FUNCTION,
            &CompileLimits {
                max_ast_depth: 1,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
    assert!(parse(
        RETURN_FUNCTION,
        &CompileLimits {
            max_ast_depth: 4,
            ..CompileLimits::default()
        }
    )
    .is_ok());
    assert_eq!(
        code(
            RETURN_FUNCTION,
            &CompileLimits {
                max_ast_depth: 3,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
    let module = parse(RETURN_FUNCTION, &CompileLimits::default()).expect("flat AST nodes");
    assert!(module
        .expressions
        .iter()
        .any(|node| matches!(node.kind, ExprKind::Integer { magnitude: 1, .. })));
    assert!(module
        .types
        .iter()
        .any(|node| matches!(node.kind, TypeKind::Primitive(_))));
    assert!(module
        .statements
        .iter()
        .any(|node| matches!(node.kind, StatementKind::Return(Some(_)))));
}

#[test]
fn nested_blocks_and_long_left_associated_expressions_stop_at_depth_cap() {
    let exact_blocks = format!(
        "@entry scalar f() -> null {{ {}{} }}",
        "{".repeat(5),
        "}".repeat(5)
    );
    let over_blocks = format!(
        "@entry scalar f() -> null {{ {}{} }}",
        "{".repeat(6),
        "}".repeat(6)
    );
    let limits = CompileLimits {
        max_ast_depth: 7,
        ..CompileLimits::default()
    };
    assert!(parse(&exact_blocks, &limits).is_ok());
    assert_eq!(code(&over_blocks, &limits), Err("B001".to_owned()));

    let exact_chain = format!(
        "@entry scalar f() -> i64 {{ return {}; }}",
        std::iter::repeat_n("1", 5).collect::<Vec<_>>().join(" + ")
    );
    let over_chain = format!(
        "@entry scalar f() -> i64 {{ return {}; }}",
        std::iter::repeat_n("1", 6).collect::<Vec<_>>().join(" + ")
    );
    let limits = CompileLimits {
        max_ast_depth: 8,
        ..CompileLimits::default()
    };
    assert!(parse(&exact_chain, &limits).is_ok());
    assert_eq!(code(&over_chain, &limits), Err("B001".to_owned()));

    let hard_edge = format!(
        "@entry scalar f() -> i64 {{ return {}; }}",
        std::iter::repeat_n("1", 125)
            .collect::<Vec<_>>()
            .join(" + ")
    );
    assert!(parse(&hard_edge, &CompileLimits::default()).is_ok());
    let over_hard = format!(
        "@entry scalar f() -> i64 {{ return {}; }}",
        std::iter::repeat_n("1", 126)
            .collect::<Vec<_>>()
            .join(" + ")
    );
    assert_eq!(
        code(&over_hard, &CompileLimits::default()),
        Err("B001".to_owned())
    );
}

#[test]
fn reserved_comparison_and_mixed_arenas_have_exact_node_counts_and_caps() {
    let comparison = "@entry scalar f() -> bool { return 1 < 2; }";
    let comparison_module = parse(comparison, &CompileLimits::default()).expect("comparison AST");
    assert_eq!(comparison_module.node_count(), 7);
    assert_eq!(comparison_module.functions.len(), 1);
    assert_eq!(comparison_module.parameters.len(), 0);
    assert_eq!(comparison_module.expressions.len(), 3);
    assert_eq!(comparison_module.value_fields.len(), 0);
    assert_eq!(comparison_module.statements.len(), 2);
    assert_eq!(comparison_module.types.len(), 1);
    assert_eq!(comparison_module.type_fields.len(), 0);
    assert_eq!(
        code(
            comparison,
            &CompileLimits {
                max_ast_nodes: 7,
                ..CompileLimits::default()
            }
        ),
        Ok(7)
    );
    assert_eq!(
        code(
            comparison,
            &CompileLimits {
                max_ast_nodes: 6,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );

    let mixed = "@entry scalar f(p: { a: i64 }) -> null {\n    let x = { a: 1 };\n}";
    let mixed_module = parse(mixed, &CompileLimits::default()).expect("mixed-arena AST");
    assert_eq!(mixed_module.node_count(), 11);
    assert_eq!(mixed_module.functions.len(), 1);
    assert_eq!(mixed_module.parameters.len(), 1);
    assert_eq!(mixed_module.expressions.len(), 2);
    assert_eq!(mixed_module.value_fields.len(), 1);
    assert_eq!(mixed_module.statements.len(), 2);
    assert_eq!(mixed_module.types.len(), 3);
    assert_eq!(mixed_module.type_fields.len(), 1);
    assert_eq!(
        code(
            mixed,
            &CompileLimits {
                max_ast_nodes: 11,
                ..CompileLimits::default()
            }
        ),
        Ok(11)
    );
    assert_eq!(
        code(
            mixed,
            &CompileLimits {
                max_ast_nodes: 10,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
}

#[test]
fn grouped_unary_and_nested_list_expressions_obey_ast_depth_boundaries() {
    let exact_groups = format!(
        "@entry scalar f() -> i64 {{ return {}1{}; }}",
        "(".repeat(124),
        ")".repeat(124)
    );
    assert!(parse(&exact_groups, &CompileLimits::default()).is_ok());
    let over_groups = format!(
        "@entry scalar f() -> i64 {{ return {}1{}; }}",
        "(".repeat(125),
        ")".repeat(125)
    );
    assert_eq!(
        code(&over_groups, &CompileLimits::default()),
        Err("B001".to_owned())
    );

    let limits = CompileLimits {
        max_ast_depth: 8,
        ..CompileLimits::default()
    };
    let exact_unary = format!("@entry scalar f() -> i64 {{ return {}1; }}", "+".repeat(4));
    assert!(parse(&exact_unary, &limits).is_ok());
    let over_unary = format!("@entry scalar f() -> i64 {{ return {}1; }}", "+".repeat(5));
    assert_eq!(code(&over_unary, &limits), Err("B001".to_owned()));

    let exact_lists = "@entry scalar f() -> i64 { return [[[[1]]]]; }";
    assert!(parse(exact_lists, &limits).is_ok());
    let over_lists = "@entry scalar f() -> i64 { return [[[[[1]]]]]; }";
    assert_eq!(code(over_lists, &limits), Err("B001".to_owned()));
}
