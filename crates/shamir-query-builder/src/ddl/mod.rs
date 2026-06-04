//! Typed constructors for DDL / admin `BatchOp` variants.
//!
//! Every public function or builder in this module returns a
//! [`BatchOp`](shamir_query_types::batch::BatchOp) that can be fed
//! straight into `Batch::op(alias, ddl::create_db("mydb"))`.
//!
//! Where an operation has many or optional fields a builder struct is
//! returned instead; call `.build()` to finalize it into a `BatchOp`.
//!
//! Re-exports [`ResourceRef`] and [`GroupRef`] from `shamir-query-types`
//! so callers do not need an extra import. The [`res`] sub-module provides
//! tiny helpers to construct a `ResourceRef` without spelling out enum
//! variants.

use shamir_query_types::admin::{
    AccessTreeOp, AddGroupMemberOp, AlterBufferConfigOp, BindValidatorOp, BufferConfigDto,
    BufferConfigPatch, ChgrpOp, ChmodOp, ChownOp, CommitMigrationOp, CreateDbOp,
    CreateFunctionFolderOp, CreateFunctionOp, CreateGroupOp, CreateIndexOp, CreateRepoOp,
    CreateTableOp, CreateValidatorOp, DropDbOp, DropFunctionOp, DropGroupOp, DropIndexOp,
    DropRepoOp, DropTableOp, DropValidatorOp, GetBufferConfigOp, ListOp, ListValidatorsOp,
    MigrationStatusOp, RemoveGroupMemberOp, RenameFunctionOp, RenameValidatorOp,
    RollbackMigrationOp, SetBufferConfigOp, StartMigrationOp, UnbindValidatorOp,
};
use shamir_query_types::auth::{
    CreateRoleOp, CreateUserOp, DropRoleOp, DropUserOp, GrantRoleOp, Permission, RevokeRoleOp,
    SecretString,
};
use shamir_query_types::batch::BatchOp;
pub use shamir_query_types::WriteOp;

// Re-export wire types that callers need to assemble resource / group
// references and buffer configs.
pub use shamir_query_types::admin::{BufferConfigDto as BufConfig, BufferConfigPatch as BufPatch};
pub use shamir_query_types::admin::{GroupRef, ResourceRef};

// ============================================================================
// res — tiny ResourceRef constructors
// ============================================================================

/// Ergonomic helpers to build a [`ResourceRef`] without spelling out
/// enum variants.
pub mod res {
    use super::ResourceRef;

    /// Reference a database by name.
    pub fn database(name: impl Into<String>) -> ResourceRef {
        ResourceRef::Database {
            database: name.into(),
        }
    }

    /// Reference a store (repo) by `[db, store]`.
    pub fn store(db: impl Into<String>, store: impl Into<String>) -> ResourceRef {
        ResourceRef::Store {
            store: [db.into(), store.into()],
        }
    }

    /// Reference a table by `[db, store, table]`.
    pub fn table(
        db: impl Into<String>,
        store: impl Into<String>,
        table: impl Into<String>,
    ) -> ResourceRef {
        ResourceRef::Table {
            table: [db.into(), store.into(), table.into()],
        }
    }

    /// Reference a function by name.
    pub fn function(name: impl Into<String>) -> ResourceRef {
        ResourceRef::Function {
            function: name.into(),
        }
    }

    /// Reference the function namespace singleton.
    pub fn function_namespace() -> ResourceRef {
        ResourceRef::FunctionNamespace {
            function_namespace: true,
        }
    }
}

// ============================================================================
// Database DDL
// ============================================================================

/// Create a new database.
pub fn create_db(name: impl Into<String>) -> BatchOp {
    BatchOp::CreateDb(CreateDbOp {
        create_db: name.into(),
        if_not_exists: false,
    })
}

/// Drop a database. Optionally attach an HMAC tag.
pub fn drop_db(name: impl Into<String>) -> DropDb {
    DropDb {
        name: name.into(),
        hmac: None,
        cascade: false,
    }
}

/// Builder for [`DropDbOp`] (supports optional HMAC and cascade).
pub struct DropDb {
    name: String,
    hmac: Option<String>,
    cascade: bool,
}

impl DropDb {
    /// Attach the hex-encoded HMAC-SHA256 tag.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Enable cascade (drop all child repos and their tables).
    pub fn cascade(mut self) -> Self {
        self.cascade = true;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::DropDb(DropDbOp {
            drop_db: self.name,
            hmac: self.hmac,
            cascade: self.cascade,
        })
    }
}

impl From<DropDb> for BatchOp {
    fn from(b: DropDb) -> Self {
        b.build()
    }
}

// ============================================================================
// Repository DDL
// ============================================================================

/// Create a new repository. Returns a builder for optional fields.
pub fn create_repo(name: impl Into<String>) -> CreateRepo {
    CreateRepo {
        name: name.into(),
        engine: None,
        path: None,
        tables: Vec::new(),
        if_not_exists: false,
    }
}

/// Builder for [`CreateRepoOp`].
pub struct CreateRepo {
    name: String,
    engine: Option<String>,
    path: Option<String>,
    tables: Vec<String>,
    if_not_exists: bool,
}

impl CreateRepo {
    /// Set the storage engine (e.g. `"in_memory"`, `"redb"`, `"fjall"`).
    pub fn engine(mut self, engine: impl Into<String>) -> Self {
        self.engine = Some(engine.into());
        self
    }

    /// Set the data path.
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Pre-create these tables inside the repo.
    pub fn tables(mut self, tables: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tables = tables.into_iter().map(Into::into).collect();
        self
    }

    /// Skip error if the repo already exists.
    pub fn if_not_exists(mut self) -> Self {
        self.if_not_exists = true;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CreateRepo(CreateRepoOp {
            create_repo: self.name,
            engine: self.engine,
            path: self.path,
            tables: self.tables,
            if_not_exists: self.if_not_exists,
        })
    }
}

impl From<CreateRepo> for BatchOp {
    fn from(b: CreateRepo) -> Self {
        b.build()
    }
}

/// Drop a repository. Returns a builder for optional HMAC.
pub fn drop_repo(name: impl Into<String>) -> DropRepo {
    DropRepo {
        name: name.into(),
        hmac: None,
        cascade: false,
    }
}

/// Builder for [`DropRepoOp`].
pub struct DropRepo {
    name: String,
    hmac: Option<String>,
    cascade: bool,
}

impl DropRepo {
    /// Attach the hex-encoded HMAC tag.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Enable cascade (drop all child tables).
    pub fn cascade(mut self) -> Self {
        self.cascade = true;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::DropRepo(DropRepoOp {
            drop_repo: self.name,
            hmac: self.hmac,
            cascade: self.cascade,
        })
    }
}

impl From<DropRepo> for BatchOp {
    fn from(b: DropRepo) -> Self {
        b.build()
    }
}

// ============================================================================
// Table DDL
// ============================================================================

/// Create a table. Defaults to `repo = "main"`.
pub fn create_table(name: impl Into<String>) -> CreateTable {
    CreateTable {
        name: name.into(),
        repo: "main".to_owned(),
        if_not_exists: false,
    }
}

/// Builder for [`CreateTableOp`].
pub struct CreateTable {
    name: String,
    repo: String,
    if_not_exists: bool,
}

impl CreateTable {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Skip error if the table already exists.
    pub fn if_not_exists(mut self) -> Self {
        self.if_not_exists = true;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CreateTable(CreateTableOp {
            create_table: self.name,
            repo: self.repo,
            if_not_exists: self.if_not_exists,
        })
    }
}

impl From<CreateTable> for BatchOp {
    fn from(b: CreateTable) -> Self {
        b.build()
    }
}

/// Drop a table. Defaults to `repo = "main"`.
pub fn drop_table(name: impl Into<String>) -> DropTable {
    DropTable {
        name: name.into(),
        repo: "main".to_owned(),
        hmac: None,
    }
}

/// Builder for [`DropTableOp`].
pub struct DropTable {
    name: String,
    repo: String,
    hmac: Option<String>,
}

impl DropTable {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Attach the hex-encoded HMAC tag.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::DropTable(DropTableOp {
            drop_table: self.name,
            repo: self.repo,
            hmac: self.hmac,
        })
    }
}

impl From<DropTable> for BatchOp {
    fn from(b: DropTable) -> Self {
        b.build()
    }
}

// ============================================================================
// Index DDL
// ============================================================================

/// Create an index on a table. Returns a builder for the many optional
/// knobs (unique, sorted, FTS, vector, functional).
pub fn create_index(name: impl Into<String>, table: impl Into<String>) -> CreateIndex {
    CreateIndex {
        name: name.into(),
        table: table.into(),
        fields: Vec::new(),
        unique: false,
        sorted: false,
        repo: "main".to_owned(),
        index_type: None,
        fts_tokenizer: None,
        fts_language: None,
        functional_op: None,
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
        if_not_exists: false,
    }
}

/// Builder for [`CreateIndexOp`].
pub struct CreateIndex {
    name: String,
    table: String,
    fields: Vec<Vec<String>>,
    unique: bool,
    sorted: bool,
    repo: String,
    index_type: Option<String>,
    fts_tokenizer: Option<String>,
    fts_language: Option<String>,
    functional_op: Option<String>,
    functional_args: Option<Vec<serde_json::Value>>,
    vector_dim: Option<u32>,
    vector_metric: Option<String>,
    if_not_exists: bool,
}

impl CreateIndex {
    /// Set the indexed field paths.
    ///
    /// Each element is a path (e.g. `vec!["email"]` or
    /// `vec!["address", "city"]`).
    pub fn fields(mut self, fields: impl IntoIterator<Item = Vec<String>>) -> Self {
        self.fields = fields.into_iter().collect();
        self
    }

    /// Convenience: single-field index (most common case).
    pub fn field(mut self, field: impl Into<String>) -> Self {
        self.fields = vec![vec![field.into()]];
        self
    }

    /// Mark as a unique-constraint index.
    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    /// Mark as a sorted (value-ordered) index.
    pub fn sorted(mut self) -> Self {
        self.sorted = true;
        self
    }

    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Set the index type (`"btree"`, `"fts"`, `"functional"`, `"vector"`).
    pub fn index_type(mut self, t: impl Into<String>) -> Self {
        self.index_type = Some(t.into());
        self
    }

    /// Set the FTS tokenizer (`"whitespace"` or `"unicode"`).
    pub fn fts_tokenizer(mut self, tok: impl Into<String>) -> Self {
        self.fts_tokenizer = Some(tok.into());
        self
    }

    /// Set the FTS language hint.
    pub fn fts_language(mut self, lang: impl Into<String>) -> Self {
        self.fts_language = Some(lang.into());
        self
    }

    /// Set the functional index operator.
    pub fn functional_op(mut self, op: impl Into<String>) -> Self {
        self.functional_op = Some(op.into());
        self
    }

    /// Set the functional index arguments.
    pub fn functional_args(mut self, args: Vec<serde_json::Value>) -> Self {
        self.functional_args = Some(args);
        self
    }

    /// Set the vector dimension.
    pub fn vector_dim(mut self, dim: u32) -> Self {
        self.vector_dim = Some(dim);
        self
    }

    /// Set the vector metric (`"l2"`, `"cosine"`, `"dot"`).
    pub fn vector_metric(mut self, metric: impl Into<String>) -> Self {
        self.vector_metric = Some(metric.into());
        self
    }

    /// Skip error if the index already exists.
    pub fn if_not_exists(mut self) -> Self {
        self.if_not_exists = true;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CreateIndex(CreateIndexOp {
            create_index: self.name,
            table: self.table,
            fields: self.fields,
            unique: self.unique,
            sorted: self.sorted,
            repo: self.repo,
            index_type: self.index_type,
            fts_tokenizer: self.fts_tokenizer,
            fts_language: self.fts_language,
            functional_op: self.functional_op,
            functional_args: self.functional_args,
            vector_dim: self.vector_dim,
            vector_metric: self.vector_metric,
            if_not_exists: self.if_not_exists,
        })
    }
}

impl From<CreateIndex> for BatchOp {
    fn from(b: CreateIndex) -> Self {
        b.build()
    }
}

/// Drop an index from a table. Returns a builder for optional fields.
pub fn drop_index(name: impl Into<String>, table: impl Into<String>) -> DropIndex {
    DropIndex {
        name: name.into(),
        table: table.into(),
        unique: false,
        repo: "main".to_owned(),
        hmac: None,
    }
}

/// Builder for [`DropIndexOp`].
pub struct DropIndex {
    name: String,
    table: String,
    unique: bool,
    repo: String,
    hmac: Option<String>,
}

impl DropIndex {
    /// Mark that the index being dropped is a unique index.
    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Attach the hex-encoded HMAC tag.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::DropIndex(DropIndexOp {
            drop_index: self.name,
            table: self.table,
            unique: self.unique,
            repo: self.repo,
            hmac: self.hmac,
        })
    }
}

impl From<DropIndex> for BatchOp {
    fn from(b: DropIndex) -> Self {
        b.build()
    }
}

// ============================================================================
// Buffer config DDL
// ============================================================================

/// Set the full buffer config for a table. `repo` defaults to `"main"`.
pub fn set_buffer_config(table: impl Into<String>, config: BufferConfigDto) -> SetBufferConfig {
    SetBufferConfig {
        table: table.into(),
        repo: "main".to_owned(),
        config,
    }
}

/// Builder for [`SetBufferConfigOp`].
pub struct SetBufferConfig {
    table: String,
    repo: String,
    config: BufferConfigDto,
}

impl SetBufferConfig {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::SetBufferConfig(SetBufferConfigOp {
            set_buffer_config: self.table,
            repo: self.repo,
            config: self.config,
        })
    }
}

impl From<SetBufferConfig> for BatchOp {
    fn from(b: SetBufferConfig) -> Self {
        b.build()
    }
}

/// Get the buffer config for a table. `repo` defaults to `"main"`.
pub fn get_buffer_config(table: impl Into<String>) -> GetBufferConfig {
    GetBufferConfig {
        table: table.into(),
        repo: "main".to_owned(),
    }
}

/// Builder for [`GetBufferConfigOp`].
pub struct GetBufferConfig {
    table: String,
    repo: String,
}

impl GetBufferConfig {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::GetBufferConfig(GetBufferConfigOp {
            get_buffer_config: self.table,
            repo: self.repo,
        })
    }
}

impl From<GetBufferConfig> for BatchOp {
    fn from(b: GetBufferConfig) -> Self {
        b.build()
    }
}

/// Partially alter buffer config for a table. `repo` defaults to `"main"`.
pub fn alter_buffer_config(table: impl Into<String>, patch: BufferConfigPatch) -> AlterBufferCfg {
    AlterBufferCfg {
        table: table.into(),
        repo: "main".to_owned(),
        patch,
    }
}

/// Builder for [`AlterBufferConfigOp`].
pub struct AlterBufferCfg {
    table: String,
    repo: String,
    patch: BufferConfigPatch,
}

impl AlterBufferCfg {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::AlterBufferConfig(AlterBufferConfigOp {
            alter_buffer_config: self.table,
            repo: self.repo,
            patch: self.patch,
        })
    }
}

impl From<AlterBufferCfg> for BatchOp {
    fn from(b: AlterBufferCfg) -> Self {
        b.build()
    }
}

// ============================================================================
// List operations
// ============================================================================

/// List databases.
pub fn list_databases() -> BatchOp {
    BatchOp::List(ListOp::Databases)
}

/// List repos in the current database.
pub fn list_repos() -> BatchOp {
    BatchOp::List(ListOp::Repos)
}

/// List tables in a repo. `repo` defaults to `"main"`.
pub fn list_tables() -> ListTables {
    ListTables {
        repo: "main".to_owned(),
    }
}

/// Builder for the `list tables` variant of [`ListOp`].
pub struct ListTables {
    repo: String,
}

impl ListTables {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::List(ListOp::Tables { repo: self.repo })
    }
}

impl From<ListTables> for BatchOp {
    fn from(b: ListTables) -> Self {
        b.build()
    }
}

/// List indexes on a table. `repo` defaults to `"main"`.
pub fn list_indexes(table: impl Into<String>) -> ListIndexes {
    ListIndexes {
        table: table.into(),
        repo: "main".to_owned(),
    }
}

/// Builder for the `list indexes` variant of [`ListOp`].
pub struct ListIndexes {
    table: String,
    repo: String,
}

impl ListIndexes {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::List(ListOp::Indexes {
            table: self.table,
            repo: self.repo,
        })
    }
}

impl From<ListIndexes> for BatchOp {
    fn from(b: ListIndexes) -> Self {
        b.build()
    }
}

/// List users.
pub fn list_users() -> BatchOp {
    BatchOp::List(ListOp::Users)
}

/// List roles.
pub fn list_roles() -> BatchOp {
    BatchOp::List(ListOp::Roles)
}

// ============================================================================
// Migration DDL
// ============================================================================

/// Start an online table migration. Returns a builder for optional fields.
pub fn start_migration(
    table: impl Into<String>,
    dst_repo: impl Into<String>,
    dst_engine: impl Into<String>,
) -> StartMigration {
    StartMigration {
        table: table.into(),
        repo: "main".to_owned(),
        dst_repo: dst_repo.into(),
        dst_engine: dst_engine.into(),
        dst_path: None,
        hmac: None,
    }
}

/// Builder for [`StartMigrationOp`].
pub struct StartMigration {
    table: String,
    repo: String,
    dst_repo: String,
    dst_engine: String,
    dst_path: Option<String>,
    hmac: Option<String>,
}

impl StartMigration {
    /// Override the source repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Set the destination data path.
    pub fn dst_path(mut self, path: impl Into<String>) -> Self {
        self.dst_path = Some(path.into());
        self
    }

    /// Attach the hex-encoded HMAC tag.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::StartMigration(StartMigrationOp {
            start_migration: self.table,
            repo: self.repo,
            dst_repo: self.dst_repo,
            dst_engine: self.dst_engine,
            dst_path: self.dst_path,
            hmac: self.hmac,
        })
    }
}

impl From<StartMigration> for BatchOp {
    fn from(b: StartMigration) -> Self {
        b.build()
    }
}

/// Commit a running migration by ID.
pub fn commit_migration(id: impl Into<String>) -> CommitMig {
    CommitMig {
        id: id.into(),
        hmac: None,
    }
}

/// Builder for [`CommitMigrationOp`].
pub struct CommitMig {
    id: String,
    hmac: Option<String>,
}

impl CommitMig {
    /// Attach the hex-encoded HMAC tag.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CommitMigration(CommitMigrationOp {
            commit_migration: self.id,
            hmac: self.hmac,
        })
    }
}

impl From<CommitMig> for BatchOp {
    fn from(b: CommitMig) -> Self {
        b.build()
    }
}

/// Rollback a running migration by ID.
pub fn rollback_migration(id: impl Into<String>) -> RollbackMig {
    RollbackMig {
        id: id.into(),
        hmac: None,
    }
}

/// Builder for [`RollbackMigrationOp`].
pub struct RollbackMig {
    id: String,
    hmac: Option<String>,
}

impl RollbackMig {
    /// Attach the hex-encoded HMAC tag.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::RollbackMigration(RollbackMigrationOp {
            rollback_migration: self.id,
            hmac: self.hmac,
        })
    }
}

impl From<RollbackMig> for BatchOp {
    fn from(b: RollbackMig) -> Self {
        b.build()
    }
}

/// Query the status of a migration by ID.
pub fn migration_status(id: impl Into<String>) -> BatchOp {
    BatchOp::MigrationStatus(MigrationStatusOp {
        migration_status: id.into(),
    })
}

// ============================================================================
// Access-tree introspection
// ============================================================================

/// Request the access-control tree. Returns a builder for optional depth/db.
pub fn access_tree() -> AccessTree {
    AccessTree {
        depth: None,
        db: None,
    }
}

/// Builder for [`AccessTreeOp`].
pub struct AccessTree {
    depth: Option<u32>,
    db: Option<String>,
}

impl AccessTree {
    /// Cap the resource hierarchy depth.
    pub fn depth(mut self, depth: u32) -> Self {
        self.depth = Some(depth);
        self
    }

    /// Restrict the tree to a single database.
    pub fn db(mut self, db: impl Into<String>) -> Self {
        self.db = Some(db.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::AccessTree(AccessTreeOp {
            access_tree: true,
            depth: self.depth,
            db: self.db,
        })
    }
}

impl From<AccessTree> for BatchOp {
    fn from(b: AccessTree) -> Self {
        b.build()
    }
}

// ============================================================================
// Access-control DDL (chmod / chown / chgrp)
// ============================================================================

/// Change mode bits on a resource.
pub fn chmod(resource: ResourceRef, mode: u16) -> BatchOp {
    BatchOp::Chmod(ChmodOp {
        chmod: resource,
        mode,
    })
}

/// Change owner on a resource.
pub fn chown(resource: ResourceRef, owner: u64) -> BatchOp {
    BatchOp::Chown(ChownOp {
        chown: resource,
        owner,
    })
}

/// Change group on a resource. Pass `None` to clear the group.
pub fn chgrp(resource: ResourceRef, group: Option<u64>) -> BatchOp {
    BatchOp::Chgrp(ChgrpOp {
        chgrp: resource,
        group,
    })
}

// ============================================================================
// Group DDL
// ============================================================================

/// Create a new group.
pub fn create_group(name: impl Into<String>) -> BatchOp {
    BatchOp::CreateGroup(CreateGroupOp {
        create_group: name.into(),
    })
}

/// Drop a group by reference (name or id).
pub fn drop_group(group: GroupRef) -> BatchOp {
    BatchOp::DropGroup(DropGroupOp { drop_group: group })
}

/// Add a user to a group.
pub fn add_group_member(group: GroupRef, user: u64) -> BatchOp {
    BatchOp::AddGroupMember(AddGroupMemberOp {
        add_group_member: group,
        user,
    })
}

/// Remove a user from a group.
pub fn remove_group_member(group: GroupRef, user: u64) -> BatchOp {
    BatchOp::RemoveGroupMember(RemoveGroupMemberOp {
        remove_group_member: group,
        user,
    })
}

// ============================================================================
// Auth DDL (users + roles)
// ============================================================================

/// Create a user with a plaintext password. Returns a builder for optional
/// roles/profile.
pub fn create_user(name: impl Into<String>, password: impl Into<String>) -> CreateUser {
    CreateUser {
        name: name.into(),
        password: password.into(),
        roles: Vec::new(),
        profile: None,
        database: None,
    }
}

/// Builder for [`CreateUserOp`].
pub struct CreateUser {
    name: String,
    password: String,
    roles: Vec<String>,
    profile: Option<serde_json::Value>,
    database: Option<String>,
}

impl CreateUser {
    /// Assign roles to the new user.
    pub fn roles(mut self, roles: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.roles = roles.into_iter().map(Into::into).collect();
        self
    }

    /// Set the user profile JSON.
    pub fn profile(mut self, profile: serde_json::Value) -> Self {
        self.profile = Some(profile);
        self
    }

    /// Scope the user to a database, allowing that database's owner to
    /// manage it without global-admin rights.
    pub fn database(mut self, database: impl Into<String>) -> Self {
        self.database = Some(database.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CreateUser(CreateUserOp {
            create_user: self.name,
            password: SecretString::from(self.password),
            roles: self.roles,
            profile: self.profile,
            database: self.database,
        })
    }
}

impl From<CreateUser> for BatchOp {
    fn from(b: CreateUser) -> Self {
        b.build()
    }
}

/// Drop a user by name.
pub fn drop_user(name: impl Into<String>) -> DropUser {
    DropUser {
        name: name.into(),
        hmac: None,
    }
}

/// Builder for [`DropUserOp`].
pub struct DropUser {
    name: String,
    hmac: Option<String>,
}

impl DropUser {
    /// Attach the hex-encoded HMAC tag.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::DropUser(DropUserOp {
            drop_user: self.name,
            hmac: self.hmac,
        })
    }
}

impl From<DropUser> for BatchOp {
    fn from(b: DropUser) -> Self {
        b.build()
    }
}

/// Create a role with a set of permissions.
pub fn create_role(name: impl Into<String>, permissions: Vec<Permission>) -> BatchOp {
    BatchOp::CreateRole(CreateRoleOp {
        create_role: name.into(),
        permissions,
    })
}

/// Drop a role by name.
pub fn drop_role(name: impl Into<String>) -> DropRole {
    DropRole {
        name: name.into(),
        hmac: None,
    }
}

/// Builder for [`DropRoleOp`].
pub struct DropRole {
    name: String,
    hmac: Option<String>,
}

impl DropRole {
    /// Attach the hex-encoded HMAC tag.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::DropRole(DropRoleOp {
            drop_role: self.name,
            hmac: self.hmac,
        })
    }
}

impl From<DropRole> for BatchOp {
    fn from(b: DropRole) -> Self {
        b.build()
    }
}

/// Grant a role to a user.
pub fn grant_role(role: impl Into<String>, user: impl Into<String>) -> BatchOp {
    BatchOp::GrantRole(GrantRoleOp {
        grant_role: role.into(),
        user: user.into(),
    })
}

/// Revoke a role from a user.
pub fn revoke_role(role: impl Into<String>, user: impl Into<String>) -> BatchOp {
    BatchOp::RevokeRole(RevokeRoleOp {
        revoke_role: role.into(),
        user: user.into(),
    })
}

// ============================================================================
// Function DDL
// ============================================================================

/// Create (or replace) a stored function. Returns a builder for source/wasm.
pub fn create_function(name: impl Into<String>) -> CreateFunction {
    CreateFunction {
        name: name.into(),
        source: None,
        wasm: None,
        replace: false,
    }
}

/// Builder for [`CreateFunctionOp`].
pub struct CreateFunction {
    name: String,
    source: Option<String>,
    wasm: Option<String>,
    replace: bool,
}

impl CreateFunction {
    /// Set the Rust source code to compile.
    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Set the pre-compiled WASM bytes (base64-encoded).
    pub fn wasm(mut self, wasm: impl Into<String>) -> Self {
        self.wasm = Some(wasm.into());
        self
    }

    /// Enable replace-if-exists semantics.
    pub fn replace(mut self) -> Self {
        self.replace = true;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CreateFunction(CreateFunctionOp {
            create_function: self.name,
            source: self.source,
            wasm: self.wasm,
            replace: self.replace,
        })
    }
}

impl From<CreateFunction> for BatchOp {
    fn from(b: CreateFunction) -> Self {
        b.build()
    }
}

/// Drop a stored function by name.
pub fn drop_function(name: impl Into<String>) -> BatchOp {
    BatchOp::DropFunction(DropFunctionOp {
        drop_function: name.into(),
    })
}

/// Rename a stored function.
pub fn rename_function(from: impl Into<String>, to: impl Into<String>) -> BatchOp {
    BatchOp::RenameFunction(RenameFunctionOp {
        rename_function: from.into(),
        to: to.into(),
    })
}

// ============================================================================
// Validator DDL
// ============================================================================

/// Create (or replace) a validator. Returns a builder for source/wasm.
pub fn create_validator(name: impl Into<String>) -> CreateValidator {
    CreateValidator {
        name: name.into(),
        source: None,
        wasm: None,
        replace: false,
    }
}

/// Builder for [`CreateValidatorOp`].
pub struct CreateValidator {
    name: String,
    source: Option<String>,
    wasm: Option<String>,
    replace: bool,
}

impl CreateValidator {
    /// Set the Rust source code to compile.
    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Set the pre-compiled WASM bytes (base64-encoded).
    pub fn wasm(mut self, wasm: impl Into<String>) -> Self {
        self.wasm = Some(wasm.into());
        self
    }

    /// Enable replace-if-exists semantics.
    pub fn replace(mut self) -> Self {
        self.replace = true;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CreateValidator(CreateValidatorOp {
            create_validator: self.name,
            source: self.source,
            wasm: self.wasm,
            replace: self.replace,
        })
    }
}

impl From<CreateValidator> for BatchOp {
    fn from(b: CreateValidator) -> Self {
        b.build()
    }
}

/// Drop a validator by name.
pub fn drop_validator(name: impl Into<String>) -> BatchOp {
    BatchOp::DropValidator(DropValidatorOp {
        drop_validator: name.into(),
    })
}

/// Rename a validator.
pub fn rename_validator(from: impl Into<String>, to: impl Into<String>) -> BatchOp {
    BatchOp::RenameValidator(RenameValidatorOp {
        rename_validator: from.into(),
        to: to.into(),
    })
}

/// Bind a validator to a table on specified write operations. Returns a builder.
pub fn bind_validator(name: impl Into<String>, table: impl Into<String>) -> BindValidator {
    BindValidator {
        name: name.into(),
        db: String::new(),
        repo: "main".to_owned(),
        table: table.into(),
        ops: Vec::new(),
        priority: 1500,
    }
}

/// Builder for [`BindValidatorOp`].
pub struct BindValidator {
    name: String,
    db: String,
    repo: String,
    table: String,
    ops: Vec<WriteOp>,
    priority: u16,
}

impl BindValidator {
    /// Set the database name.
    pub fn db(mut self, db: impl Into<String>) -> Self {
        self.db = db.into();
        self
    }

    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Set the write operations the validator fires on.
    pub fn ops(mut self, ops: impl IntoIterator<Item = WriteOp>) -> Self {
        self.ops = ops.into_iter().collect();
        self
    }

    /// Set the priority (must be in `[1000, 9999]`).
    pub fn priority(mut self, priority: u16) -> Self {
        self.priority = priority;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::BindValidator(BindValidatorOp {
            bind_validator: self.name,
            db: self.db,
            repo: self.repo,
            table: self.table,
            ops: self.ops,
            priority: self.priority,
        })
    }
}

impl From<BindValidator> for BatchOp {
    fn from(b: BindValidator) -> Self {
        b.build()
    }
}

/// Unbind a validator from a table. Returns a builder for optional fields.
pub fn unbind_validator(name: impl Into<String>, table: impl Into<String>) -> UnbindValidator {
    UnbindValidator {
        name: name.into(),
        db: String::new(),
        repo: "main".to_owned(),
        table: table.into(),
    }
}

/// Builder for [`UnbindValidatorOp`].
pub struct UnbindValidator {
    name: String,
    db: String,
    repo: String,
    table: String,
}

impl UnbindValidator {
    /// Set the database name.
    pub fn db(mut self, db: impl Into<String>) -> Self {
        self.db = db.into();
        self
    }

    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::UnbindValidator(UnbindValidatorOp {
            unbind_validator: self.name,
            db: self.db,
            repo: self.repo,
            table: self.table,
        })
    }
}

impl From<UnbindValidator> for BatchOp {
    fn from(b: UnbindValidator) -> Self {
        b.build()
    }
}

/// List validator bindings for a table. Returns a builder for optional fields.
pub fn list_validators(table: impl Into<String>) -> ListValidatorsBuilder {
    ListValidatorsBuilder {
        table: table.into(),
        db: String::new(),
        repo: "main".to_owned(),
    }
}

/// Builder for [`ListValidatorsOp`].
pub struct ListValidatorsBuilder {
    table: String,
    db: String,
    repo: String,
}

impl ListValidatorsBuilder {
    /// Set the database name.
    pub fn db(mut self, db: impl Into<String>) -> Self {
        self.db = db.into();
        self
    }

    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::ListValidators(ListValidatorsOp {
            list_validators: self.table,
            db: self.db,
            repo: self.repo,
        })
    }
}

impl From<ListValidatorsBuilder> for BatchOp {
    fn from(b: ListValidatorsBuilder) -> Self {
        b.build()
    }
}

// ============================================================================
// Function folder DDL
// ============================================================================

/// Create a function folder by path segments.
pub fn create_function_folder(segments: impl IntoIterator<Item = impl Into<String>>) -> BatchOp {
    BatchOp::CreateFunctionFolder(CreateFunctionFolderOp {
        create_function_folder: segments.into_iter().map(Into::into).collect(),
    })
}

// ============================================================================
// IntoBatchOp impls — let builders compose directly into Batch::op()
// ============================================================================

use crate::batch::IntoBatchOp;

impl IntoBatchOp for DropDb {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for CreateRepo {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for DropRepo {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for CreateTable {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for DropTable {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for CreateIndex {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for DropIndex {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for SetBufferConfig {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for GetBufferConfig {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for AlterBufferCfg {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for ListTables {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for ListIndexes {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for StartMigration {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for CommitMig {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for RollbackMig {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for AccessTree {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for CreateUser {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for DropUser {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for DropRole {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for CreateFunction {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for CreateValidator {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for BindValidator {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for UnbindValidator {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

impl IntoBatchOp for ListValidatorsBuilder {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

#[cfg(test)]
mod tests;
