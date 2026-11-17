use sha2::{Digest, Sha256};

use crate::replication_seed::artifact::logical_dump::{
    codec::{encode_canonical, validate_item_section},
    content::{BindingKind, IdentityKind, PortableItem},
    header::LogicalDumpHeader,
    section::{SectionKind, SectionRecord},
    trailer::TrailerSection,
};
use crate::replication_seed::error::invalid;
use crate::replication_seed::SeedError;

#[derive(Clone)]
pub(crate) struct SectionState {
    pub kind: SectionKind,
    pub expected: Option<u64>,
    pub count: u64,
    digest: Sha256,
    last_order: Option<Vec<Vec<u8>>>,
}

pub(crate) struct DumpState {
    pub header: LogicalDumpHeader,
    pub bytes: u64,
    pub sequence: u64,
    pub preceding_frames: u64,
    pub items: u64,
    pub sections: u64,
    pub metadata_bytes: u64,
    pub prefix: Sha256,
    pub active: Option<SectionState>,
    pub next_section: usize,
    pub reports: Vec<TrailerSection>,
}

impl DumpState {
    pub fn new(header: LogicalDumpHeader, prefix: &[u8], header_bytes: usize) -> Self {
        let mut hash = Sha256::new();
        hash.update(prefix);
        Self {
            header,
            bytes: prefix.len() as u64,
            sequence: 0,
            preceding_frames: 0,
            items: 0,
            sections: 0,
            metadata_bytes: header_bytes as u64,
            prefix: hash,
            active: None,
            next_section: 0,
            reports: Vec::with_capacity(SectionKind::ORDERED.len()),
        }
    }

    pub fn current_section(&self) -> Option<SectionKind> {
        self.active.as_ref().map(|section| section.kind)
    }

    pub fn is_complete(&self) -> bool {
        self.next_section == SectionKind::ORDERED.len() && self.active.is_none()
    }

    pub fn next_kind(&self) -> Option<SectionKind> {
        SectionKind::ORDERED.get(self.next_section).copied()
    }

    pub fn metadata_remaining(&self) -> u64 {
        crate::replication_seed::codec::METADATA_LIMIT.saturating_sub(self.metadata_bytes)
    }

    pub fn charge_metadata(&mut self, bytes: usize) -> Result<(), SeedError> {
        self.metadata_bytes = self
            .metadata_bytes
            .checked_add(bytes as u64)
            .filter(|total| *total <= crate::replication_seed::codec::METADATA_LIMIT)
            .ok_or_else(|| invalid("logical-dump aggregate metadata limit exceeded"))?;
        Ok(())
    }

    pub fn start_section(
        &mut self,
        section: SectionKind,
        expected_items: Option<u64>,
    ) -> Result<(), SeedError> {
        if self.active.is_some() {
            return Err(invalid("a logical-dump section is already open"));
        }
        if section == SectionKind::End && expected_items != Some(0) {
            return Err(invalid("End section must declare zero items"));
        }
        if self.next_kind() != Some(section) {
            return Err(invalid("logical-dump section order is invalid"));
        }
        let inventory = &self.header.sections[self.next_section];
        if inventory.expected_items != expected_items {
            return Err(invalid("section start count differs from header inventory"));
        }
        self.active = Some(SectionState {
            kind: section,
            expected: expected_items,
            count: 0,
            digest: Sha256::new(),
            last_order: None,
        });
        Ok(())
    }

    pub fn add_item(&mut self, payload: &[u8], item: &PortableItem) -> Result<(), SeedError> {
        let section = self
            .active
            .as_ref()
            .ok_or_else(|| invalid("logical item outside a section"))?
            .kind;
        if section == SectionKind::End {
            return Err(invalid("End section must be empty"));
        }
        validate_item_section(section, item)?;
        if self.header.scope.schema_only && matches!(item, PortableItem::Record(_)) {
            return Err(invalid("schema-only dump cannot contain records"));
        }
        if matches!(item, PortableItem::Identity(identity) if !identity.disabled_on_import) {
            return Err(invalid("imported identities must be disabled"));
        }
        let order = item_order(item);
        let active = self.active.as_mut().expect("section checked above");
        if active
            .last_order
            .as_ref()
            .is_some_and(|last| last >= &order)
        {
            return Err(invalid("duplicate or out-of-order logical item"));
        }
        let count = active
            .count
            .checked_add(1)
            .ok_or_else(|| invalid("section item count overflow"))?;
        if active.expected.is_some_and(|expected| count > expected) {
            return Err(invalid("section contains more items than declared"));
        }
        active.last_order = Some(order);
        active.count = count;
        active.digest.update(payload);
        self.items = self
            .items
            .checked_add(1)
            .ok_or_else(|| invalid("logical-dump item count overflow"))?;
        Ok(())
    }

    pub fn section_digest(&self) -> Result<[u8; 32], SeedError> {
        let active = self
            .active
            .as_ref()
            .ok_or_else(|| invalid("no logical-dump section is open"))?;
        Ok(active.digest.clone().finalize().into())
    }

    pub fn close_section(
        &mut self,
        actual_items: u64,
        digest: [u8; 32],
    ) -> Result<TrailerSection, SeedError> {
        let active = self
            .active
            .take()
            .ok_or_else(|| invalid("no logical-dump section is open"))?;
        let calculated: [u8; 32] = active.digest.finalize().into();
        if actual_items != active.count {
            return Err(invalid(
                "section end count differs from observed item count",
            ));
        }
        if active
            .expected
            .is_some_and(|expected| expected != actual_items)
        {
            return Err(invalid("section end count differs from declared count"));
        }
        if digest != calculated {
            return Err(invalid("section item digest mismatch"));
        }
        if active.kind == SectionKind::End && actual_items != 0 {
            return Err(invalid("End section must be empty"));
        }
        let report = TrailerSection {
            section: active.kind,
            items: actual_items,
            digest,
        };
        self.reports.push(report.clone());
        self.sections += 1;
        self.next_section += 1;
        Ok(report)
    }

    pub fn ensure_frame_fits(&self, wire: &[u8]) -> Result<(), SeedError> {
        let _working_buffer_limit = crate::replication_seed::codec::CHUNK_LIMIT;
        self.bytes
            .checked_add(wire.len() as u64)
            .filter(|total| *total <= crate::replication_seed::codec::ARTIFACT_LIMIT)
            .ok_or_else(|| invalid("logical-dump artifact exceeds 1 TiB"))?;
        Ok(())
    }

    pub fn commit_frame(&mut self, wire: &[u8], trailer: bool) -> Result<(), SeedError> {
        self.bytes = self
            .bytes
            .checked_add(wire.len() as u64)
            .filter(|total| *total <= crate::replication_seed::codec::ARTIFACT_LIMIT)
            .ok_or_else(|| invalid("logical-dump artifact exceeds 1 TiB"))?;
        if !trailer {
            self.prefix.update(wire);
            self.preceding_frames = self
                .preceding_frames
                .checked_add(1)
                .ok_or_else(|| invalid("logical-dump frame count overflow"))?;
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| invalid("logical-dump frame sequence overflow"))?;
        Ok(())
    }

    pub fn prefix_digest(&self) -> [u8; 32] {
        self.prefix.clone().finalize().into()
    }
}

fn item_order(item: &PortableItem) -> Vec<Vec<u8>> {
    let text = |value: &str| value.as_bytes().to_vec();
    let tag = |value: &str| value.as_bytes().to_vec();
    match item {
        PortableItem::Identity(value) => vec![
            text(&value.logical_name),
            tag(match value.kind {
                IdentityKind::User => "user",
                IdentityKind::Group => "group",
            }),
        ],
        PortableItem::Database(value) => vec![text(&value.name)],
        PortableItem::Repository(value) => vec![text(&value.name)],
        PortableItem::Table(value) => vec![text(&value.repository), text(&value.name)],
        PortableItem::PortableSchema(value) => {
            vec![text(&value.repository), text(&value.table)]
        }
        PortableItem::Validator(value) => vec![text(&value.name), tag("validator")],
        PortableItem::Function(value) => vec![text(&value.name), tag("function")],
        PortableItem::GuestWasmModule(value) => vec![text(&value.module_name)],
        PortableItem::Binding(value) => vec![
            text(&value.repository),
            text(&value.table),
            text(&value.function),
            tag(match value.kind {
                BindingKind::Validator => "validator",
                BindingKind::Transform => "transform",
                BindingKind::ForeignKey => "foreign-key",
            }),
        ],
        PortableItem::IndexDescriptor(value) => vec![
            text(&value.repository),
            text(&value.table),
            text(&value.name),
        ],
        PortableItem::Acl(value) => vec![
            text(&value.resource.database),
            option_key(value.resource.repository.as_deref()),
            option_key(value.resource.table.as_deref()),
            option_key(value.resource.object.as_deref()),
        ],
        PortableItem::Record(value) => vec![
            text(&value.repository),
            text(&value.table),
            value.raw_key.clone(),
        ],
    }
}

fn option_key(value: Option<&str>) -> Vec<u8> {
    match value {
        None => vec![0],
        Some(value) => {
            let mut key = Vec::with_capacity(value.len() + 1);
            key.push(1);
            key.extend_from_slice(value.as_bytes());
            key
        }
    }
}

pub(crate) fn item_payload<T>(record: &SectionRecord<T>) -> Result<Vec<u8>, SeedError>
where
    T: serde::Serialize,
{
    encode_canonical(record)
}
