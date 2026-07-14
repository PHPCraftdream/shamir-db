# Brief: восстановить осмысленные сообщения для 202 мусорных коммитов ранней истории master

## Контекст

Готовим переписывание ранней Git-истории `shamir-db` (только `master`, ветка
защищена бэкапом `backup/pre-history-rewrite` / тег
`backup/pre-history-rewrite-2026-07-14`, оба уже запушены в origin — эта
задача НИЧЕГО не удаляет и не переписывает сама по себе, только готовит
входные данные для последующего `git filter-repo`).

202 коммита на `master` (с 2026-01-29 по 2026-03-19, вперемешку с обычными
коммитами — не сплошной блок) имеют бессмысленные сообщения: `....` (118 шт),
`...` (78 шт), `storage`, `stogares`, `main.md`, `init`, `fmt`, `engine`
(по 1 шт).

Полный список — `docs/dev-artifacts/prompts/git-history-cleanup/junk-commits-input.txt`,
формат `<hash>|<date>|<старое сообщение>`, в хронологическом порядке.

## Задача

Для КАЖДОГО из 202 хэшей выполнить `git show --stat <hash>` и при необходимости
`git show <hash> -- <файл>` (или `git diff <hash>^..<hash>`), понять что реально
изменилось, и написать одну строку итоговой таблицы:

```
<hash>|<старое сообщение>|<новое сообщение в стиле Conventional Commits>
```

Примеры целевого стиля (из самой задачи):
```
....       -> feat(storage): add durable storage backend
...        -> fix(engine): handle transaction recovery
storage    -> feat(storage): introduce storage layer
engine     -> feat(engine): add query execution engine
```

Правила:
- Сообщение должно отражать РЕАЛЬНОЕ содержимое diff'а этого конкретного
  коммита — не гадать по соседним коммитам.
- Conventional Commits формат: `<type>(<scope>): <описание>`, type один из
  `feat|fix|refactor|test|docs|chore|perf|style|build|ci`. Scope — короткое
  имя крейта/модуля без префикса `shamir-` (например `storage`, `engine`,
  `tx`, `wal`, `types`) или `repo` для общерепозиторных изменений.
- Если по содержимому diff’а НАДЁЖНО определить назначение коммита нельзя
  (например, тривиальный whitespace-фикс без контекста, или мусорный
  коммит без реального смысла) — использовать ровно:
  `chore(history): preserve early repository snapshot`
- НЕ анализировать коммиты, которых нет в списке — сообщения остальных
  1553 коммитов master уже нормальные и трогать их не нужно.
- НЕ пытаться менять сам репозиторий (никаких `git commit`, `git rebase`,
  `git filter-repo`, `git reset`, `git checkout -- <file>`, `git push`,
  `git branch -D` и т.п.) — только читать историю (`git show`, `git log`,
  `git diff`) и писать РЕЗУЛЬТАТ в файл.

## Формат результата

Записать итоговую таблицу (все 202 строки, в ТОМ ЖЕ порядке, что во входном
файле) в новый файл:

```
docs/dev-artifacts/prompts/git-history-cleanup/junk-commits-message-map.tsv
```

Формат каждой строки — `<hash>|<старое сообщение>|<новое сообщение>` (три
поля через `|`, без заголовка таблицы, без пустых строк, ровно 202 строки).

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm` / `rebase` / `filter-repo` / `commit` / `push`, или любую
другую git-команду, которая меняет рабочее дерево, индекс или историю.
Только читай (`git show`, `git log`, `git diff`, `cat`) и пиши файл
`junk-commits-message-map.tsv`. Оркестратор сам проверит результат и сам
закоммитит его.

## Проверка (это сделает оркестратор, не ты)

- Ровно 202 строки, в том же порядке хэшей, что во входном файле.
- Каждая строка имеет ровно 3 поля через `|`.
- Каждое новое сообщение — валидный Conventional Commits формат или ровно
  `chore(history): preserve early repository snapshot`.
- Выборочная проверка нескольких строк против реального `git show <hash>`.
