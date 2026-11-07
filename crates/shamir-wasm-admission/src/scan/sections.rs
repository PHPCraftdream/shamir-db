use crate::abi::{ABI_SECTION, MAX_ORIGIN_BYTES, ORIGIN_SECTION, SUPPORTED_ABI_VERSION};
use crate::types::AdmissionError;
use wasmparser::CustomSectionReader;

/// Parsed `shamir.abi` / `shamir.origin` custom sections.
#[derive(Debug, Default)]
pub(crate) struct CustomSections {
    abi_version: Option<u32>,
    origin: Option<String>,
}

impl CustomSections {
    /// Inspects one custom section; sections with other names are ignored.
    pub(crate) fn observe(
        &mut self,
        reader: &CustomSectionReader<'_>,
    ) -> Result<(), AdmissionError> {
        match reader.name() {
            ABI_SECTION => self.observe_abi(reader.data()),
            ORIGIN_SECTION => self.observe_origin(reader.data()),
            _ => Ok(()),
        }
    }

    fn observe_abi(&mut self, data: &[u8]) -> Result<(), AdmissionError> {
        if self.abi_version.is_some() {
            return Err(AdmissionError::DuplicateAbiSection);
        }
        let raw: [u8; 4] = data
            .try_into()
            .map_err(|_| AdmissionError::AbiSectionLength {
                observed: data.len(),
            })?;
        let found = u32::from_le_bytes(raw);
        if found != SUPPORTED_ABI_VERSION {
            return Err(AdmissionError::UnsupportedAbiVersion {
                found,
                supported: SUPPORTED_ABI_VERSION,
            });
        }
        self.abi_version = Some(found);
        Ok(())
    }

    fn observe_origin(&mut self, data: &[u8]) -> Result<(), AdmissionError> {
        if self.origin.is_some() {
            return Err(AdmissionError::DuplicateOriginSection);
        }
        let text = std::str::from_utf8(data).map_err(|_| AdmissionError::OriginNotUtf8)?;
        if data.len() > MAX_ORIGIN_BYTES {
            return Err(AdmissionError::OriginTooLong {
                limit: MAX_ORIGIN_BYTES,
                observed: data.len(),
            });
        }
        self.origin = Some(text.to_string());
        Ok(())
    }

    /// The declared ABI version, or the supported one when absent.
    pub(crate) fn abi_version(&self) -> u32 {
        self.abi_version.unwrap_or(SUPPORTED_ABI_VERSION)
    }

    /// Consumes the state, yielding the origin claim.
    pub(crate) fn into_origin(self) -> Option<String> {
        self.origin
    }
}
