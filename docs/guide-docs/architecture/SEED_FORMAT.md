בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Seed artifact format (SEED-2/SEED-3, R2-lite)

Normative description of the on-disk artifact produced by
`replication-seed-export` and consumed by `replication-seed-verify` /
`replication-seed-install`, as implemented in
`crates/shamir-server/src/replication_seed/`. The structs of record are
`Manifest`, `TableRecord` and `FileRecord` in that module; this document
describes what is there, not aspirations.

## Layout

A seed artifact is a DIRECTORY (version 1):

```text
manifest.mpack            # written LAST; its SHA-256 is the artifact digest
schema.mpack              # declarative schema, paths as NAMES
bindings-<ordinal>.mpack  # field-id -> field-name pages (numeric ordinals)
tables/<ordinal>/rows-<ordinal>.mpack
```

* All files are MessagePack (`rmp_serde` named-field encoding).
* Table names and schema paths appear only as METADATA — the filesystem
  layout uses generated numeric ordinals, so no name is ever a path.
* The manifest is published last; export finalizes by same-filesystem
  publication of the completed directory.
* Every non-manifest file is listed in `manifest.files` with byte count
  and SHA-256; verification re-hashes and rejects unknown files.

## Manifest (`manifest.mpack`)

`Manifest` — all fields required (`deny_unknown_fields`):

| Field | Type | Meaning |
|---|---|---|
| `format_version` | u32 | artifact layout version (1) |
| `codec_version` | u32 | record frame codec version |
| `requirements` | Vec<String> | feature requirements the installer must satisfy |
| `source_db`, `source_repo` | String | exported `(db, repo)`; must equal the install destination names |
| `lineage_id` | [u8; 16] | durable source repository lineage (changes only when the repository is RECREATED) |
| `history_epoch` | u64 | durable source history epoch (advances when history is restored/rewound) |
| `version` | u64 | the settled source version `V` at export time; `V >= certified_gap.through` is enforced |
| `certified_gap` | Option<(u64, u64)> | the source's certified recovery interval `[first, through]` at export, if any |
| `schema_sha256` | String | canonical schema digest over `schema.mpack` content (names form); compared on every Resume |
| `artifact_id` | [u8; 16] | random artifact identifier |
| `index_policy` | String | always the excluded-indexes policy marker (followers run WITHOUT secondary indexes) |
| `tables` | Vec<TableRecord> | ordered table inventory (includes EMPTY tables) |
| `files` | Vec<FileRecord> | strict file inventory: path, bytes, sha256, owning table ordinal, row count |

The **artifact digest** is `SHA-256(manifest.mpack bytes)`. It is the
value printed by export, checked by verify/install via the operator's
trusted `--manifest-digest`, and stored in the installation receipt.
SHA-256 gives INTEGRITY relative to a trusted digest — not
authentication; the digest must travel out-of-band.

## Table record

`TableRecord` per table:

| Field | Meaning |
|---|---|
| `name` | table name (metadata, never a path) |
| `source_enable_indexes` / `follower_enable_indexes` | both exported as `false`: secondary indexes are excluded from seeds |
| `schema_version` | source schema version |
| `schema` | declarative schema with paths expressed as field NAMES (recompiled against the destination interner at install) |
| `excluded_indexes` | source index inventory, explicitly marked excluded, for operator visibility |
| `live_records` | live-record count at `V`, independently recomputed during export AND install |

## Records

Each `tables/<ordinal>/rows-<ordinal>.mpack` is a stream of length-prefixed
frames; a frame is a MessagePack pair `(key, payload)`:

* `key` — the record's EXACT raw key bytes (the source RecordId/opaque key
  is preserved byte-for-byte; no key translation).
* `payload` — the record value with source field-ID references; the
  field-id → name `bindings-*` pages let the installer translate to the
  destination interner, rejecting missing or conflicting bindings.

Value types are preserved (no round-trip through generic query values).
Dropped and aborted MVCC versions are not exported — only live state at
the settled version `V` from a bounded, key-ordered scan.

## What is deliberately NOT in an artifact

No credentials or server secrets, no TLS material, no audit logs, no
source WAL/journal, no source bookmarks or version markers beyond the
receipt fields above, no system repositories (refused), no secondary-index
definitions or state, no historical MVCC versions. The seed is STATE, not
a replayable event stream: the installer materializes all rows at
destination-local version 1, records a local bootstrap gap `[1,1]` with a
clean horizon, and never fabricates a replication event for the seed.

## Integrity bounds (installer, v1)

Maximum row frame 16 MiB, 64 MiB row chunks, 64 MiB aggregate
manifest/schema metadata, 1 TiB default artifact budget; duplicate record
keys, unknown files, unsafe paths, and unsupported codecs are refused.
Streaming hashing and streaming row import — no whole-table buffering.
