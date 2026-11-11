use crate::api::{CompileLimits, Stage};
use crate::back::ir::{cell_tag, InstKind};
use crate::front::{self, CompileBudget};
use crate::sem;

const STATIC_BASE: u64 = 1_088;

#[test]
fn reference_fields_lower_to_static_marker_cells() {
    let source = r#"
@entry procedure p() -> i64 {
    const b0 = batch_read(batch(), "src", "users", { id: 1 });
    const values: { id: i64, label: string } = {
        id: query_ref_field(b0, "src", 0, "id"),
        label: "literal"
    };
    const b1 = batch_insert(b0, "created", "users", values);
    return 0;
}
"#;
    let limits = CompileLimits::default();
    let module = front::parse(source, &limits).expect("fixture source parses");
    let typed = sem::check(&module, &limits, Stage::Procedures).unwrap_or_else(|diagnostics| {
        panic!(
            "fixture checks: {:?}",
            diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        )
    });
    let mut budget = CompileBudget::new(&limits).expect("default limits form a budget");
    let ir =
        crate::back::lower::lower(&module, &typed, &limits, &mut budget).expect("fixture lowers");
    crate::back::ir::verify(&ir).expect("lowered fixture verifies");

    let static_base = STATIC_BASE as usize;
    let image = &ir.static_bytes;
    let descriptor_offset = image
        .windows(20)
        .enumerate()
        .find_map(|(offset, bytes)| {
            if u32::from_le_bytes(bytes[0..4].try_into().ok()?) != 2
                || u32::from_le_bytes(bytes[4..8].try_into().ok()?) != 6
                || u32::from_le_bytes(bytes[12..16].try_into().ok()?) != 4
            {
                return None;
            }
            let query_offset = usize::try_from(
                u32::from_le_bytes(bytes[8..12].try_into().ok()?)
                    .checked_sub(u32::try_from(STATIC_BASE).ok()?)?,
            )
            .ok()?;
            let path_offset = usize::try_from(
                u32::from_le_bytes(bytes[16..20].try_into().ok()?)
                    .checked_sub(u32::try_from(STATIC_BASE).ok()?)?,
            )
            .ok()?;
            (image.get(query_offset..query_offset + 6)? == b"$query"
                && image.get(path_offset..path_offset + 4)? == b"path")
                .then_some(offset)
        })
        .expect("descriptor names $query and path");
    let descriptor_address = STATIC_BASE + descriptor_offset as u64;

    let marker_cell_offset = image
        .windows(16)
        .enumerate()
        .find_map(|(offset, cell)| {
            let tag = u32::from_le_bytes(cell[0..4].try_into().ok()?);
            let aux = u32::from_le_bytes(cell[4..8].try_into().ok()?);
            let payload = u64::from_le_bytes(cell[8..16].try_into().ok()?);
            (tag == cell_tag::RECORD && aux == 2 && payload >> 32 == descriptor_address)
                .then_some(offset)
        })
        .expect("static marker record cell");
    let marker_address =
        u32::try_from(STATIC_BASE + marker_cell_offset as u64).expect("marker cell address");
    assert!(ir.functions.iter().any(|function| {
        function.insts.iter().any(
            |inst| matches!(inst.kind, InstKind::StaticCell(address) if address == marker_address),
        )
    }));

    let record_payload = u64::from_le_bytes(
        image[marker_cell_offset + 8..marker_cell_offset + 16]
            .try_into()
            .expect("record payload"),
    );
    let region_offset = (record_payload as u32 as usize)
        .checked_sub(static_base)
        .expect("region address is in static image");
    for (index, expected) in [b"@src".as_slice(), b"[0].id".as_slice()]
        .iter()
        .enumerate()
    {
        let cell_start = region_offset + index * 16;
        let cell = &image[cell_start..cell_start + 16];
        let tag = u32::from_le_bytes(cell[0..4].try_into().expect("string tag"));
        let length = u32::from_le_bytes(cell[4..8].try_into().expect("string length")) as usize;
        let address = u64::from_le_bytes(cell[8..16].try_into().expect("string address"));
        let text_offset = (address as usize)
            .checked_sub(static_base)
            .expect("string address is in static image");
        assert_eq!(tag, cell_tag::STRING);
        assert_eq!(&image[text_offset..text_offset + length], *expected);
    }
}
