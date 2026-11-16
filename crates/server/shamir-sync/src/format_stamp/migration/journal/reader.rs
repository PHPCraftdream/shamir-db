use std::path::Path;

use super::record::JournalRecord;

/// Parse complete LF-terminated records; only a damaged trailing record is ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalParseError {
    pub line: usize,
    pub message: String,
}

/// Parse journal bytes without filesystem access.
pub fn parse_records(bytes: &[u8]) -> Result<Vec<JournalRecord>, JournalParseError> {
    parse_records_with_prefix(bytes).map(|(records, _)| records)
}

pub(super) fn parse_records_with_prefix(
    bytes: &[u8],
) -> Result<(Vec<JournalRecord>, usize), JournalParseError> {
    if bytes.is_empty() {
        return Ok((Vec::new(), 0));
    }
    let mut lines: Vec<&[u8]> = bytes.split(|byte| *byte == b'\n').collect();
    if bytes.ends_with(b"\n") {
        lines.pop();
    }
    let last_index = lines.len().saturating_sub(1);
    let mut records = Vec::new();
    let mut accepted_bytes = 0;
    let mut offset = 0;
    for (index, line) in lines.into_iter().enumerate() {
        let terminated = index < last_index || bytes.ends_with(b"\n");
        let parsed = std::str::from_utf8(line)
            .map_err(|error| error.to_string())
            .and_then(|text| {
                serde_json::from_str::<JournalRecord>(text).map_err(|e| e.to_string())
            });
        match parsed {
            Ok(record) if index != last_index || terminated => {
                records.push(record);
                accepted_bytes = offset + line.len() + if terminated { 1 } else { 0 };
            }
            Ok(_) => {}
            Err(_) if index == last_index => {}
            Err(message) => {
                return Err(JournalParseError {
                    line: index + 1,
                    message,
                });
            }
        }
        offset += line.len() + 1;
    }
    Ok((records, accepted_bytes))
}

/// Read and parse `<dir>/format_migration.journal`; absence means no progress.
pub fn read(dir: &Path) -> Result<Vec<JournalRecord>, JournalParseError> {
    let path = dir.join(super::record::JOURNAL_FILE_NAME);
    match std::fs::read(path) {
        Ok(bytes) => parse_records(&bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(JournalParseError {
            line: 0,
            message: error.to_string(),
        }),
    }
}
