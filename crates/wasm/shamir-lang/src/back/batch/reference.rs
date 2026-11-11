use crate::back::ir::{cell_tag, IrBuilder, IrError};

/// Bakes the static `$query` reference marker record and returns its cell address.
pub(crate) fn marker_cell(
    builder: &mut IrBuilder,
    alias: &str,
    path: &str,
) -> Result<u32, IrError> {
    let normalized_alias = format!("@{}", alias.trim_start_matches('@'));
    let (alias_len, alias_address) = builder.static_text(normalized_alias.as_bytes())?;
    let (path_len, path_address) = builder.static_text(path.as_bytes())?;
    let region = builder.static_region(&[
        (cell_tag::STRING, alias_len, alias_address),
        (cell_tag::STRING, path_len, path_address),
    ])?;
    let descriptor = builder.record_descriptor(&[String::from("$query"), String::from("path")])?;
    let payload = (u64::from(descriptor) << 32) | u64::from(region);
    builder.static_cell(cell_tag::RECORD, 2, payload)
}
