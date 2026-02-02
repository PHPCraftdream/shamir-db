use crate::db::engine::interner::PersistentInterner;
use crate::db::error::DbResult;
use crate::db::storage::types::Store;
use crate::types::record_id::RecordId;
use crate::types::value::{UserValue};
use std::sync::Arc;
use crate::types::repo_record::RepoRecord;

/// A Table represents a logical collection of records with its own persistence and interner.
pub struct Table {
    name: String,
    data_store: Arc<dyn Store>,
    interner: Arc<PersistentInterner>,
}

impl Table {
    pub fn new(name: String, data_store: Arc<dyn Store>, interner: Arc<PersistentInterner>) -> Self {
        Self {
            name,
            data_store,
            interner,
        }
    }

    /// Inserts a new record.
    pub async fn insert(&self, value: &UserValue) -> DbResult<RecordId> {
        let inner = self.to_inner(value).await?;
        self.data_store.insert(&inner).await
    }

    /// Sets or updates a record.
    pub async fn set(&self, key: RecordId, value: &UserValue) -> DbResult<bool> {
        let inner = self.to_inner(value).await?;
        self.data_store.set(key, &inner).await
    }

    /// Retrieves a record.
    pub async fn get(&self, key: RecordId) -> DbResult<UserValue> {
        let record = self.data_store.get(key).await?;
        Ok(self.to_user(&record))
    }

    /// Removes a record.
    pub async fn remove(&self, key: RecordId) -> DbResult<bool> {
        self.data_store.remove(key).await
    }

    /// Returns all records.
    pub async fn iter(&self) -> DbResult<Vec<(RecordId, UserValue)>> {
        let records = self.data_store.iter().await?;
        let mut result = Vec::with_capacity(records.len());
        for record in records {
            let id = record.0;
            let val = self.to_user(&record);
            result.push((id, val));
        }
        Ok(result)
    }

    // Transformation helpers
    // Note: We need a bridge between PersistentInterner and core::Interner or 
    // we should make transformation functions work with a trait/interface.
    // For now, let's do a simple bridge.

    async fn to_inner(&self, value: &UserValue) -> DbResult<crate::types::value::InnerValue> {
        // This is a bit inefficient because it creates a temporary core::Interner
        // or we need to update user_to_inner to be async and support PersistentInterner.
        // Let's create a temporary bridge for now.
        
        let temp_interner = crate::core::interner::Interner::new();
        // Fill temp interner with existing data from persistent one
        // (Actually, user_to_inner might call touch_ind, which needs to be persistent)
        
        // REFACTORING NEEDED: user_to_inner should probably take a trait.
        // For this MVP, I'll implement a custom async transformation here.
        self.transform_user_to_inner(value).await
    }

    fn to_user(&self, record: &RepoRecord) -> UserValue {
        // record.3 is InnerValue
        let temp_interner = crate::core::interner::Interner::new();
        // This is also inefficient.
        // TODO: Update core::transform to use a trait for Interner.
        
        // For now, let's use a manual transformation or a bridge.
        self.transform_inner_to_user(&record.3)
    }

    async fn transform_user_to_inner(&self, value: &UserValue) -> DbResult<crate::types::value::InnerValue> {
        use crate::types::value::Value;
        use crate::types::common::{new_map_wc, new_set_wc};

        match value {
            Value::Nil => Ok(Value::Nil),
            Value::Bool(b) => Ok(Value::Bool(*b)),
            Value::Int(i) => Ok(Value::Int(*i)),
            Value::F64(f) => Ok(Value::F64(*f)),
            Value::Dec(d) => Ok(Value::Dec(d.clone())),
            Value::Big(b) => Ok(Value::Big(b.clone())),
            Value::Str(s) => Ok(Value::Str(s.clone())),
            Value::Bin(b) => Ok(Value::Bin(b.clone())),
            Value::List(list) => {
                let mut inner_list = Vec::with_capacity(list.len());
                for v in list {
                    inner_list.push(Box::pin(self.transform_user_to_inner(v)).await?);
                }
                Ok(Value::List(inner_list))
            }
            Value::Set(set) => {
                let mut inner_set = new_set_wc(set.len());
                for v in set {
                    inner_set.insert(Box::pin(self.transform_user_to_inner(v)).await?);
                }
                Ok(Value::Set(inner_set))
            }
            Value::Map(map) => {
                let mut inner_map = new_map_wc(map.len());
                for (key, val) in map {
                    let interned_key = self.interner.touch_ind(key).await?;
                    let inner_val = Box::pin(self.transform_user_to_inner(val)).await?;
                    inner_map.insert(interned_key, inner_val);
                }
                Ok(Value::Map(inner_map))
            }
        }
    }

    fn transform_inner_to_user(&self, value: &crate::types::value::InnerValue) -> UserValue {
        use crate::types::value::Value;
        use crate::types::common::{new_map_wc, new_set_wc};

        match value {
            Value::Nil => Value::Nil,
            Value::Bool(b) => Value::Bool(*b),
            Value::Int(i) => Value::Int(*i),
            Value::F64(f) => Value::F64(*f),
            Value::Dec(d) => Value::Dec(d.clone()),
            Value::Big(b) => Value::Big(b.clone()),
            Value::Str(s) => Value::Str(s.clone()),
            Value::Bin(b) => Value::Bin(b.clone()),
            Value::List(list) => {
                let user_list = list.iter().map(|v| self.transform_inner_to_user(v)).collect();
                Value::List(user_list)
            }
            Value::Set(set) => {
                let mut user_set = new_set_wc(set.len());
                for v in set {
                    user_set.insert(self.transform_inner_to_user(v));
                }
                Value::Set(user_set)
            }
            Value::Map(map) => {
                let mut user_map = new_map_wc(map.len());
                for (key_id, val) in map {
                    let key = self.interner.get_str(*key_id).expect("Data corruption: interned key not found");
                    let user_val = self.transform_inner_to_user(val);
                    user_map.insert(key, user_val);
                }
                Value::Map(user_map)
            }
        }
    }
}
