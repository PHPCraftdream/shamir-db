use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, bail, Result};
use shamir_server_security::user_directory::FjallUserDirectory;
use shamir_sync::replication_seed::{
    AclItem, IdentityItem, IdentityKind, LogicalValue, ResourceRef,
};
use shamir_types::types::value::QueryValue;

/// Allowlisted projections only. Never serialize catalogue rows or credentials.
pub(super) struct Projection {
    directory: Option<FjallUserDirectory>,
    pub(super) identities: BTreeMap<String, IdentityItem>,
    pub(super) acls: BTreeMap<Vec<String>, AclItem>,
}

impl Projection {
    pub(super) fn new(data_dir: &Path) -> Result<Self> {
        let path = data_dir.join("users");
        Ok(Self {
            directory: if path.is_dir() {
                Some(FjallUserDirectory::open(path)?)
            } else {
                None
            },
            identities: BTreeMap::new(),
            acls: BTreeMap::new(),
        })
    }

    pub(super) fn meta(
        &mut self,
        row: &QueryValue,
        resource: ResourceRef,
    ) -> Result<(Option<String>, Option<String>, u16)> {
        let owner = match row.get("owner") {
            None => None, // legacy open metadata has System ownership
            Some(QueryValue::Int(0)) => None,
            Some(QueryValue::Int(id)) if *id > 0 => {
                let user = self
                    .directory
                    .as_ref()
                    .and_then(|dir| dir.resolve_by_principal64(u64::try_from(*id).ok()?))
                    .ok_or_else(|| {
                        anyhow!("logical-export unresolved ACL owner; explicit mapping required")
                    })?;
                if self.identities.len() >= 4096 {
                    bail!("logical-export identity inventory exceeds scoped 4096 limit");
                }
                let name = user.username;
                if name.len() > 1024 {
                    bail!("logical-export principal name exceeds scoped 1024-byte limit");
                }
                self.identities.insert(
                    name.clone(),
                    IdentityItem {
                        logical_name: name.clone(),
                        kind: IdentityKind::User,
                        disabled_on_import: true,
                    },
                );
                Some(name)
            }
            _ => bail!("logical-export malformed ACL owner"),
        };
        if !matches!(row.get("group"), None | Some(QueryValue::Null)) {
            bail!("logical-export group ACLs and group membership are not implemented");
        }
        let mode = match row.get("mode") {
            None => 0o777,
            Some(QueryValue::Int(mode)) if (0..=0o777).contains(mode) => u16::try_from(*mode)?,
            _ => bail!("logical-export malformed ACL mode"),
        };
        let key = vec![
            resource.database.clone(),
            resource.repository.clone().unwrap_or_default(),
            resource.table.clone().unwrap_or_default(),
            resource.object.clone().unwrap_or_default(),
        ];
        if self.acls.len() >= 4096 {
            bail!("logical-export ACL inventory exceeds scoped 4096 limit");
        }
        if self
            .acls
            .insert(
                key,
                AclItem {
                    resource,
                    owner: owner.clone(),
                    group: None,
                    mode,
                    grants: vec![],
                },
            )
            .is_some()
        {
            bail!("logical-export duplicate ACL resource");
        }
        Ok((owner, None, mode))
    }
}

pub(super) fn fields(row: &QueryValue, allowed: &[&str], family: &str) -> Result<()> {
    let map = row
        .as_object()
        .ok_or_else(|| anyhow!("logical-export malformed {family}"))?;
    for key in map.keys() {
        if !allowed.contains(&key.as_str()) {
            bail!("logical-export unsupported {family} field '{key}'");
        }
    }
    Ok(())
}

pub(super) fn text<'a>(row: &'a QueryValue, key: &str) -> Result<&'a str> {
    let value = row
        .get(key)
        .and_then(QueryValue::as_str)
        .ok_or_else(|| anyhow!("logical-export missing/invalid catalogue field '{key}'"))?;
    if value.len() > 1024 {
        bail!("logical-export logical name exceeds scoped 1024-byte limit");
    }
    Ok(value)
}

pub(super) fn resource(
    db: &str,
    repo: Option<&str>,
    table: Option<&str>,
    object: Option<&str>,
) -> ResourceRef {
    ResourceRef {
        database: db.into(),
        repository: repo.map(Into::into),
        table: table.map(Into::into),
        object: object.map(Into::into),
    }
}

/// Persisted Str/Dec/Big and List/Set cannot be distinguished. Reject both
/// ambiguous carrier kinds, recursively, instead of claiming full fidelity.
/// Map order, binary, integer and IEEE-754 bits remain exact.
pub(super) fn record_value(value: &QueryValue) -> Result<LogicalValue> {
    let mut nodes = 0;
    let mut bytes = 0;
    record_budget(value, 0, &mut nodes, &mut bytes)?;
    convert_record(value)
}

fn record_budget(
    value: &QueryValue,
    depth: usize,
    nodes: &mut usize,
    bytes: &mut usize,
) -> Result<()> {
    *nodes += 1;
    if depth >= 64 || *nodes > 2048 || *bytes > 512 * 1024 {
        bail!("logical-export record exceeds scoped expansion budget (2048 nodes / 512 KiB / depth 64)");
    }
    match value {
        QueryValue::Map(map) => {
            for (key, child) in map {
                *bytes = bytes.saturating_add(key.len());
                record_budget(child, depth + 1, nodes, bytes)?;
            }
        }
        QueryValue::Bin(value) => {
            *bytes = bytes.saturating_add(value.len());
        }
        _ => {}
    }
    if *bytes > 512 * 1024 {
        bail!("logical-export record exceeds scoped 512 KiB expansion budget");
    }
    Ok(())
}

fn convert_record(value: &QueryValue) -> Result<LogicalValue> {
    Ok(match value {
        QueryValue::Null => LogicalValue::Null,
        QueryValue::Bool(v) => LogicalValue::Bool(*v),
        QueryValue::Int(v) => LogicalValue::Int(*v),
        QueryValue::F64(v) => LogicalValue::F64(v.to_bits()),
        QueryValue::Bin(v) => LogicalValue::Bin(v.clone()),
        QueryValue::Map(map) => LogicalValue::Map(map.iter().map(|(k, v)| Ok((k.clone(), convert_record(v)?))).collect::<Result<_>>()?),
        QueryValue::Str(_) | QueryValue::Dec(_) | QueryValue::Big(_) => bail!("logical-export cannot prove Str/Dec/Big fidelity from persisted storage; string-shaped record values are unsupported"),
        QueryValue::List(_) | QueryValue::Set(_) => bail!("logical-export cannot prove List/Set fidelity from persisted storage; array-shaped record values are unsupported"),
    })
}
