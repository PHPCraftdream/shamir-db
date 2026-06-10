use shamir_query_types::admin::{
    AlterBufferConfigOp, BufferConfigDto, BufferConfigPatch, GetBufferConfigOp, SetBufferConfigOp,
};
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

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

impl IntoBatchOp for SetBufferConfig {
    fn into_batch_op(self) -> BatchOp {
        self.build()
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

impl IntoBatchOp for GetBufferConfig {
    fn into_batch_op(self) -> BatchOp {
        self.build()
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

impl IntoBatchOp for AlterBufferCfg {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
