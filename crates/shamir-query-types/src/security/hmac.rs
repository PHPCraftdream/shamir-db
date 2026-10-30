//! Canonical grammar is `PREFIX || tag:u16be || fields`: strings are
//! `len:u32be || UTF-8`, options are `0` or `1 || value`, lists are
//! `count:u32be || strings`; integers are big-endian and discriminants are
//! one byte. Conditional CreateFunction/CreateValidator operations are
//! encoded even when NotGated, allowing signing before tags are attached.
//!
//! | Operation | v2 tag |
//! | --- | --- |
//! | DropDb | 0x0001 |
//! | DropRepo | 0x0002 |
//! | DropTable | 0x0003 |
//! | DropIndex | 0x0004 |
//! | DropUser | 0x0005 |
//! | StartMigration | 0x0006 |
//! | CommitMigration | 0x0007 |
//! | RollbackMigration | 0x0008 |
//! | GrantRole | 0x0009 |
//! | RevokeRole | 0x000A |
//! | Chmod | 0x000B |
//! | Chown | 0x000C |
//! | Chgrp | 0x000D |
//! | CreateUser | 0x000E |
//! | SetRetention | 0x000F |
//! | PurgeHistory | 0x0010 |
//! | CreateGroup | 0x0011 |
//! | DropGroup | 0x0012 |
//! | RenameGroup | 0x0013 |
//! | AddGroupMember | 0x0014 |
//! | RemoveGroupMember | 0x0015 |
//! | CreateFunction | 0x0016 |
//! | SetSuperuser | 0x0017 |
//! | SetReplicator | 0x0018 |
//! | CreateScramUser | 0x0019 |
//! | DropFunction | 0x001A |
//! | DropValidator | 0x001B |
//! | CreateValidator | 0x001C |
//! | DropReplicationProfile | 0x001D |
//! | DropPublication | 0x001E |
//! | DropSubscription | 0x001F |

use crate::admin::{GroupRef, PurgeScope, ResourceRef};
use crate::batch::BatchOp;
use crate::batch::BatchOp::*;
use crate::wire::DbRequest;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

const PREFIX: &[u8] = b"shamir-db hmac canonical v2\0";
#[repr(u16)]
#[derive(Clone, Copy)]
enum OpTag {
    DropDb = 0x0001,
    DropRepo = 0x0002,
    DropTable = 0x0003,
    DropIndex = 0x0004,
    DropUser = 0x0005,
    StartMigration = 0x0006,
    CommitMigration = 0x0007,
    RollbackMigration = 0x0008,
    GrantRole = 0x0009,
    RevokeRole = 0x000A,
    Chmod = 0x000B,
    Chown = 0x000C,
    Chgrp = 0x000D,
    CreateUser = 0x000E,
    SetRetention = 0x000F,
    PurgeHistory = 0x0010,
    CreateGroup = 0x0011,
    DropGroup = 0x0012,
    RenameGroup = 0x0013,
    AddGroupMember = 0x0014,
    RemoveGroupMember = 0x0015,
    CreateFunction = 0x0016,
    SetSuperuser = 0x0017,
    SetReplicator = 0x0018,
    CreateScramUser = 0x0019,
    DropFunction = 0x001A,
    DropValidator = 0x001B,
    CreateValidator = 0x001C,
    DropReplicationProfile = 0x001D,
    DropPublication = 0x001E,
    DropSubscription = 0x001F,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalEncodingError {
    LengthOverflow,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalOpError {
    UnsupportedOperation,
    Encoding(CanonicalEncodingError),
}
impl From<CanonicalEncodingError> for CanonicalOpError {
    fn from(error: CanonicalEncodingError) -> Self {
        Self::Encoding(error)
    }
}

struct Encoder {
    bytes: Vec<u8>,
}
impl Encoder {
    fn new(tag: OpTag) -> Self {
        let mut bytes = PREFIX.to_vec();
        bytes.extend_from_slice(&(tag as u16).to_be_bytes());
        Self { bytes }
    }
    fn str(&mut self, s: &str) -> Result<(), CanonicalEncodingError> {
        let n = u32::try_from(s.len()).map_err(|_| CanonicalEncodingError::LengthOverflow)?;
        self.bytes.extend_from_slice(&n.to_be_bytes());
        self.bytes.extend_from_slice(s.as_bytes());
        Ok(())
    }
    fn u64(&mut self, n: u64) {
        self.bytes.extend_from_slice(&n.to_be_bytes());
    }
    fn u16(&mut self, n: u16) {
        self.bytes.extend_from_slice(&n.to_be_bytes());
    }
    fn bool(&mut self, b: bool) {
        self.bytes.push(u8::from(b));
    }
    fn opt<T>(
        &mut self,
        v: Option<T>,
        f: impl FnOnce(&mut Self, T) -> Result<(), CanonicalEncodingError>,
    ) -> Result<(), CanonicalEncodingError> {
        match v {
            None => {
                self.bytes.push(0);
                Ok(())
            }
            Some(v) => {
                self.bytes.push(1);
                f(self, v)
            }
        }
    }
    fn list(&mut self, v: &[String]) -> Result<(), CanonicalEncodingError> {
        let n = u32::try_from(v.len()).map_err(|_| CanonicalEncodingError::LengthOverflow)?;
        self.bytes.extend_from_slice(&n.to_be_bytes());
        for s in v {
            self.str(s)?;
        }
        Ok(())
    }
    fn finish(self) -> Result<Vec<u8>, CanonicalEncodingError> {
        Ok(self.bytes)
    }
    fn digest(&mut self, s: &str) {
        self.bytes.extend_from_slice(&Sha256::digest(s.as_bytes()));
    }
    fn resource(&mut self, r: &ResourceRef) -> Result<(), CanonicalEncodingError> {
        match r {
            ResourceRef::Database { database } => {
                self.bytes.push(0);
                self.str(database)?;
            }
            ResourceRef::Store { store: [a, b] } => {
                self.bytes.push(1);
                self.str(a)?;
                self.str(b)?;
            }
            ResourceRef::Table { table: [a, b, c] } => {
                self.bytes.push(2);
                self.str(a)?;
                self.str(b)?;
                self.str(c)?;
            }
            ResourceRef::Function { function } => {
                self.bytes.push(3);
                self.str(function)?;
            }
            ResourceRef::FunctionFolder { function_folder } => {
                self.bytes.push(4);
                self.list(function_folder)?;
            }
            ResourceRef::FunctionNamespace { .. } => self.bytes.push(5),
        }
        Ok(())
    }
    fn group(&mut self, r: &GroupRef) -> Result<(), CanonicalEncodingError> {
        match r {
            GroupRef::Name { name } => {
                self.bytes.push(0);
                self.str(name)?;
            }
            GroupRef::Id { id } => {
                self.bytes.push(1);
                self.u64(*id);
            }
        }
        Ok(())
    }
    fn scope(&mut self, s: &PurgeScope) -> Result<(), CanonicalEncodingError> {
        match s {
            PurgeScope::OlderThan { timestamp } => {
                self.bytes.push(0);
                self.u64(*timestamp);
            }
            PurgeScope::OlderThanAge { age_secs } => {
                self.bytes.push(1);
                self.u64(*age_secs);
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HmacPolicy {
    Required,
    NotGated,
}
/// Exhaustive destructive-op coverage policy. All other variants are explicitly not gated.
pub fn hmac_policy(op: &BatchOp) -> HmacPolicy {
    match op {
        DropDb(_)
        | DropRepo(_)
        | DropTable(_)
        | DropIndex(_)
        | DropUser(_)
        | StartMigration(_)
        | CommitMigration(_)
        | RollbackMigration(_)
        | GrantRole(_)
        | RevokeRole(_)
        | Chmod(_)
        | Chown(_)
        | Chgrp(_)
        | CreateUser(_)
        | SetRetention(_)
        | PurgeHistory(_)
        | CreateGroup(_)
        | DropGroup(_)
        | RenameGroup(_)
        | AddGroupMember(_)
        | RemoveGroupMember(_)
        | DropFunction(_)
        | DropValidator(_)
        | DropReplicationProfile(_)
        | DropPublication(_)
        | DropSubscription(_) => HmacPolicy::Required,
        CreateFunction(x)
            if x.security.as_deref() == Some("definer")
                || !x.secret_grants.is_empty()
                || x.replace =>
        {
            HmacPolicy::Required
        }
        CreateValidator(x) if x.replace => HmacPolicy::Required,
        Read(_)
        | Insert(_)
        | Update(_)
        | Set(_)
        | Delete(_)
        | CreateDb(_)
        | CreateRepo(_)
        | RenameRepo(_)
        | AlterRepo(_)
        | RenameDb(_)
        | CreateTable(_)
        | RenameTable(_)
        | CreateIndex(_)
        | RenameIndex(_)
        | SetBufferConfig(_)
        | GetBufferConfig(_)
        | AlterBufferConfig(_)
        | List(_)
        | MigrationStatus(_)
        | AccessTree(_)
        | CreateFunction(_)
        | RenameFunction(_)
        | CreateValidator(_)
        | RenameValidator(_)
        | BindValidator(_)
        | UnbindValidator(_)
        | ListValidators(_)
        | SetTableSchema(_)
        | AddSchemaRule(_)
        | RemoveSchemaRule(_)
        | GetTableSchema(_)
        | DescribeTable(_)
        | DescribeRepo(_)
        | CreateFunctionFolder(_)
        | RenameFunctionFolder(_)
        | InternerDump(_)
        | InternerTouch(_)
        | ChangesSince(_)
        | Call(_)
        | Batch(_)
        | ForEach(_)
        | Subscribe(_)
        | Unsubscribe(_)
        | CreateReplicationProfile(_)
        | CreatePublication(_)
        | CreateSubscription(_)
        | AlterSubscription(_)
        | ListPublications(_)
        | ListSubscriptions(_)
        | ReplicationStatus(_) => HmacPolicy::NotGated,
    }
}
/// Canonical bytes for a whole operation; every applicable DTO is destructured without `..`.
pub fn canonical_batch_op(db: &str, op: &BatchOp) -> Result<Vec<u8>, CanonicalOpError> {
    let mut e;
    match op {
        DropDb(x) => {
            let crate::admin::DropDbOp {
                drop_db,
                cascade,
                if_exists,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::DropDb);
            e.str(drop_db)?;
            e.bool(*cascade);
            e.bool(*if_exists);
        }
        DropRepo(x) => {
            let crate::admin::DropRepoOp {
                drop_repo,
                cascade,
                if_exists,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::DropRepo);
            e.str(db)?;
            e.str(drop_repo)?;
            e.bool(*cascade);
            e.bool(*if_exists);
        }
        DropTable(x) => {
            let crate::admin::DropTableOp {
                drop_table,
                repo,
                hmac: _,
                if_exists,
                cascade,
            } = x;
            e = Encoder::new(OpTag::DropTable);
            e.str(db)?;
            e.str(repo)?;
            e.str(drop_table)?;
            e.bool(*cascade);
            e.bool(*if_exists);
        }
        DropIndex(x) => {
            let crate::admin::DropIndexOp {
                drop_index,
                repo,
                table,
                if_exists,
                hmac: _,
                request_id: _,
            } = x;
            e = Encoder::new(OpTag::DropIndex);
            e.str(db)?;
            e.str(repo)?;
            e.str(table)?;
            e.str(drop_index)?;
            e.bool(*if_exists);
        }
        DropUser(x) => {
            let crate::auth::DropUserOp {
                drop_user,
                if_exists,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::DropUser);
            e.str(drop_user)?;
            e.bool(*if_exists);
        }
        StartMigration(x) => {
            let crate::admin::StartMigrationOp {
                start_migration,
                repo,
                dst_repo,
                dst_engine,
                dst_path,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::StartMigration);
            e.str(db)?;
            e.str(repo)?;
            e.str(start_migration)?;
            e.str(dst_repo)?;
            e.str(dst_engine)?;
            e.opt(dst_path.clone(), |e, v| e.str(&v))?;
        }
        CommitMigration(x) => {
            let crate::admin::CommitMigrationOp {
                commit_migration,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::CommitMigration);
            e.str(db)?;
            e.str(commit_migration)?;
        }
        RollbackMigration(x) => {
            let crate::admin::RollbackMigrationOp {
                rollback_migration,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::RollbackMigration);
            e.str(db)?;
            e.str(rollback_migration)?;
        }
        GrantRole(x) => {
            let crate::auth::GrantRoleOp {
                grant_role,
                hmac: _,
                user,
            } = x;
            e = Encoder::new(OpTag::GrantRole);
            e.str(grant_role)?;
            e.str(user)?;
        }
        RevokeRole(x) => {
            let crate::auth::RevokeRoleOp {
                revoke_role,
                hmac: _,
                user,
            } = x;
            e = Encoder::new(OpTag::RevokeRole);
            e.str(revoke_role)?;
            e.str(user)?;
        }
        Chmod(x) => {
            let crate::admin::ChmodOp {
                chmod,
                mode,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::Chmod);
            e.resource(chmod)?;
            e.u16(*mode);
        }
        Chown(x) => {
            let crate::admin::ChownOp {
                chown,
                owner,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::Chown);
            e.resource(chown)?;
            e.u64(*owner);
        }
        Chgrp(x) => {
            let crate::admin::ChgrpOp {
                chgrp,
                group,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::Chgrp);
            e.resource(chgrp)?;
            e.opt(*group, |e, v| {
                e.u64(v);
                Ok(())
            })?;
        }
        CreateUser(x) => {
            let crate::auth::CreateUserOp {
                create_user,
                password: _,
                roles,
                database,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::CreateUser);
            e.str(create_user)?;
            e.list(roles)?;
            e.opt(database.clone(), |e, v| e.str(&v))?;
        }
        SetRetention(x) => {
            let crate::admin::SetRetentionOp {
                set_retention,
                repo,
                retention,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::SetRetention);
            e.str(db)?;
            e.str(repo)?;
            e.str(set_retention)?;
            e.opt(retention.max_age_secs, |e, v| {
                e.u64(v);
                Ok(())
            })?;
            e.opt(retention.max_count, |e, v| {
                e.u64(v);
                Ok(())
            })?;
            e.opt(retention.min_count, |e, v| {
                e.u64(v);
                Ok(())
            })?;
        }
        PurgeHistory(x) => {
            let crate::admin::PurgeHistoryOp {
                purge_history,
                repo,
                scope,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::PurgeHistory);
            e.str(db)?;
            e.str(repo)?;
            e.str(purge_history)?;
            e.scope(scope)?;
        }
        CreateGroup(x) => {
            let crate::admin::CreateGroupOp {
                create_group,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::CreateGroup);
            e.str(create_group)?;
        }
        DropGroup(x) => {
            let crate::admin::DropGroupOp {
                drop_group,
                if_exists,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::DropGroup);
            e.group(drop_group)?;
            e.bool(*if_exists);
        }
        RenameGroup(x) => {
            let crate::admin::RenameGroupOp {
                rename_group,
                to,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::RenameGroup);
            e.group(rename_group)?;
            e.str(to)?;
        }
        AddGroupMember(x) => {
            let crate::admin::AddGroupMemberOp {
                add_group_member,
                user,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::AddGroupMember);
            e.group(add_group_member)?;
            e.u64(*user);
        }
        RemoveGroupMember(x) => {
            let crate::admin::RemoveGroupMemberOp {
                remove_group_member,
                user,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::RemoveGroupMember);
            e.group(remove_group_member)?;
            e.u64(*user);
        }
        CreateFunction(x) => {
            let crate::admin::CreateFunctionOp {
                create_function,
                source,
                wasm,
                replace,
                visibility,
                security,
                secret_grants,
                net_grants,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::CreateFunction);
            e.str(create_function)?;
            e.bool(*replace);
            e.str(security.as_deref().unwrap_or("invoker"))?;
            e.opt(visibility.clone(), |e, v| e.str(&v))?;
            e.list(secret_grants)?;
            e.list(net_grants)?;
            e.opt(source.as_deref(), |e, v| {
                e.digest(v);
                Ok(())
            })?;
            e.opt(wasm.as_deref(), |e, v| {
                e.digest(v);
                Ok(())
            })?;
        }
        CreateValidator(x) => {
            let crate::admin::CreateValidatorOp {
                create_validator,
                source,
                wasm,
                replace,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::CreateValidator);
            e.str(create_validator)?;
            e.bool(*replace);
            e.opt(source.as_deref(), |e, v| {
                e.digest(v);
                Ok(())
            })?;
            e.opt(wasm.as_deref(), |e, v| {
                e.digest(v);
                Ok(())
            })?;
        }
        DropFunction(x) => {
            let crate::admin::DropFunctionOp {
                drop_function,
                if_exists,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::DropFunction);
            e.str(drop_function)?;
            e.bool(*if_exists);
        }
        DropValidator(x) => {
            let crate::admin::DropValidatorOp {
                drop_validator,
                if_exists,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::DropValidator);
            e.str(drop_validator)?;
            e.bool(*if_exists);
        }
        DropReplicationProfile(x) => {
            let crate::admin::DropReplicationProfileOp {
                drop_replication_profile,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::DropReplicationProfile);
            e.str(drop_replication_profile)?;
        }
        DropPublication(x) => {
            let crate::admin::DropPublicationOp {
                drop_publication,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::DropPublication);
            e.str(drop_publication)?;
        }
        DropSubscription(x) => {
            let crate::admin::DropSubscriptionOp {
                drop_subscription,
                hmac: _,
            } = x;
            e = Encoder::new(OpTag::DropSubscription);
            e.str(drop_subscription)?;
        }
        _ => return Err(CanonicalOpError::UnsupportedOperation),
    }
    match e.finish() {
        Ok(bytes) => Ok(bytes),
        Err(err) => Err(CanonicalOpError::Encoding(err)),
    }
}
/// Canonical bytes for HMAC-bearing top-level requests; password is excluded.
pub fn canonical_db_request(r: &DbRequest) -> Result<Vec<u8>, CanonicalOpError> {
    let mut e;
    match r {
        DbRequest::CreateScramUser {
            name,
            password: _,
            roles,
            hmac: _,
        } => {
            e = Encoder::new(OpTag::CreateScramUser);
            e.str(name)?;
            e.list(roles)?;
        }
        DbRequest::SetSuperuser { user, on, hmac: _ } => {
            e = Encoder::new(OpTag::SetSuperuser);
            e.str(user)?;
            e.bool(*on)
        }
        DbRequest::SetReplicator { user, on, hmac: _ } => {
            e = Encoder::new(OpTag::SetReplicator);
            e.str(user)?;
            e.bool(*on)
        }
        _ => return Err(CanonicalOpError::UnsupportedOperation),
    }
    match e.finish() {
        Ok(bytes) => Ok(bytes),
        Err(err) => Err(CanonicalOpError::Encoding(err)),
    }
}
/// HMAC field accessor for exhaustive verifier dispatch.
pub fn supplied_hmac(op: &BatchOp) -> Option<&str> {
    match op {
        DropDb(x) => x.hmac.as_deref(),
        DropRepo(x) => x.hmac.as_deref(),
        DropTable(x) => x.hmac.as_deref(),
        DropIndex(x) => x.hmac.as_deref(),
        DropUser(x) => x.hmac.as_deref(),
        StartMigration(x) => x.hmac.as_deref(),
        CommitMigration(x) => x.hmac.as_deref(),
        RollbackMigration(x) => x.hmac.as_deref(),
        GrantRole(x) => x.hmac.as_deref(),
        RevokeRole(x) => x.hmac.as_deref(),
        Chmod(x) => x.hmac.as_deref(),
        Chown(x) => x.hmac.as_deref(),
        Chgrp(x) => x.hmac.as_deref(),
        CreateUser(x) => x.hmac.as_deref(),
        SetRetention(x) => x.hmac.as_deref(),
        PurgeHistory(x) => x.hmac.as_deref(),
        CreateGroup(x) => x.hmac.as_deref(),
        DropGroup(x) => x.hmac.as_deref(),
        RenameGroup(x) => x.hmac.as_deref(),
        AddGroupMember(x) => x.hmac.as_deref(),
        RemoveGroupMember(x) => x.hmac.as_deref(),
        CreateFunction(x) => x.hmac.as_deref(),
        CreateValidator(x) => x.hmac.as_deref(),
        DropFunction(x) => x.hmac.as_deref(),
        DropValidator(x) => x.hmac.as_deref(),
        DropReplicationProfile(x) => x.hmac.as_deref(),
        DropPublication(x) => x.hmac.as_deref(),
        DropSubscription(x) => x.hmac.as_deref(),
        _ => None,
    }
}
/// Unchanged session key derivation: SHA-256 domain separation over the session id.
pub fn derive_session_hmac_key(session_id: &[u8; 32]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"shamir-db hmac key v1\0");
    h.update(session_id);
    let out = h.finalize();
    let mut k = [0; 32];
    k.copy_from_slice(&out);
    k
}
pub fn compute_tag_hex(key: &[u8; 32], canonical: &[u8]) -> String {
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC accepts any key length");
    m.update(canonical);
    hex_encode(&m.finalize().into_bytes())
}
pub fn verify_tag_hex(key: &[u8; 32], canonical: &[u8], tag: &str) -> bool {
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC accepts any key length");
    m.update(canonical);
    let Ok(bytes) = hex_decode(tag) else {
        return false;
    };
    m.verify_slice(&bytes).is_ok()
}
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    const T: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(T[(b >> 4) as usize] as char);
        s.push(T[(b & 15) as usize] as char)
    }
    s
}
pub(crate) fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    if !s.len().is_multiple_of(2) {
        return Err(());
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    for p in s.as_bytes().chunks_exact(2) {
        out.push((nibble(p[0])? << 4) | nibble(p[1])?)
    }
    Ok(out)
}
fn nibble(c: u8) -> Result<u8, ()> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(()),
    }
}
