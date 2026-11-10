use super::contract::check_checked_call_import;
use crate::sanitizer::SANCTIONED_HOST_IMPORTS;
use crate::types::AdmissionError;
use wasmparser::{FuncType, ImportSectionReader, TypeRef};

/// Rejects any import outside [`SANCTIONED_HOST_IMPORTS`] or not a function.
///
/// When `func_type_indices` is given, the type index of every imported
/// function is appended (imports precede defined functions in the index space).
pub(super) fn scan_imports(
    reader: ImportSectionReader<'_>,
    mut func_type_indices: Option<&mut Vec<u32>>,
    types: &[FuncType],
) -> Result<(), AdmissionError> {
    for import in reader.into_imports() {
        let import = import.map_err(|e| AdmissionError::Parse(e.to_string()))?;
        let sanctioned = SANCTIONED_HOST_IMPORTS
            .iter()
            .any(|(m, n)| *m == import.module && *n == import.name);
        let type_index = match import.ty {
            TypeRef::Func(index) | TypeRef::FuncExact(index) => Some(index),
            TypeRef::Table(_) | TypeRef::Memory(_) | TypeRef::Global(_) | TypeRef::Tag(_) => None,
        };
        let (Some(index), true) = (type_index, sanctioned) else {
            return Err(AdmissionError::UnsanctionedImport {
                module: import.module.to_string(),
                name: import.name.to_string(),
                kind: kind_name(import.ty),
            });
        };
        if import.module == "shamir_host" && import.name == crate::abi::IMPORT_CALL_CHECKED {
            check_checked_call_import(index, types)?;
        }
        if let Some(indices) = func_type_indices.as_deref_mut() {
            indices.push(index);
        }
    }
    Ok(())
}

fn kind_name(ty: TypeRef) -> &'static str {
    match ty {
        TypeRef::Func(_) | TypeRef::FuncExact(_) => "func",
        TypeRef::Table(_) => "table",
        TypeRef::Memory(_) => "memory",
        TypeRef::Global(_) => "global",
        TypeRef::Tag(_) => "tag",
    }
}
