בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# R0-a — wire-типы ReplRequest / ReplResponse (PR5-форма)

> Контекст: `docs/roadmap/REPLICATION.md` §5.1 (ops), §5.3 (R0 = только
> Pull), §10 (PR5-решение: один вариант `DbRequest::Repl`).

## Задача

Ввести privileged репликационный протокол в wire-слой ОДНИМ вариантом
верхнеуровневого enum'а (не пятью), чтобы не раздувать клиентский
`DbRequest` и дать протоколу собственную версию.

## Файлы

Новый файл `crates/shamir-query-types/src/wire/repl.rs` (один primary
export-кластер: связанные repl-типы). Подключить в `wire/mod.rs`
(re-export). Добавить варианты в существующие enum'ы в
`wire/db_message.rs`.

## Типы (в repl.rs)

```rust
use serde::{Deserialize, Serialize};

/// Privileged replication request (leader-facing). Carried as the single
/// `DbRequest::Repl` variant so the replication protocol versions
/// independently of the client query protocol (REPLICATION §5, PR5).
/// R0 implements only Hello + Pull (§5.3); Stream/InternerSync/Status
/// are later phases and are intentionally absent here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "repl_op", rename_all = "snake_case")]
pub enum ReplRequest {
    /// Handshake: advertise protocol version + node identity, learn the
    /// leader's epoch and replicable repos.
    Hello { proto_ver: u32, node_id: String },
    /// Pull a batch of changelog events for one repo from `from_version`.
    Pull {
        db: String,
        repo: String,
        from_version: u64,
        limit: u32,
        /// Long-poll budget in ms. `None`/0 = return immediately even if
        /// no events are available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait_ms: Option<u32>,
    },
}

/// Per-repo advertisement in a `Hello` reply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplRepoInfo {
    pub db: String,
    pub repo: String,
    /// Highest committed version currently in this repo's journal.
    pub current_version: u64,
    /// Lowest version still retained in the journal (G4). R0: 0 (no
    /// retention yet) — follower with bookmark+1 < floor needs reseed.
    pub journal_floor: u64,
}

/// Privileged replication reply. Every variant carries `leader_epoch`
/// (VR-style fencing, §5.2): the follower tracks the max epoch seen and
/// drops a connection whose epoch regresses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "repl_kind", rename_all = "snake_case")]
pub enum ReplResponse {
    Hello {
        leader_epoch: u64,
        repos: Vec<ReplRepoInfo>,
    },
    Pull {
        leader_epoch: u64,
        /// msgpack-encoded `Vec<ChangelogEvent>` — raw events, opaque at
        /// the wire layer (decoded by the follower apply-engine in R1).
        #[serde(with = "serde_bytes")]
        events: Vec<u8>,
        /// Set if a gap was detected (requested `from_version` precedes
        /// `journal_floor`): the follower must reseed from this version.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gap_at: Option<u64>,
        /// Highest version in the repo at reply time (lag computation).
        current_version: u64,
    },
    /// Replication-layer error (bad role, denied repo, unknown repo, stale
    /// epoch). Carries the epoch so the follower can still fence.
    Error {
        leader_epoch: u64,
        code: String,
        message: String,
    },
}
```

## Врезка в db_message.rs

- `DbRequest`: добавить вариант `Repl(ReplRequest)` (serde `#[serde(tag =
  "op")]` уже стоит — убедись, что вложенный enum со своим tag `repl_op` не
  конфликтует; при adjacently/internally-tagged проблеме используй тот
  паттерн, что уже работает для батчей, или пометь вариант так, чтобы
  round-trip был чистым — это ГЛАВНОЕ требование, проверяется тестом).
- `DbResponse`: добавить вариант `Repl(ReplResponse)`.
- Re-export `ReplRequest`, `ReplResponse`, `ReplRepoInfo` из `wire/mod.rs`
  (и, если `db_message`/`wire` реэкспортятся выше — по существующей цепочке).

## Тесты (wire/tests/)

Добавить в существующий `wire/tests/` (манифест-only mod.rs) новый файл
`repl_tests.rs` (зарегистрировать в tests/mod.rs):
- round-trip `DbRequest::Repl(ReplRequest::Hello{..})` через
  `rmp_serde::to_vec_named` → `from_slice` → равно исходному;
- round-trip `DbRequest::Repl(ReplRequest::Pull{..})` с `wait_ms: Some` и
  `None`;
- round-trip `DbResponse::Repl(ReplResponse::Hello{..})` с непустым
  `repos`;
- round-trip `DbResponse::Repl(ReplResponse::Pull{..})` с непустыми
  `events` bytes и `gap_at: Some`/`None`;
- round-trip `DbResponse::Repl(ReplResponse::Error{..})`.
Сравнение — по полям (derive PartialEq на repl-типах, если нужно для
assert_eq; добавь `#[derive(PartialEq)]` к repl-типам).

## Гейт

- `./scripts/test.sh -p shamir-query-types` зелёный.
- `cargo fmt -p shamir-query-types -- --check` чистый.
- `cargo clippy -p shamir-query-types --all-targets -- -D warnings` чистый.

## Definition of done

- repl.rs с типами выше; варианты в DbRequest/DbResponse; re-exports.
- Serde round-trip чистый для всех вариантов (тесты зелёные).
- Только Hello+Pull (никаких Stream/InternerSync/Status).
- Тронуты только wire/repl.rs, wire/mod.rs, wire/db_message.rs, wire/tests/*.
- Финальное сообщение: список тронутых файлов, вывод test.sh.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
