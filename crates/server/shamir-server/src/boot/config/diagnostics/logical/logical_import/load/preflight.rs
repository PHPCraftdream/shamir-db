//! Preflight: verify the whole artifact and cross-check the coherence the
//! verifier does not. No mutation. Runs the landed verifier over the whole
//! input, then a second streaming pass collects the definition inventory
//! later reused by load and validate.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use shamir_sync::replication_seed::{
    verify_dump_file, AclItem, DatabaseItem, DumpReader, DumpReport, IdentityItem, IdentityKind,
    LogicalDumpHeader, LogicalValue, PortableItem, RecordItem, RepositoryItem, SettingItem,
    SettingKey, TableItem,
};
use tokio_util::sync::CancellationToken;

use super::super::error::LogicalImportError;
use super::super::reader::CancelledRead;
use super::super::LogicalImportArgs;

/// Definition inventory collected during the preflight stream.
pub(crate) struct Preflight {
    pub(crate) report: DumpReport,
    pub(crate) database: DatabaseItem,
    pub(crate) repos: BTreeMap<String, RepositoryItem>,
    pub(crate) tables: BTreeMap<String, TableItem>,
    pub(crate) identities: BTreeMap<String, IdentityItem>,
    /// Settings are carried inside the DatabaseItem (no standalone section).
    pub(crate) settings: Vec<SettingItem>,
}

impl Preflight {
    /// Full verifier pass (magic, version, features, canonical encoding,
    /// limits, frame and section checksums, counts, trailer prefix digest,
    /// EOF), then a coherence pass the verifier does not perform. Typed
    /// refusals only; nothing is skipped or ignored.
    pub(crate) fn run(
        input: &Path,
        args: &LogicalImportArgs,
        cancel: &CancellationToken,
    ) -> Result<Self, LogicalImportError> {
        let report = verify_dump_file(input).map_err(|e| artifact(e, cancel))?;
        check_header(&report.header, args)?;
        let mut inventory = Inventory::default();
        let file = std::fs::File::open(input)?;
        let mut reader = DumpReader::new(CancelledRead::new(file, cancel.clone()))
            .map_err(|e| artifact(e, cancel))?;
        let mut records = 0u64;
        while let Some(item) = reader.next_item().map_err(|e| artifact(e, cancel))? {
            match item {
                PortableItem::Identity(identity) => inventory.identity(identity)?,
                PortableItem::Database(database) => inventory.database(database)?,
                PortableItem::Repository(repo) => inventory.repo(repo)?,
                PortableItem::Table(table) => inventory.table(table)?,
                PortableItem::Acl(acl) => inventory.acl(acl)?,
                PortableItem::Record(record) => {
                    records += 1;
                    inventory.record(&record)?;
                }
                PortableItem::PortableSchema(_)
                | PortableItem::Validator(_)
                | PortableItem::Function(_)
                | PortableItem::GuestWasmModule(_)
                | PortableItem::Binding(_)
                | PortableItem::IndexDescriptor(_) => {
                    return Err(LogicalImportError::Preflight(
                        "artifact carries schemas, validators, functions, guest modules, \
                         bindings or index descriptors (unsupported)"
                            .into(),
                    ));
                }
            }
        }
        if report.header.scope.schema_only && records > 0 {
            return Err(LogicalImportError::Preflight(
                "schema-only artifact carries record items".into(),
            ));
        }
        inventory.finish(report)
    }
}

#[derive(Default)]
struct Inventory {
    database: Option<DatabaseItem>,
    repos: BTreeMap<String, RepositoryItem>,
    tables: BTreeMap<String, TableItem>,
    identities: BTreeMap<String, IdentityItem>,
    acls: BTreeMap<Vec<String>, AclItem>,
}

impl Inventory {
    fn identity(&mut self, identity: IdentityItem) -> Result<(), LogicalImportError> {
        if self.identities.len() >= 4096 {
            return Err(refuse("identity inventory exceeds the scoped 4096 limit"));
        }
        if identity.kind != IdentityKind::User || !identity.disabled_on_import {
            return Err(refuse(format!(
                "identity '{}' is not a disabled user identity",
                identity.logical_name
            )));
        }
        if self
            .identities
            .insert(identity.logical_name.clone(), identity)
            .is_some()
        {
            return Err(refuse("duplicate identity in artifact"));
        }
        Ok(())
    }

    fn database(&mut self, database: DatabaseItem) -> Result<(), LogicalImportError> {
        if self.database.replace(database).is_some() {
            return Err(refuse("duplicate database item in artifact"));
        }
        Ok(())
    }

    fn repo(&mut self, repo: RepositoryItem) -> Result<(), LogicalImportError> {
        if repo.group.is_some() {
            return Err(refuse(format!(
                "repository '{}' has group ACL metadata (unsupported)",
                repo.name
            )));
        }
        if self.repos.insert(repo.name.clone(), repo).is_some() {
            return Err(refuse("duplicate repository in artifact"));
        }
        Ok(())
    }

    fn table(&mut self, table: TableItem) -> Result<(), LogicalImportError> {
        if table.group.is_some() || table.schema != LogicalValue::Null {
            return Err(refuse(format!(
                "table '{}/{}' has group ACL metadata or a schema (unsupported)",
                table.repository, table.name
            )));
        }
        let key = format!("{}/{}", table.repository, table.name);
        if self.tables.insert(key.clone(), table).is_some() {
            return Err(refuse("duplicate table in artifact"));
        }
        Ok(())
    }

    fn acl(&mut self, acl: AclItem) -> Result<(), LogicalImportError> {
        if self.acls.len() >= 4096 {
            return Err(refuse("ACL inventory exceeds the scoped 4096 limit"));
        }
        if !acl.grants.is_empty() {
            return Err(refuse("ACL carries grants (unsupported)"));
        }
        if acl.group.is_some() {
            return Err(refuse("ACL has group metadata (unsupported)"));
        }
        let key = vec![
            acl.resource.database.clone(),
            acl.resource.repository.clone().unwrap_or_default(),
            acl.resource.table.clone().unwrap_or_default(),
            acl.resource.object.clone().unwrap_or_default(),
        ];
        if self.acls.insert(key, acl).is_some() {
            return Err(refuse("duplicate ACL resource in artifact"));
        }
        Ok(())
    }

    fn record(&mut self, record: &RecordItem) -> Result<(), LogicalImportError> {
        if record.raw_key.len() != 16 {
            return Err(refuse(format!(
                "record key for '{}/{}' is {} bytes, not the 16-byte record id",
                record.repository,
                record.table,
                record.raw_key.len()
            )));
        }
        if !self
            .tables
            .contains_key(&format!("{}/{}", record.repository, record.table))
        {
            return Err(refuse(format!(
                "record refers to unexported table '{}/{}'",
                record.repository, record.table
            )));
        }
        Ok(())
    }

    /// Cross-section coherence: every table's repository is exported, every
    /// ownership/ACL owner name resolves to an exported identity, and the
    /// unsupported setting families are refused.
    fn finish(self, report: DumpReport) -> Result<Preflight, LogicalImportError> {
        let database = self
            .database
            .ok_or_else(|| refuse("artifact has no database item"))?;
        let settings = database.settings.clone();
        for table in self.tables.values() {
            if !self.repos.contains_key(&table.repository) {
                return Err(refuse(format!(
                    "table '{}/{}' refers to unexported repository",
                    table.repository, table.name
                )));
            }
        }
        let mut owners = BTreeSet::new();
        push_owner(&mut owners, database.owner.as_deref());
        for repo in self.repos.values() {
            push_owner(&mut owners, repo.owner.as_deref());
        }
        for table in self.tables.values() {
            push_owner(&mut owners, table.owner.as_deref());
        }
        for acl in self.acls.values() {
            push_owner(&mut owners, acl.owner.as_deref());
        }
        for owner in owners {
            if !self.identities.contains_key(&owner) {
                return Err(refuse(format!(
                    "ACL owner '{owner}' is not an exported identity"
                )));
            }
        }
        for setting in settings.iter() {
            if setting.key == SettingKey::FunctionNamespaceMeta {
                return Err(refuse(
                    "function namespace settings are unsupported (functions are refused)",
                ));
            }
        }
        Ok(Preflight {
            report,
            database,
            repos: self.repos,
            tables: self.tables,
            identities: self.identities,
            settings,
        })
    }
}

fn push_owner(owners: &mut BTreeSet<String>, owner: Option<&str>) {
    if let Some(owner) = owner {
        owners.insert(owner.to_owned());
    }
}

fn check_header(
    header: &LogicalDumpHeader,
    args: &LogicalImportArgs,
) -> Result<(), LogicalImportError> {
    if !header.required_features.is_empty() {
        return Err(refuse(format!(
            "artifact requires unsupported features: {:?}",
            header.required_features
        )));
    }
    if header.application_schema_version.is_some() {
        return Err(refuse(
            "artifact carries an application schema version (unsupported)",
        ));
    }
    if header.scope.include_artifacts {
        return Err(refuse(
            "artifact was exported with guest-artifact scope (unsupported)",
        ));
    }
    if header.scope.schema_only != args.schema_only {
        return Err(refuse(format!(
            "--schema-only={} does not match the artifact scope (schema_only={})",
            args.schema_only, header.scope.schema_only
        )));
    }
    Ok(())
}

fn refuse(reason: impl std::fmt::Display) -> LogicalImportError {
    LogicalImportError::Preflight(reason.to_string())
}

fn artifact(
    e: shamir_sync::replication_seed::SeedError,
    cancel: &CancellationToken,
) -> LogicalImportError {
    if cancel.is_cancelled() {
        LogicalImportError::Cancelled
    } else {
        LogicalImportError::Artifact(e)
    }
}
