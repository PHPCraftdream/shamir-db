use std::io::Read;

use sha2::{Digest, Sha256};

use crate::replication_seed::artifact::logical_dump::{
    codec::validate_item_section,
    content::PortableItem,
    header::LogicalDumpHeader,
    section::{SectionKind, SectionRecord},
    trailer::Trailer,
};
use crate::replication_seed::error::invalid;
use crate::replication_seed::SeedError;

use super::frame::{read_counted, read_frame, MAGIC, SUPPORTED_FLAGS, VERSION};
use super::state::DumpState;

/// Forward-only reader. `next_item` verifies the final trailer and EOF before
/// returning `None`.
pub struct DumpReader<R: Read> {
    input: Option<R>,
    state: DumpState,
    pub finished: bool,
    terminal: bool,
}

impl<R: Read> DumpReader<R> {
    pub fn new(mut input: R) -> Result<Self, SeedError> {
        let mut prefix = Vec::with_capacity(48);
        let mut magic = [0u8; 8];
        contextual_read(&mut input, &mut magic, 0, 0, None)?;
        if &magic != MAGIC {
            return Err(at(0, 0, None, "invalid logical-dump magic"));
        }
        prefix.extend_from_slice(&magic);
        let mut version = [0u8; 2];
        contextual_read(&mut input, &mut version, 8, 0, None)?;
        let version = u16::from_be_bytes(version);
        if version != VERSION {
            return Err(at(8, 0, None, "unsupported logical-dump version"));
        }
        prefix.extend_from_slice(&version.to_be_bytes());
        let mut flags = [0u8; 2];
        contextual_read(&mut input, &mut flags, 10, 0, None)?;
        let flags = u16::from_be_bytes(flags);
        if flags != SUPPORTED_FLAGS {
            return Err(at(10, 0, None, "unsupported logical-dump flags"));
        }
        prefix.extend_from_slice(&flags.to_be_bytes());
        let mut header_length = [0u8; 4];
        contextual_read(&mut input, &mut header_length, 12, 0, None)?;
        let header_length = u32::from_be_bytes(header_length) as usize;
        if header_length == 0 || header_length > crate::replication_seed::codec::FRAME_LIMIT {
            return Err(at(
                12,
                0,
                None,
                "logical-dump header length is outside limits",
            ));
        }
        prefix.extend_from_slice(&(header_length as u32).to_be_bytes());
        let mut header_bytes = vec![0; header_length];
        contextual_read(&mut input, &mut header_bytes, 16, 0, None)?;
        prefix.extend_from_slice(&header_bytes);
        let mut received = [0u8; 32];
        contextual_read(
            &mut input,
            &mut received,
            16 + header_length as u64,
            0,
            None,
        )?;
        let calculated: [u8; 32] = Sha256::digest(&header_bytes).into();
        if received != calculated {
            return Err(at(
                16 + header_length as u64,
                0,
                None,
                "logical-dump header checksum mismatch",
            ));
        }
        prefix.extend_from_slice(&received);
        if prefix.len() as u64 > crate::replication_seed::codec::ARTIFACT_LIMIT {
            return Err(at(0, 0, None, "logical-dump artifact exceeds 1 TiB"));
        }
        if header_bytes.len() as u64 > crate::replication_seed::codec::METADATA_LIMIT {
            return Err(at(
                16,
                0,
                None,
                "logical-dump aggregate metadata limit exceeded",
            ));
        }
        let header = super::super::codec::decode_header(&header_bytes, &[])
            .map_err(|error| at(16, 0, None, &error.to_string()))?;
        let state = DumpState::new(header, &prefix, header_bytes.len());
        Ok(Self {
            input: Some(input),
            state,
            finished: false,
            terminal: false,
        })
    }

    pub fn open_file(
        path: impl AsRef<std::path::Path>,
    ) -> Result<DumpReader<std::fs::File>, SeedError> {
        let file = std::fs::File::open(path)?;
        super::verify::preflight_artifact_size(file.metadata()?.len())?;
        DumpReader::new(file)
    }

    pub fn header(&self) -> &LogicalDumpHeader {
        &self.state.header
    }

    pub fn next_item(&mut self) -> Result<Option<PortableItem>, SeedError> {
        if self.terminal {
            return Err(invalid(
                "logical-dump reader is terminal after an earlier failure",
            ));
        }
        if self.finished {
            return Ok(None);
        }
        let result = self.next_item_inner();
        if result.is_err() {
            self.terminal = true;
        }
        result
    }

    fn next_item_inner(&mut self) -> Result<Option<PortableItem>, SeedError> {
        loop {
            if self.state.is_complete() {
                self.read_trailer_and_eof()?;
                self.finished = true;
                return Ok(None);
            }
            let section_context = self
                .state
                .current_section()
                .or_else(|| self.state.next_kind());
            let sequence = self.state.sequence;
            let offset = self.state.bytes;
            let input = self.input.as_mut().expect("reader input is present");
            let frame = read_frame(input, offset, sequence)
                .map_err(|error| at(offset, sequence, section_context, &error.to_string()))?;
            if self.state.active.is_none() {
                let record: SectionRecord<PortableItem> =
                    super::super::codec::decode_canonical(&frame.payload).map_err(|error| {
                        at(offset, sequence, section_context, &error.to_string())
                    })?;
                match record {
                    SectionRecord::Start {
                        section,
                        expected_items,
                    } => {
                        self.state
                            .start_section(section, expected_items)
                            .map_err(|error| {
                                at(offset, sequence, Some(section), &error.to_string())
                            })?;
                    }
                    _ => {
                        return Err(at(
                            offset,
                            sequence,
                            section_context,
                            "expected logical-dump section start",
                        ));
                    }
                }
                self.state.charge_metadata(frame.payload.len())?;
                self.commit_read_frame(&frame.wire, false, offset, sequence, section_context)?;
                continue;
            }
            if self.state.current_section().is_some() {
                let record: SectionRecord<PortableItem> =
                    super::super::codec::decode_canonical(&frame.payload).map_err(|error| {
                        at(offset, sequence, section_context, &error.to_string())
                    })?;
                match record {
                    SectionRecord::Item { section, item } => {
                        if section != self.state.current_section().unwrap() {
                            return Err(at(
                                offset,
                                sequence,
                                section_context,
                                "item section mismatch",
                            ));
                        }
                        validate_item_section(section, &item).map_err(|error| {
                            at(offset, sequence, Some(section), &error.to_string())
                        })?;
                        self.state
                            .add_item(&frame.payload, &item)
                            .map_err(|error| {
                                at(offset, sequence, Some(section), &error.to_string())
                            })?;
                        if !matches!(item, PortableItem::Record(_)) {
                            self.state
                                .charge_metadata(frame.payload.len())
                                .map_err(|error| {
                                    at(offset, sequence, Some(section), &error.to_string())
                                })?;
                        }
                        self.commit_read_frame(
                            &frame.wire,
                            false,
                            offset,
                            sequence,
                            Some(section),
                        )?;
                        return Ok(Some(item));
                    }
                    SectionRecord::End {
                        section,
                        actual_items,
                        digest,
                    } => {
                        if section != self.state.current_section().unwrap() {
                            return Err(at(
                                offset,
                                sequence,
                                section_context,
                                "section end mismatch",
                            ));
                        }
                        self.state
                            .close_section(actual_items, digest)
                            .map_err(|error| {
                                at(offset, sequence, Some(section), &error.to_string())
                            })?;
                        self.state.charge_metadata(frame.payload.len())?;
                        self.commit_read_frame(
                            &frame.wire,
                            false,
                            offset,
                            sequence,
                            Some(section),
                        )?;
                    }
                    SectionRecord::Start { .. } => {
                        return Err(at(
                            offset,
                            sequence,
                            section_context,
                            "nested section start",
                        ));
                    }
                }
            }
        }
    }

    fn commit_read_frame(
        &mut self,
        wire: &[u8],
        trailer: bool,
        offset: u64,
        sequence: u64,
        section: Option<SectionKind>,
    ) -> Result<(), SeedError> {
        self.state
            .commit_frame(wire, trailer)
            .map_err(|error| at(offset, sequence, section, &error.to_string()))
    }

    fn read_trailer_and_eof(&mut self) -> Result<(), SeedError> {
        let offset = self.state.bytes;
        let sequence = self.state.sequence;
        let input = self.input.as_mut().expect("reader input is present");
        let frame = read_frame(input, offset, sequence)
            .map_err(|error| at(offset, sequence, Some(SectionKind::End), &error.to_string()))?;
        if frame.payload.len() as u64 > self.state.metadata_remaining() {
            return Err(at(
                offset,
                sequence,
                Some(SectionKind::End),
                "aggregate metadata limit exceeded",
            ));
        }
        let trailer: Trailer = super::super::codec::decode_canonical(&frame.payload)
            .map_err(|error| at(offset, sequence, Some(SectionKind::End), &error.to_string()))?;
        if trailer.preceding_frames != self.state.preceding_frames
            || trailer.preceding_items != self.state.items
            || trailer.preceding_sections != self.state.sections
            || trailer.preceding_bytes != self.state.bytes
            || trailer.prefix_sha256 != self.state.prefix_digest()
            || trailer.sections != self.state.reports
        {
            return Err(at(
                offset,
                sequence,
                Some(SectionKind::End),
                "logical-dump trailer totals or prefix digest mismatch",
            ));
        }
        let trailer_wire_len = frame.wire.len() as u64;
        self.state
            .commit_frame(&frame.wire, true)
            .map_err(|error| at(offset, sequence, Some(SectionKind::End), &error.to_string()))?;
        let mut trailing = [0u8; 1];
        let input = self.input.as_mut().expect("reader input is present");
        let read = input.read(&mut trailing).map_err(|error| {
            at(
                offset + trailer_wire_len,
                sequence + 1,
                Some(SectionKind::End),
                &error.to_string(),
            )
        })?;
        if read != 0 {
            return Err(at(
                offset + trailer_wire_len,
                sequence + 1,
                Some(SectionKind::End),
                "trailing bytes after logical-dump trailer",
            ));
        }
        Ok(())
    }

    pub(crate) fn report(&self) -> Result<super::verify::DumpReport, SeedError> {
        if !self.finished {
            return Err(invalid(
                "logical-dump report requested before verification completed",
            ));
        }
        Ok(super::verify::DumpReport {
            bytes: self.state.bytes,
            frames: self.state.sequence,
            items: self.state.items,
            sections: self.state.sections,
            header: self.state.header.clone(),
            prefix_sha256: self.state.prefix_digest(),
        })
    }
}

fn contextual_read<R: Read>(
    input: &mut R,
    bytes: &mut [u8],
    offset: u64,
    sequence: u64,
    section: Option<SectionKind>,
) -> Result<(), SeedError> {
    read_counted(input, bytes).map_err(|error| at(offset, sequence, section, &error.to_string()))
}

fn at(offset: u64, sequence: u64, section: Option<SectionKind>, message: &str) -> SeedError {
    invalid(format!(
        "logical dump byte offset {offset}, frame sequence {sequence}, section {section:?}: {message}"
    ))
}
