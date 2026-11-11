use crate::back::ir::{cell_tag, IrBuilder, IrError};
use crate::sem::{ReferenceKind, ReferenceMarker};

/// Bakes the static `$query` reference marker record and returns its cell address.
pub(crate) fn marker_cell(
    builder: &mut IrBuilder,
    alias: &str,
    path: &str,
) -> Result<u32, IrError> {
    let marker = marker_tuple(builder, alias, path)?;
    builder.static_cell(marker.0, marker.1, marker.2)
}

/// Bakes a static guard filter and returns its cell address.
pub(crate) fn guard_cell(
    builder: &mut IrBuilder,
    markers: &[ReferenceMarker],
) -> Result<u32, IrError> {
    if markers.is_empty() {
        return Err(IrError::Internal("lower: empty scalar reference guard"));
    }
    let mut atoms = Vec::with_capacity(markers.len().saturating_mul(3));
    for reference in markers {
        let kind = match reference.kind {
            ReferenceKind::String => "string",
            ReferenceKind::Bool => "bool",
            ReferenceKind::Bytes => "bytes",
            ReferenceKind::F64 => "f64",
            ReferenceKind::I32 | ReferenceKind::I64 => "int",
        };
        let is_i32 = matches!(reference.kind, ReferenceKind::I32);
        let marker = marker_tuple(builder, &reference.alias, &reference.path)?;
        let type_of = fn_value(builder, "value_nav/type_of", &[marker])?;
        let expected = string_value(builder, kind)?;
        atoms.push(value_compare(builder, type_of, "eq", expected)?);
        if is_i32 {
            atoms.push(value_compare(
                builder,
                marker,
                "gte",
                (cell_tag::I64, 0, (-2_147_483_648_i64) as u64),
            )?);
            atoms.push(value_compare(
                builder,
                marker,
                "lte",
                (cell_tag::I64, 0, 2_147_483_647_i64 as u64),
            )?);
        }
    }
    let guard = match atoms.as_slice() {
        [single] => *single,
        _ => {
            let filters = list_value(builder, &atoms)?;
            let op = string_value(builder, "and")?;
            record_value(builder, &["op", "filters"], &[op, filters])?
        }
    };
    builder.static_cell(guard.0, guard.1, guard.2)
}

fn marker_tuple(
    builder: &mut IrBuilder,
    alias: &str,
    path: &str,
) -> Result<(u32, u32, u64), IrError> {
    let normalized_alias = format!("@{}", alias.trim_start_matches('@'));
    let alias = string_value(builder, &normalized_alias)?;
    let path = string_value(builder, path)?;
    record_value(builder, &["$query", "path"], &[alias, path])
}

fn string_value(builder: &mut IrBuilder, value: &str) -> Result<(u32, u32, u64), IrError> {
    let (length, address) = builder.static_text(value.as_bytes())?;
    Ok((cell_tag::STRING, length, address))
}

fn record_value(
    builder: &mut IrBuilder,
    names: &[&str],
    fields: &[(u32, u32, u64)],
) -> Result<(u32, u32, u64), IrError> {
    let region = builder.static_region(fields)?;
    let field_count = u32::try_from(fields.len())
        .map_err(|_| IrError::Internal("lower: reference record field count overflow"))?;
    let names: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    let descriptor = builder.record_descriptor(&names)?;
    let payload = (u64::from(descriptor) << 32) | u64::from(region);
    Ok((cell_tag::RECORD, field_count, payload))
}

fn list_value(
    builder: &mut IrBuilder,
    items: &[(u32, u32, u64)],
) -> Result<(u32, u32, u64), IrError> {
    let region = builder.static_region(items)?;
    let count = u32::try_from(items.len())
        .map_err(|_| IrError::Internal("lower: reference guard atom count overflow"))?;
    Ok((cell_tag::LIST, count, u64::from(region)))
}

fn fn_value(
    builder: &mut IrBuilder,
    name: &str,
    args: &[(u32, u32, u64)],
) -> Result<(u32, u32, u64), IrError> {
    let name = string_value(builder, name)?;
    let args = list_value(builder, args)?;
    let call = record_value(builder, &["name", "args"], &[name, args])?;
    record_value(builder, &["$fn"], &[call])
}

fn value_compare(
    builder: &mut IrBuilder,
    left: (u32, u32, u64),
    cmp: &str,
    right: (u32, u32, u64),
) -> Result<(u32, u32, u64), IrError> {
    let op = string_value(builder, "value_compare")?;
    let cmp = string_value(builder, cmp)?;
    record_value(
        builder,
        &["op", "left", "cmp", "right"],
        &[op, left, cmp, right],
    )
}
