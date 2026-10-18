use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_keys::{build_index_key, build_index_key_from_record};
use crate::base_index::index_manager::IndexManager;
use bytes::Bytes;
use shamir_storage::error::DbResult;
use shamir_types::record_view::RecordRef;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

impl IndexManager {
    /// Deterministic unique-index keys this `value` would claim.
    ///
    /// For every unique index whose paths the value fully populates,
    /// returns the key that `check_unique_constraint`
    /// (and `add_unique_entry`) read/write. The tx commit path records
    /// these as `UniqueGuard`s and re-validates them under `commit_lock`,
    /// closing the tx-concurrent unique hole.
    ///
    /// Returns an empty vec when there are no unique indexes or the
    /// value populates none of them.
    pub fn unique_keys_for(&self, value: &(impl RecordRef + ?Sized)) -> Vec<Bytes> {
        if !self.has_unique_indexes() {
            return Vec::new();
        }
        let mut keys = Vec::with_capacity(4);
        for def in self.indexes_unique.iter() {
            if let Some(irk) =
                build_index_key_from_record(true, def.name_interned, value, &def.paths)
            {
                keys.push(irk.to_bytes());
            }
        }
        keys
    }

    /// Проверяет, существует ли запись с данным значением в уникальном индексе.
    ///
    /// # Возвращает
    ///
    /// - `Ok(Some(RecordId))` — запись существует
    /// - `Ok(None)` — значение свободно
    /// - `Err` — ошибка чтения
    pub(super) async fn check_unique_constraint(
        &self,
        name_interned: u64,
        values: &[InnerValue],
    ) -> DbResult<Option<RecordId>> {
        let index_key = build_index_key(true, name_interned, values).to_bytes();
        self.check_unique_key(&index_key).await
    }

    /// Check a unique constraint by its pre-computed index key bytes.
    ///
    /// P0-3a (#1011) — why the UNIQUE family gets NO `ReaderDrainGate`
    /// (unlike the regular family's `lookup_by_index`): this is the unique
    /// family's only production read chokepoint, and its ONLY production
    /// callers are `validate_unique_for_create` / `validate_unique_for_update`
    /// (via `check_unique_constraint`), BOTH of which are already serialized
    /// against DROP UNIQUE by the per-table `unique_write_lock`. The writer
    /// takes `unique_write_lock` BEFORE calling validate
    /// (`table_manager_crud.rs`: the `unique_write_lock.lock().await` at the
    /// insert/update paths precedes `validate_unique_for_create`/`_update`),
    /// and DROP UNIQUE (`drop_unique_index` → `TableManager::drop_unique_index`)
    /// runs under the same `begin_write_barrier` admission serialization. The
    /// commit-time equivalent is covered too: `pre_commit.rs` Phase 2.5 acquires
    /// every relevant table's `unique_write_lock` and holds it across Phase 2.6
    /// (which reads `info_store().get()` directly under that lock). So a DROP
    /// UNIQUE cannot physically overlap a `check_unique_key` read — adding a
    /// gate here would serialize an already-serialized path.
    ///
    /// TRIPWIRE: `lookup_by_unique_index` (which delegates here) is TEST-ONLY
    /// today (verified by workspace grep — every caller lives under a `tests/`
    /// dir). If it ever gains a PRODUCTION caller that is NOT already inside a
    /// `unique_write_lock` critical section, that caller's read path MUST get
    /// its own `ReaderDrainGate` acquisition at that point — do NOT silently
    /// rely on the unique-family lock serialization holding for a new caller.
    /// Iterate over all unique index definitions.
    pub fn iter_unique_indexes(&self) -> impl Iterator<Item = IndexDefinition> + '_ {
        self.indexes_unique.iter()
    }

    /// Проверяет существование уникального индекса по его имени.
    pub fn unique_index_exists(&self, name_interned: u64) -> bool {
        self.indexes_unique.contains(name_interned)
    }

    /// Возвращает определение уникального индекса по его имени.
    pub fn get_unique_index_definition(&self, name_interned: u64) -> Option<IndexDefinition> {
        self.indexes_unique.get_index(name_interned)
    }
}
