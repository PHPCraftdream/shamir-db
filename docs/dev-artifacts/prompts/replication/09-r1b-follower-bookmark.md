בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# R1-b — follower bookmark (durable per-(db,repo) applied leader-version)

> Контекст: `docs/dev-artifacts/roadmap/REPLICATION.md` §4.2. R1-a `apply_replicated`
> (`crates/shamir-engine/src/tx/apply_replicated.rs`) уже принимает
> `applied_watermark` параметром — R1-b даёт ему durable-хранилище.

## Задача

Durable bookmark: наибольший LEADER `commit_version`, применённый
follower'ом на данном (db, repo). Хранить в per-repo meta info-store —
ровно тем же механизмом, что `LastCommittedVersion` / `NextTxId`
(`crates/shamir-engine/src/meta/recovery_marker.rs` +
`crates/shamir-engine/src/meta/namespace.rs::MetaKey`).

## Шаги

1. **`meta/namespace.rs`** — добавить вариант `MetaKey::ReplicationBookmark`
   с уникальным строковым ключом (напр. `"_t.rbm"`, в стиле `"_t.lcv"`).
2. **Новый файл `meta/repl_bookmark.rs`** (one-primary-export правило;
   recovery_marker.rs держит commit/txid маркеры — bookmark логически
   отдельный) с:
   ```rust
   pub async fn load_replication_bookmark(info_store: &Arc<dyn Store>) -> DbResult<u64>
   // вернуть 0 если ключа нет (дефолт)
   pub async fn save_replication_bookmark(info_store: &Arc<dyn Store>, version: u64) -> DbResult<()>
   ```
   Переиспользуй приватные `load_u64`/`save_u64` из recovery_marker.rs
   (сделай их `pub(crate)` если нужно) или продублируй минимально — предпочти
   переиспользование. Подключить модуль в `meta/mod.rs`.
3. **RepoInstance-обёртки** (`repo/repo_instance.rs`, рядом с
   `apply_replicated`):
   ```rust
   pub async fn replication_bookmark(&self) -> DbResult<u64>
   // load; дефолт 0
   pub async fn advance_replication_bookmark(&self, version: u64) -> DbResult<()>
   // МОНОТОННО: сохранять только если version > текущего (иначе no-op) —
   // защита от отката при out-of-order доставке
   ```
   Используй `self.tx_info_store().await?`.

## Тесты (meta/tests/ или repl_bookmark_tests.rs; + repo-level в tx/tests/)

`#[tokio::test]`, in-memory repo:
1. **Дефолт 0:** свежий repo → `replication_bookmark()` == 0.
2. **Advance + read:** advance до 5 → read == 5.
3. **Монотонность:** advance до 5, затем advance до 3 → read остаётся 5
   (откат отвергнут); advance до 8 → read == 8.
4. **Переживает reopen:** advance до 7, переоткрыть repo (durable backend —
   если тест на in-memory не переживает reopen, используй durable temp-
   backend по образцу существующих persistence-тестов; если это тяжело в
   объёме R1-b, сделай reopen-тест на том backend'е, что реально
   персистит, и отметь в финальном сообщении) → read == 7.

## Гейт

- `./scripts/test.sh @oracle` зелёный.
- `cargo fmt -p shamir-engine -- --check` чистый.
- `cargo clippy -p shamir-engine --all-targets -- -D warnings` чистый.

## Definition of done

- `MetaKey::ReplicationBookmark` + `meta/repl_bookmark.rs` + RepoInstance
  обёртки (монотонный advance).
- Тесты 1-4 зелёные.
- Тронуты: namespace.rs, repl_bookmark.rs, meta/mod.rs, repo_instance.rs,
  recovery_marker.rs (если делаешь load_u64 pub(crate)), тесты.
- Финальное сообщение: тронутые файлы, как решён reopen-тест, вывод test.sh.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
