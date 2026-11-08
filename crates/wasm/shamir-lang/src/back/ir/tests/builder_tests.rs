use crate::api::CompileLimits;
use crate::back::ir::cell_tag;
use crate::back::ir::ArithOp;
use crate::back::ir::BlockKind;
use crate::back::ir::EqShape;
use crate::back::ir::InstId;
use crate::back::ir::InstKind;
use crate::back::ir::IrBuilder;
use crate::back::ir::IrError;
use crate::back::ir::IrLimits;
use crate::back::ir::Terminator;
use crate::back::ir::ValueKind;
use crate::front::CompileBudget;

fn generous_limits() -> IrLimits {
    IrLimits {
        max_ir_nodes: usize::MAX,
        max_ir_nodes_per_function: usize::MAX,
        max_single_literal_bytes: usize::MAX,
        max_total_literal_bytes: usize::MAX,
        max_static_bytes: usize::MAX,
    }
}

fn builder(limits: IrLimits) -> IrBuilder<'static> {
    // Leaky borrow keeps test ergonomics; the budget outlives the builder here.
    let budget: &'static mut CompileBudget = Box::leak(Box::new(budget()));
    IrBuilder::new(limits, budget)
}

fn budget() -> CompileBudget {
    let limits = CompileLimits::default();
    CompileBudget::new(&limits).expect("default limits normalize")
}

#[test]
fn builds_add_function_with_checked_arithmetic() {
    let mut builder = builder(generous_limits());
    builder
        .begin_function(
            vec![ValueKind::I64, ValueKind::I64],
            Some(ValueKind::I64),
            Vec::new(),
        )
        .expect("begin add");
    let block = builder.open_block(None).expect("entry block");
    assert_eq!(block, crate::back::ir::BlockId(0));
    let a = builder
        .append(InstKind::LocalGet(0), crate::api::Span::source_start())
        .expect("get a");
    let b = builder
        .append(InstKind::LocalGet(1), crate::api::Span::source_start())
        .expect("get b");
    let sum = builder
        .append(
            InstKind::CheckedArith {
                op: ArithOp::Add,
                width: ValueKind::I64,
                left: a,
                right: b,
            },
            crate::api::Span::source_start(),
        )
        .expect("add");
    builder.terminate_return(Some(sum)).expect("return");
    builder.finish_function().expect("finish add");
    let module = builder.finish().expect("module");
    assert_eq!(module.functions.len(), 1);
    assert_eq!(module.entry, 0);
    let function = &module.functions[0];
    assert_eq!(function.blocks.len(), 1);
    assert_eq!(function.blocks[0].kind, BlockKind::Plain);
    assert_eq!(function.blocks[0].inst_count, 3);
    assert_eq!(
        function.blocks[0].terminator,
        Terminator::Return(Some(InstId(2)))
    );
}

#[test]
fn append_fails_with_b001_when_ir_nodes_are_exhausted() {
    let mut builder = builder(IrLimits {
        max_ir_nodes: 2,
        ..generous_limits()
    });
    builder
        .begin_function(Vec::new(), None, Vec::new())
        .expect("begin f");
    builder.open_block(None).expect("entry block");
    builder
        .append(InstKind::ConstI32(1), crate::api::Span::source_start())
        .expect("node 1");
    builder
        .append(InstKind::ConstI32(2), crate::api::Span::source_start())
        .expect("node 2");
    let error = builder
        .append(InstKind::ConstI32(3), crate::api::Span::source_start())
        .expect_err("third node over the limit");
    match error {
        IrError::LimitExceeded { code, observed, .. } => {
            assert_eq!(code, "B001");
            assert_eq!(observed, 3);
        }
        other => panic!("expected LimitExceeded, got {:?}", other),
    }
}

#[test]
fn string_literal_lays_out_bytes_then_cell() {
    let mut builder = builder(generous_limits());
    let cell = builder.string_literal(b"hi".to_vec()).expect("literal");
    // Absolute address = STATIC_BASE (1088) + image offset 2 (two data bytes).
    assert_eq!(cell, 1090);
    let image = &builder.finish().expect("module").static_bytes;
    assert_eq!(image.len(), 2 + 16);
    assert_eq!(&image[0..2], b"hi");
    // Cell layout: tag le u32, aux le u32 (len), payload le u64 (absolute data address).
    assert_eq!(&image[2..6], &cell_tag::STRING.to_le_bytes());
    assert_eq!(&image[6..10], &2u32.to_le_bytes());
    // 1088 = STATIC_BASE, the absolute address of the data at image offset 0.
    assert_eq!(&image[10..18], &1088u64.to_le_bytes());
}

#[test]
fn null_cell_is_cached() {
    let mut builder = builder(generous_limits());
    let first = builder.null_cell().expect("null cell");
    let second = builder.null_cell().expect("null cell again");
    // Both calls hit the cache: same absolute address (STATIC_BASE = 1088).
    assert_eq!(first, 1088);
    assert_eq!(first, second);
    assert_eq!(builder.finish().expect("module").static_bytes.len(), 16);
}

#[test]
fn record_descriptor_matches_documented_layout() {
    let mut builder = builder(generous_limits());
    // Field name bytes go first: "a" at 0, "bc" at 1.
    let descriptor = builder
        .record_descriptor(&[String::from("a"), String::from("bc")])
        .expect("descriptor");
    // Absolute descriptor address = STATIC_BASE (1088) + image offset 3 ("a" + "bc").
    assert_eq!(descriptor, 1091);
    let image = builder.finish().expect("module").static_bytes;
    assert_eq!(&image[0..1], b"a");
    assert_eq!(&image[1..3], b"bc");
    // Descriptor at image offset 3: u32 count, then per field u32 len + u32 offset.
    assert_eq!(&image[3..7], &2u32.to_le_bytes());
    assert_eq!(&image[7..11], &1u32.to_le_bytes());
    // Name offsets are ABSOLUTE: 1088 = "a" (offset 0), 1089 = "bc" (offset 1).
    assert_eq!(&image[11..15], &1088u32.to_le_bytes());
    assert_eq!(&image[15..19], &2u32.to_le_bytes());
    assert_eq!(&image[19..23], &1089u32.to_le_bytes());
}

#[test]
fn eq_shape_dedups_structurally_identical_shapes() {
    let mut builder = builder(generous_limits());
    let inner = builder.eq_shape(EqShape::I32).expect("inner shape");
    let list_a = builder.eq_shape(EqShape::List(inner)).expect("list a");
    let inner_again = builder.eq_shape(EqShape::I32).expect("inner again");
    let list_b = builder
        .eq_shape(EqShape::List(inner_again))
        .expect("list b");
    assert_eq!(list_a, list_b);
    let record_a = builder
        .eq_shape(EqShape::Record(vec![inner, list_a]))
        .expect("record a");
    let record_b = builder
        .eq_shape(EqShape::Record(vec![inner, list_b]))
        .expect("record b");
    assert_eq!(record_a, record_b);
    let scalar = builder.eq_shape(EqShape::I32).expect("scalar dedup");
    assert_eq!(scalar, inner);
    assert_eq!(builder.finish().expect("module").eq_shapes.len(), 3);
}

#[test]
fn single_literal_cap_fires() {
    let mut builder = builder(IrLimits {
        max_single_literal_bytes: 4,
        ..generous_limits()
    });
    let error = builder
        .string_literal(b"toolong".to_vec())
        .expect_err("literal over the single cap");
    match error {
        IrError::LimitExceeded {
            code,
            observed,
            limit,
            ..
        } => {
            assert_eq!(code, "B001");
            assert_eq!(observed, 7);
            assert_eq!(limit, 4);
        }
        other => panic!("expected LimitExceeded, got {:?}", other),
    }
}
