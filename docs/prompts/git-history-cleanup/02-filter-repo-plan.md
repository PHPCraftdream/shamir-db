# План: filter-repo прогон по master (сообщения + удаление мусорных файлов)

Ничего из этого файла ещё не выполнялось. Это план для согласования (задача
#599) перед фактическим запуском (#600).

## Область действия

Только `master` (`--refs master`). Остальные локальные/remote ветки
(`worktree-agent-*`, `opencode/*`, `izuchit-proekt-422e`,
`set-up-project-config-ed1f`, `chore/rand-0.9-bump`, `pr7-onto-master`,
`dependabot/*`) не трогаются и продолжат указывать на старые хэши.

## Бэкап (уже создан и запушен)

- ветка `backup/pre-history-rewrite` → `dbf476dd`
- тег `backup/pre-history-rewrite-2026-07-14` → `dbf476dd`

## 1. Исправление сообщений — 202 коммита

Таблица соответствий: `docs/prompts/git-history-cleanup/junk-commits-message-map.tsv`
(202 строки, верифицирована — см. `docs/checkpoints`/сессионный лог).
Callback: `docs/prompts/git-history-cleanup/commit-callback.py`
(`commit_callback(commit, metadata)`, матчит по `commit.original_id`).

Порядок коммитов, авторы и даты — не меняются. Меняется только `message`.

## 2. Удаление файлов из истории — подтверждено

Три категории, все УЖЕ отсутствуют в текущем HEAD (удалены рабочим
коммитом `616358a` "tidy root" и в `.gitignore`) — удаление из истории не
изменит текущее дерево файлов, только избавит от старых blob'ов:

- `.flamegraphs/` (3 SVG-файла, ~1 МБ каждый):
  - `.flamegraphs/shamir-engine-tx_pipeline-symbols.svg`
  - `.flamegraphs/shamir-engine-tx_pipeline-symbols-post-289.svg`
  - `.flamegraphs/shamir-engine-tx_pipeline-symbols-post-290.svg`
- `run.log` (корень, ~452 КБ, старый scratch-лог)
- `crates/shamir-storage/test_data/fjall_repo_basic/0.jnl` (64 МБ)
- `crates/shamir-storage/test_data/fjall_repo_list/0.jnl` (64 МБ)

Ничего из явно защищённого списка (Cargo.lock, документация, тестовые
fixtures — кроме двух явно согласованных 64 МБ файлов, benchmark-файлы,
клиентские TypeScript-файлы, `.git-blame-ignore-revs`) не удаляется.

## 3. Точная команда (НЕ выполнена, только план)

```bash
git filter-repo \
  --refs master \
  --invert-paths \
  --path .flamegraphs \
  --path run.log \
  --path crates/shamir-storage/test_data/fjall_repo_basic/0.jnl \
  --path crates/shamir-storage/test_data/fjall_repo_list/0.jnl \
  --commit-callback "$(cat docs/prompts/git-history-cleanup/commit-callback.py)" \
  --force
```

`--force` требуется потому что репозиторий не является свежим клоном
(filter-repo по умолчанию отказывается работать не-в-свежем клоне —
у нас есть полноценный бэкап, поэтому это осознанно).

## 4. Ожидаемый эффект

- Хэши ВСЕХ 1755 коммитов на master изменятся (rewrite меняет содержимое
  каждого коммита начиная с первого затронутого — это нормально и
  неизбежно при переписывании истории, даже если у конкретного коммита
  ничего не поменялось).
- Актуальное состояние файлов на HEAD — не меняется (все удаляемые файлы
  уже не в дереве).
- Author/committer date и identity — не меняются.
- Merge-структура — сохраняется (filter-repo сохраняет граф коммитов,
  меняются только payload'ы).

## 5. Верификация после прогона (задача #600)

```bash
git fsck --full
git log --oneline --graph -30
git diff backup/pre-history-rewrite HEAD  # ожидаем: пусто (дерево идентично)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib
```

## 6. Публикация (задача #601, отдельное подтверждение)

```bash
git push --force-with-lease origin master
```

Backup-ветка/тег не удаляются. После публикации — уведомление о
необходимости переклонирования/ресинка локальных копий (если у кого-то
есть форки/клоны этого репо).
