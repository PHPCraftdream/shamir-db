use crate::api::{CompileLimits, Stage};
use crate::back::batch::BatchStatics;
use crate::back::ir::{cell_tag, InstKind};
use crate::front::{self, CompileBudget};
use crate::sem;

const STATIC_BASE: usize = 1_088;

fn lower(source: &str) -> crate::back::ir::IrModule {
    let limits = CompileLimits::default();
    let parsed = front::parse(source, &limits).expect("source parses");
    let typed = sem::check(&parsed, &limits, Stage::Procedures).expect("source checks");
    let mut budget = CompileBudget::new(&limits).expect("default budget");
    let lowered =
        crate::back::lower::lower(&parsed, &typed, &limits, &mut budget).expect("source lowers");
    crate::back::ir::verify(&lowered).expect("lowered module verifies");
    lowered
}

fn has_text(image: &[u8], text: &[u8]) -> bool {
    image.windows(text.len()).any(|window| window == text)
}

fn descriptor_names(image: &[u8], expected: &[&[u8]]) -> bool {
    image.windows(4 + expected.len() * 8).any(|descriptor| {
        u32::from_le_bytes(descriptor[0..4].try_into().expect("count")) as usize == expected.len()
            && expected.iter().enumerate().all(|(index, name)| {
                let start = 4 + index * 8;
                let len = u32::from_le_bytes(
                    descriptor[start..start + 4]
                        .try_into()
                        .expect("name length"),
                ) as usize;
                let address = u32::from_le_bytes(
                    descriptor[start + 4..start + 8]
                        .try_into()
                        .expect("name address"),
                ) as usize;
                address
                    .checked_sub(STATIC_BASE)
                    .and_then(|offset| image.get(offset..offset + len))
                    == Some(*name)
            })
    })
}

#[test]
fn transactional_request_cell_sets_static_bool_true() {
    let mut budget = CompileBudget::new(&CompileLimits::default()).expect("default budget");
    let mut builder = crate::back::ir::IrBuilder::new(
        crate::back::ir::IrLimits {
            max_ir_nodes: usize::MAX,
            max_ir_nodes_per_function: usize::MAX,
            max_single_literal_bytes: usize::MAX,
            max_total_literal_bytes: usize::MAX,
            max_static_bytes: usize::MAX,
        },
        &mut budget,
    );
    let mut statics = BatchStatics::bake(&mut builder).expect("existing templates bake");
    let original_cell = statics.batch_cell;
    let request = statics
        .transactional_batch_cell(&mut builder)
        .expect("transactional request bakes");
    let image = builder.finish().expect("builder finishes").static_bytes;
    assert_ne!(request, original_cell);
    let offset = request as usize - STATIC_BASE;
    let cell = &image[offset..offset + 16];
    assert_eq!(
        u32::from_le_bytes(cell[0..4].try_into().expect("tag")),
        cell_tag::RECORD
    );
    let record_payload = u64::from_le_bytes(cell[8..16].try_into().expect("payload"));
    let request_region = record_payload as u32 as usize - STATIC_BASE;
    let transactional = &image[request_region + 16..request_region + 32];
    assert_eq!(
        u32::from_le_bytes(transactional[0..4].try_into().expect("bool tag")),
        cell_tag::BOOL
    );
    assert_eq!(
        u64::from_le_bytes(transactional[8..16].try_into().expect("bool payload")),
        1
    );
}

#[test]
fn only_transactional_guarded_read_insert_descriptors_add_abort_on_skip() {
    let transactional = lower(
        r#"
@entry procedure p() -> i64 {
    const b0 = batch_read(batch_tx(), "src", "users", { id: 1 });
    const plain = batch_read(b0, "plain", "users", { id: 2 });
    const readKey: { id: i64 } = { id: query_ref_field(plain, "src", 0, "id") };
    const b1 = batch_read(plain, "dependent", "users", readKey);
    const insertValue: { id: i64 } = { id: query_ref_field(plain, "src", 0, "id") };
    const b2 = batch_insert(b1, "created", "users", insertValue);
    let req = request(b2);
    return 0;
}
"#,
    );
    assert!(descriptor_names(
        &transactional.static_bytes,
        &[
            b"from",
            b"select",
            b"where",
            b"return_result",
            b"when",
            b"abort_on_skip"
        ]
    ));
    assert!(descriptor_names(
        &transactional.static_bytes,
        &[
            b"insert_into",
            b"values",
            b"return_result",
            b"when",
            b"abort_on_skip"
        ]
    ));

    let ordinary = lower(
        r#"
@entry procedure p() -> i64 {
    const b0 = batch_read(batch(), "src", "users", { id: 1 });
    const value: { id: i64 } = { id: query_ref_field(b0, "src", 0, "id") };
    const b1 = batch_insert(b0, "created", "users", value);
    let req = request(b1);
    return 0;
}
"#,
    );
    assert!(descriptor_names(
        &ordinary.static_bytes,
        &[b"from", b"select", b"where", b"return_result", b"when"]
    ));
    assert!(!has_text(&ordinary.static_bytes, b"abort_on_skip"));
    assert!(ordinary.functions.iter().any(|function| {
        function
            .insts
            .iter()
            .any(|inst| matches!(inst.kind, InstKind::BatchAppend { .. }))
    }));
}
