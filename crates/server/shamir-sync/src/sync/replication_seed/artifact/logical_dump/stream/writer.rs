use std::io::Write;

use sha2::Digest;

use crate::replication_seed::artifact::logical_dump::{
    content::PortableItem,
    header::LogicalDumpHeader,
    section::{SectionKind, SectionRecord},
    trailer::Trailer,
};
use crate::replication_seed::error::invalid;
use crate::replication_seed::SeedError;

use super::frame::{encode_frame, write_all, MAGIC, SUPPORTED_FLAGS, VERSION};
use super::state::{item_payload, DumpState};

/// Forward-only logical-dump writer. Any error is terminal because output may
/// already contain an incomplete frame.
pub struct DumpWriter<W: Write> {
    output: Option<W>,
    state: DumpState,
    terminal: bool,
}

fn write_item<W: Write>(
    state: &mut DumpState,
    output: &mut W,
    item: &PortableItem,
) -> Result<(), SeedError> {
    let section = state
        .current_section()
        .ok_or_else(|| invalid("logical item outside a section"))?;
    let record = SectionRecord::Item {
        section,
        item: item.clone(),
    };
    let payload = item_payload(&record)?;
    state.add_item(&payload, item)?;
    write_payload(
        state,
        output,
        &payload,
        !matches!(item, PortableItem::Record(_)),
    )
}

fn write_section_end<W: Write>(state: &mut DumpState, output: &mut W) -> Result<(), SeedError> {
    let section = state
        .current_section()
        .ok_or_else(|| invalid("no logical-dump section is open"))?;
    let count = state.active.as_ref().expect("section checked").count;
    let digest = state.section_digest()?;
    let record = SectionRecord::<PortableItem>::End {
        section,
        actual_items: count,
        digest,
    };
    let payload = item_payload(&record)?;
    state.close_section(count, digest)?;
    write_payload(state, output, &payload, true)
}

fn write_trailer<W: Write>(state: &mut DumpState, output: &mut W) -> Result<(), SeedError> {
    let trailer = Trailer {
        preceding_frames: state.preceding_frames,
        preceding_items: state.items,
        preceding_sections: state.sections,
        sections: state.reports.clone(),
        preceding_bytes: state.bytes,
        prefix_sha256: state.prefix_digest(),
    };
    let payload = super::super::codec::encode_canonical(&trailer)?;
    if payload.len() as u64 > state.metadata_remaining() {
        return Err(invalid("logical-dump aggregate metadata limit exceeded"));
    }
    let wire = encode_frame(state.sequence, &payload)?;
    state.ensure_frame_fits(&wire)?;
    write_all(output, &wire)?;
    state.commit_frame(&wire, true)
}

fn write_payload<W: Write>(
    state: &mut DumpState,
    output: &mut W,
    payload: &[u8],
    charge_metadata: bool,
) -> Result<(), SeedError> {
    if charge_metadata && payload.len() as u64 > state.metadata_remaining() {
        return Err(invalid("logical-dump aggregate metadata limit exceeded"));
    }
    let wire = encode_frame(state.sequence, payload)?;
    state.ensure_frame_fits(&wire)?;
    write_all(output, &wire)?;
    if charge_metadata {
        state.charge_metadata(payload.len())?;
    }
    state.commit_frame(&wire, false)
}

fn contextualize_result<T>(
    bytes: u64,
    sequence: u64,
    section: Option<SectionKind>,
    result: Result<T, SeedError>,
) -> Result<T, SeedError> {
    result.map_err(|error| {
        invalid(format!(
            "logical dump byte offset {bytes}, frame sequence {sequence}, section {section:?}: {error}"
        ))
    })
}

impl<W: Write> DumpWriter<W> {
    pub fn new(mut output: W, header: LogicalDumpHeader) -> Result<Self, SeedError> {
        header.validate(&[]).map_err(invalid)?;
        let header_bytes = super::super::codec::encode_header(&header, &[])?;
        if header_bytes.len() > crate::replication_seed::codec::FRAME_LIMIT {
            return Err(invalid("logical-dump header exceeds 16 MiB"));
        }
        let length = u32::try_from(header_bytes.len())
            .map_err(|_| invalid("logical-dump header length overflow"))?;
        let mut prefix = Vec::with_capacity(8 + 2 + 2 + 4 + header_bytes.len() + 32);
        prefix.extend_from_slice(MAGIC);
        prefix.extend_from_slice(&VERSION.to_be_bytes());
        prefix.extend_from_slice(&SUPPORTED_FLAGS.to_be_bytes());
        prefix.extend_from_slice(&length.to_be_bytes());
        prefix.extend_from_slice(&header_bytes);
        let digest: [u8; 32] = sha2::Sha256::digest(&header_bytes).into();
        prefix.extend_from_slice(&digest);
        if prefix.len() as u64 > crate::replication_seed::codec::ARTIFACT_LIMIT {
            return Err(invalid("logical-dump artifact exceeds 1 TiB"));
        }
        write_all(&mut output, &prefix)?;
        let state = DumpState::new(header, &prefix, header_bytes.len());
        Ok(Self {
            output: Some(output),
            state,
            terminal: false,
        })
    }

    pub fn start_section(&mut self, section: SectionKind) -> Result<(), SeedError> {
        let expected = self
            .state
            .header
            .sections
            .get(self.state.next_section)
            .map(|inventory| inventory.expected_items)
            .ok_or_else(|| invalid("all logical-dump sections are already complete"))?;
        self.start_section_with_count(section, expected)
    }

    pub fn start_section_with_count(
        &mut self,
        section: SectionKind,
        expected_items: Option<u64>,
    ) -> Result<(), SeedError> {
        self.ensure_live()?;
        let bytes = self.state.bytes;
        let sequence = self.state.sequence;
        let section_ctx = self.state.current_section();
        let result = {
            let output = self.output.as_mut().expect("writer output is present");
            self.state.start_section(section, expected_items)?;
            let payload =
                super::super::codec::encode_canonical(&SectionRecord::<PortableItem>::Start {
                    section,
                    expected_items,
                })?;
            write_payload(&mut self.state, output, &payload, true)
        };
        let result = contextualize_result(bytes, sequence, section_ctx, result);
        self.poison_on_error(&result);
        result
    }

    pub fn write_item(&mut self, item: &PortableItem) -> Result<(), SeedError> {
        self.ensure_live()?;
        let bytes = self.state.bytes;
        let sequence = self.state.sequence;
        let section = self.state.current_section();
        let result = {
            let output = self.output.as_mut().expect("writer output is present");
            write_item(&mut self.state, output, item)
        };
        let result = contextualize_result(bytes, sequence, section, result);
        self.poison_on_error(&result);
        result
    }

    pub fn end_section(&mut self) -> Result<(), SeedError> {
        self.ensure_live()?;
        let bytes = self.state.bytes;
        let sequence = self.state.sequence;
        let section = self.state.current_section();
        let result = {
            let output = self.output.as_mut().expect("writer output is present");
            write_section_end(&mut self.state, output)
        };
        let result = contextualize_result(bytes, sequence, section, result);
        self.poison_on_error(&result);
        result
    }

    pub fn finish(mut self) -> Result<W, SeedError> {
        self.ensure_live()?;
        if !self.state.is_complete() {
            self.terminal = true;
            return Err(invalid("cannot finish before all 12 sections end"));
        }
        let bytes = self.state.bytes;
        let sequence = self.state.sequence;
        let section = self.state.current_section();
        let result = {
            let output = self.output.as_mut().expect("writer output is present");
            write_trailer(&mut self.state, output)
        };
        let result = contextualize_result(bytes, sequence, section, result);
        self.poison_on_error(&result);
        result?;
        self.terminal = true;
        Ok(self.output.take().expect("writer output is present"))
    }

    fn ensure_live(&self) -> Result<(), SeedError> {
        if self.terminal {
            Err(invalid(
                "logical-dump writer is terminal after an earlier failure",
            ))
        } else {
            Ok(())
        }
    }

    fn poison_on_error(&mut self, result: &Result<(), SeedError>) {
        if result.is_err() {
            self.terminal = true;
        }
    }
}
