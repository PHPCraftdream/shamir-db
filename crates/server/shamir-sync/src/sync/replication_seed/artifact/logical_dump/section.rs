use serde::{Deserialize, Serialize};

/// Fixed section order from logical-dump design §3.1. `End` is the final
/// section marker; the artifact trailer is not a section and follows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SectionKind {
    Identities,
    Database,
    Repositories,
    Tables,
    PortableSchemas,
    ValidatorsAndFunctions,
    GuestWasmModules,
    Bindings,
    IndexDescriptors,
    Acls,
    Records,
    End,
}

impl SectionKind {
    pub const ORDERED: [Self; 12] = [
        Self::Identities,
        Self::Database,
        Self::Repositories,
        Self::Tables,
        Self::PortableSchemas,
        Self::ValidatorsAndFunctions,
        Self::GuestWasmModules,
        Self::Bindings,
        Self::IndexDescriptors,
        Self::Acls,
        Self::Records,
        Self::End,
    ];
}

/// Header inventory entry for one ordered section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionInventory {
    pub section: SectionKind,
    pub expected_items: Option<u64>,
}

/// Logical section frame record. The stage-2 trailer remains a distinct type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum SectionRecord<T> {
    Start {
        section: SectionKind,
        expected_items: Option<u64>,
    },
    Item {
        section: SectionKind,
        item: T,
    },
    End {
        section: SectionKind,
        actual_items: u64,
        digest: [u8; 32],
    },
}
