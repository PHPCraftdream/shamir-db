use super::scan::{admitted_validator, CustomSections, Structure};
use super::types::ModuleHash;
use super::types::{AdmissionError, AdmissionPolicy, ModuleReport};
use wasmparser::{Encoding, Error, FuncValidatorAllocations, Parser, Payload, ValidPayload};

fn validation(e: Error) -> AdmissionError {
    AdmissionError::Validation(e.to_string())
}

/// Admits a core WebAssembly module under `policy` in one `wasmparser` pass:
/// parse, feature-set validation, structural scan and (under
/// [`AdmissionPolicy::Admission`]) numeric caps plus the export contract.
///
/// Sections are validated before their caps are checked; function bodies are
/// scanned (caps, forbidden operators) before they are validated.
pub fn admit_module(
    bytes: &[u8],
    policy: &AdmissionPolicy,
) -> Result<ModuleReport, AdmissionError> {
    let limits = match policy {
        AdmissionPolicy::Admission(limits) => Some(limits),
        AdmissionPolicy::Load => None,
    };
    if let Some(limits) = limits {
        let limit = u64::try_from(limits.max_module_bytes).unwrap_or(u64::MAX);
        let observed = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if observed > limit {
            return Err(AdmissionError::ModuleTooLarge { limit, observed });
        }
    }
    let hash = ModuleHash::of(bytes);

    let mut validator = admitted_validator();
    let mut structure = Structure::new(limits);
    let mut sections = CustomSections::default();
    let mut allocations = FuncValidatorAllocations::default();
    let mut ended = false;

    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|e| AdmissionError::Parse(e.to_string()))?;
        // Rejected before the validator, which would otherwise start
        // validating a component.
        if let Payload::Version {
            encoding: Encoding::Component,
            ..
        } = payload
        {
            return Err(AdmissionError::ComponentEncoding);
        }
        match validator.payload(&payload).map_err(validation)? {
            ValidPayload::Ok => {}
            ValidPayload::End(_) => ended = true,
            ValidPayload::Parser(_) => {
                return Err(AdmissionError::Validation(
                    "nested modules are not permitted".to_string(),
                ));
            }
            ValidPayload::Func(to_validate, body) => {
                structure.on_function_body(to_validate.index, &body)?;
                let mut function_validator = to_validate.into_validator(allocations);
                function_validator.validate(&body).map_err(validation)?;
                allocations = function_validator.into_allocations();
                continue;
            }
        }
        match payload {
            Payload::TypeSection(reader) => structure.on_type_section(reader)?,
            Payload::ImportSection(reader) => structure.on_import_section(reader)?,
            Payload::FunctionSection(reader) => structure.on_function_section(reader)?,
            Payload::TableSection(reader) => structure.on_table_section(reader)?,
            Payload::MemorySection(reader) => structure.on_memory_section(reader)?,
            Payload::GlobalSection(reader) => structure.on_global_section(&reader)?,
            Payload::ExportSection(reader) => structure.on_export_section(reader)?,
            Payload::ElementSection(reader) => structure.on_element_section(reader)?,
            Payload::DataSection(reader) => structure.on_data_section(reader)?,
            Payload::CustomSection(reader) => sections.observe(&reader)?,
            _ => {}
        }
    }
    if !ended {
        return Err(AdmissionError::Parse(
            "unexpected end of module".to_string(),
        ));
    }

    structure.check_contract()?;
    Ok(structure.into_report(
        bytes.len(),
        hash,
        sections.abi_version(),
        sections.into_origin(),
    ))
}
