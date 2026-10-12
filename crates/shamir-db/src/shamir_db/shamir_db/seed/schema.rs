use std::sync::Arc;

use shamir_engine::repo::RepoInstance;
use shamir_engine::validator::schema::SchemaValidator;
use shamir_engine::validator::{RecordValidator, ValidatorBinding, WriteOp};
use shamir_storage::types::{RecordKey, Store};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use crate::{DbError, DbResult, ShamirDb};

use super::records_codec::encode_schema_records;

use super::super::schema_management::{
    parse_schema, schema_validator_name, SCHEMA_VALIDATOR_PRIORITY,
};

impl ShamirDb {
    /// Compile against destination IDs and persist only in the unpublished image.
    pub async fn prepare_seed_schema(
        &self,
        repo: &RepoInstance,
        db_name: &str,
        repo_name: &str,
        tables: &[(String, QueryValue, u64)],
    ) -> DbResult<Arc<dyn Store>> {
        let manager = repo.repo_interner().await?;
        let interner = manager.get().await?;
        let mut records = Vec::with_capacity(tables.len());
        for (name, names_schema, version) in tables {
            let mut schema = names_schema.clone();
            if let QueryValue::List(rules) = &mut schema {
                for rule in rules {
                    let QueryValue::Map(fields) = rule else {
                        return Err(DbError::Validation("seed rule must be a map".into()));
                    };
                    let Some(QueryValue::List(path)) = fields.get_mut("path") else {
                        return Err(DbError::Validation("seed schema path missing".into()));
                    };
                    for value in path {
                        let name = value.as_str().ok_or_else(|| {
                            DbError::Validation("seed schema path must contain names".into())
                        })?;
                        let id = interner
                            .touch_ind(name)
                            .map_err(|e| DbError::Codec(e.into()))?
                            .into_key()
                            .id();
                        *value = QueryValue::Int(
                            i64::try_from(id).map_err(|e| DbError::Codec(e.to_string()))?,
                        );
                    }
                }
            }
            let validator_id = RecordId::new();
            if !matches!(schema, QueryValue::Null) {
                let validator: Arc<dyn RecordValidator> =
                    Arc::new(SchemaValidator::new(parse_schema(&schema, interner)?));
                let validator_name = schema_validator_name(db_name, repo_name, name);
                self.validators
                    .register(validator_id, &validator_name, validator)
                    .map_err(|e| DbError::Validation(e.to_string()))?;
                // Bind the isolated table directly: never insert staging in dbs.
                repo.get_table(name)
                    .await?
                    .add_validator_binding(ValidatorBinding {
                        validator_id,
                        ops: vec![
                            WriteOp::Insert,
                            WriteOp::Update,
                            WriteOp::Upsert,
                            WriteOp::Delete,
                        ]
                        .into(),
                        priority: SCHEMA_VALIDATOR_PRIORITY,
                    })
                    .await?;
            }
            // Metadata values, not queries: SystemStore owns its write protocol.
            let mut fields = shamir_types::types::common::new_map();
            fields.insert("db_name".into(), QueryValue::Str(db_name.into()));
            fields.insert("repo_name".into(), QueryValue::Str(repo_name.into()));
            fields.insert("table_name".into(), QueryValue::Str(name.clone()));
            fields.insert("enable_indexes".into(), QueryValue::Bool(false));
            fields.insert("schema".into(), schema);
            fields.insert(
                "schema_version".into(),
                QueryValue::Int(
                    i64::try_from(*version).map_err(|e| DbError::Codec(e.to_string()))?,
                ),
            );
            fields.insert(
                "schema_validator_id".into(),
                QueryValue::Str(validator_id.to_string()),
            );
            let record = QueryValue::Map(fields);
            records.push(record);
        }
        let store = repo.seed_schema_store().await?;
        let bytes = encode_schema_records(&records)?;
        store
            .set(RecordKey::from_slice(b"schemas"), bytes.into())
            .await?;
        store.flush().await?;
        Ok(store)
    }
}
