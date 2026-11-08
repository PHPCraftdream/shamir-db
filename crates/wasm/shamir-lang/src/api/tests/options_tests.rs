use crate::api::{CompileLimits, LimitError};

#[test]
fn every_limit_clamps_oversized_values_to_its_hard_ceiling() {
    let oversized = CompileLimits {
        max_source_bytes: usize::MAX,
        max_tokens: usize::MAX,
        max_ast_nodes: usize::MAX,
        max_ast_depth: usize::MAX,
        max_type_depth: usize::MAX,
        max_functions: usize::MAX,
        max_locals_per_function: usize::MAX,
        max_parameters: usize::MAX,
        max_record_fields: usize::MAX,
        max_single_literal_bytes: usize::MAX,
        max_total_literal_bytes: usize::MAX,
        max_static_bytes: usize::MAX,
        max_output_bytes: usize::MAX,
        max_runtime_bytes: usize::MAX,
        max_function_body_bytes: usize::MAX,
        max_operators_per_function: usize::MAX,
        max_ir_nodes: usize::MAX,
        max_call_chain: usize::MAX,
        compile_fuel: u64::MAX,
    }
    .clamped()
    .expect("positive limits");
    assert_eq!(
        oversized,
        CompileLimits {
            max_source_bytes: 1_048_576,
            max_tokens: 200_000,
            max_ast_nodes: 200_000,
            max_ast_depth: 128,
            max_type_depth: 128,
            max_functions: 256,
            max_locals_per_function: 1024,
            max_parameters: 32,
            max_record_fields: 128,
            max_single_literal_bytes: 65_536,
            max_total_literal_bytes: 4_194_304,
            max_static_bytes: 4_194_304,
            max_output_bytes: 8_388_608,
            max_runtime_bytes: 65_536,
            max_function_body_bytes: 1_048_576,
            max_operators_per_function: 500_000,
            max_ir_nodes: 200_000,
            max_call_chain: 64,
            compile_fuel: 20_000_000,
        }
    );
}

#[test]
fn zero_limit_is_allowed_only_in_test_configuration() {
    let result = CompileLimits {
        max_source_bytes: 0,
        ..CompileLimits::default()
    }
    .clamped();
    if cfg!(test) {
        assert_eq!(result.expect("test zero bound").max_source_bytes, 0);
    } else {
        assert_eq!(
            result,
            Err(LimitError {
                field: "max_source_bytes"
            })
        );
    }
}

#[test]
fn each_limit_field_rejects_zero_in_production_configuration() {
    let defaults = CompileLimits::default();
    let zero_limits = [
        CompileLimits {
            max_source_bytes: 0,
            ..defaults
        },
        CompileLimits {
            max_tokens: 0,
            ..defaults
        },
        CompileLimits {
            max_ast_nodes: 0,
            ..defaults
        },
        CompileLimits {
            max_ast_depth: 0,
            ..defaults
        },
        CompileLimits {
            max_type_depth: 0,
            ..defaults
        },
        CompileLimits {
            max_functions: 0,
            ..defaults
        },
        CompileLimits {
            max_locals_per_function: 0,
            ..defaults
        },
        CompileLimits {
            max_parameters: 0,
            ..defaults
        },
        CompileLimits {
            max_record_fields: 0,
            ..defaults
        },
        CompileLimits {
            max_single_literal_bytes: 0,
            ..defaults
        },
        CompileLimits {
            max_total_literal_bytes: 0,
            ..defaults
        },
        CompileLimits {
            max_static_bytes: 0,
            ..defaults
        },
        CompileLimits {
            max_output_bytes: 0,
            ..defaults
        },
        CompileLimits {
            max_runtime_bytes: 0,
            ..defaults
        },
        CompileLimits {
            max_function_body_bytes: 0,
            ..defaults
        },
        CompileLimits {
            max_operators_per_function: 0,
            ..defaults
        },
        CompileLimits {
            max_ir_nodes: 0,
            ..defaults
        },
        CompileLimits {
            max_call_chain: 0,
            ..defaults
        },
        CompileLimits {
            compile_fuel: 0,
            ..defaults
        },
    ];
    for limits in zero_limits {
        let result = limits.clamped();
        if !cfg!(test) {
            assert!(result.is_err());
        } else {
            assert!(result.is_ok());
        }
    }
}
