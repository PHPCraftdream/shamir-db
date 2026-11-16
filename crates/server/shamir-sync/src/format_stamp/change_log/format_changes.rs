//! The ordered format change table (decision D12, design 4.3-4.4).

use super::format_change::FormatChange;

/// Ordered change log. First entry documents the `call_signature` change
/// absorbed into the base epoch (design 4.4); it is also the entry template.
pub const FORMAT_CHANGES: &[FormatChange] = &[FormatChange {
    id: "pre-release-absorbed",
    bumps_epoch: false,
    epoch_after: 1,
    components: &[
        ("wal", 1),
        ("storage", 1),
        ("system_catalogue", 1),
        ("index_meta", 1),
        ("vector_snapshot", 1),
        ("interner", 1),
        ("audit", 1),
    ],
    summary:
        "Function rows gained the `call_signature` field (checked calls); the change predates \
         the first release and is absorbed into the base epoch.",
    previous_binary_effect:
        "writes: a pre-signature binary replacing a `shamir`-language function row loses the \
         signature; later checked calls fail closed at the invocation gate (refusal, not \
         silent corruption).",
    reason:
        "Predates the first release: pre-release data is not protected and is moved by logical \
         export (design 3.6). Going forward the conservative rule applies: when in doubt, bump.",
    commit: "pre-release (absorbed into the base epoch, design 4.4)",
    migration_module: None,
}];
