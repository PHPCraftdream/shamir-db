בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Durable-by-default — wire-created repos

**Status:** design, revision **2026-06-02**.

## Созерцание (почему)

Слово `in_memory` тайно несёт **две ортогональные оси**:

* **скорость** — данные в RAM (это про *кеш*),
* **бренность** — данные не переживают рестарт (это про *durability*).

Мы их склеили в одно имя движка. Расклеиваем обратно:

> **Память — не движок, а кеш, который есть у всех.
> Движок — это durable-носитель, и он же есть Истина.
> Каждое репо = `[MemBuffer-кеш] → [durable-бэкинг]`.
> «Чистая память» — вырожденный случай: ты ЯВНО отказался от бэкинга.**

Это ровно truth-model из `NEXT_PHASES.md`: истина живёт в версионированном
store, всё производное — overlay, WAL — гарант материализации. Сейчас дефолт
`in_memory` нарушает манифест: у него нет истины, только overlay в воздухе.

### Принципы

1. **Бренность требует слова.** Нельзя потерять данные молча. Чтобы получить
   эфемерность — обязан попросить: `engine: "in_memory"`. Дефолт — живой.
2. **Durability репо = durability дома.** `data_root` выводится из места
   системного стора (`SystemStoreConfig::Redb(path)` → `path.parent()`).
   Durable дом → durable репо. In-memory дом (тесты) → in-memory репо.
   Когерентно, без отдельного конфиг-флага.
3. **Диск зеркалит модель.** Путь репо: `data_root/<db>/<repo>.redb` —
   дерево на диске = дерево БД→репо.
4. **Грань — per-repo.** Durability — свойство стора (= репо со своим
   `__tx__`-WAL). В одной durable-БД можно держать рядом боевое репо и явный
   `in_memory` scratch. Per-db было бы слишком грубо.
5. **Машинерия уже есть.** `ShamirDb::init` переподнимает репо из
   `load_repositories()` через `factory_from_meta(engine, path)` + recovery
   (`shamir_db.rs:134-205`, `:286`). `factory_from_meta` уже умеет `redb`
   (`:295`). Мы лишь чиним дефолт `CreateRepo` и протаскиваем `data_root`.

## Что сейчас (факты)

* `CreateRepo` хардкодит `in_memory` (`execute.rs:70-95`) → wire-репо эфемерны.
* WAL живёт в `__tx__`-сторе того же репо (`repo_instance.rs:326`) → для
  in_memory он тоже in-memory → **WAL не добавляет durability**.
* `default.main` durable: создаётся напрямую `BoxRepoFactory::redb`
  (`server.rs:376`), НЕ через DDL `CreateRepo` — поэтому этот фикс его не
  трогает.
* Таблицы в `default.main` durable (redb + `TablesRegistry`-реплей).

## Слайсы

### D1 — `data_root` в `ShamirDb`
* Добавить поле `data_root: Option<std::path::PathBuf>` в `ShamirDb`
  (`shamir_db.rs:60`).
* В `init_with_env_policy`: вывести из `config` ДО передачи в
  `SystemStore::init` — `SystemStoreConfig::Redb(p) => p.parent().map(PathBuf::from)`,
  `InMemory => None`. (Клонировать путь до move в `SystemStore::init`.)
* Геттер `pub fn data_root(&self) -> Option<&Path>`.

### D2 — `CreateRepo` durable по умолчанию
* `CreateRepoOp.engine`: `String` (default `"in_memory"`) → **`Option<String>`**
  (default `None`). Убрать `default_engine()`. Так различаем «не указан»
  (None → безопасный дефолт) от явного `Some("in_memory")` (осознанная
  эфемерность). Wire-совместимость: клиент, шлющий `"engine":"in_memory"`,
  по-прежнему получает in_memory; отсутствие поля → durable.
* Диспетч `BatchOp::CreateRepo` (`execute.rs:70`):
  * `None` → если `self.shamir.data_root()` = `Some(root)`: durable redb,
    путь `root/<db_name>/<create_repo>.redb`, `std::fs::create_dir_all(root/<db_name>)`
    перед открытием; factory `BoxRepoFactory::redb(path)`. Если `data_root` =
    `None` → `in_memory` (когерентный fallback на in-memory доме).
  * `Some("in_memory")` → `BoxRepoFactory::in_memory()` (явный opt-in).
  * `Some("redb")` → durable, путь как выше (или из `op.path`, если задан).
  * прочее → ошибка `Unsupported engine`.
* Убедиться, что `add_repo` персистит `engine`+`path` в репо-запись
  (init читает `record["engine"]`/`["path"]`). Если нет — дописать в
  `save_repository`. **Тест reopen — финальная проверка этого.**

### D3 — тесты
* `wire_created_repo_is_durable_across_reopen` (shamir-db, durable home через
  `tempfile` + `SystemStoreConfig::Redb`): через DDL `CreateRepo` (engine не
  указан) создать репо+таблицу, вставить запись, **переоткрыть `ShamirDb::init`
  на том же пути**, прочитать — запись жива.
* `explicit_in_memory_repo_is_ephemeral`: `engine:"in_memory"` → после reopen
  репо-данные пусты (или репо пере-создаётся пустым) — эфемерность сохраняется
  как осознанный выбор.
* `repo_file_layout_mirrors_db_repo_tree`: durable wire-репо создаёт файл
  `data_root/<db>/<repo>.redb`.
* In-memory-home тесты (существующие, `SystemStoreConfig::InMemory`): дефолтный
  `CreateRepo` остаётся in_memory — поведение не меняется, существующие тесты
  зелёные.

## Гейт
`cargo fmt -p <touched> -- --check` · `cargo clippy --workspace --all-targets -D warnings`
· `cargo test --workspace --lib` · затронутые интеграционные. Surgical, без
не относящихся к задаче правок. Без commit/push.
