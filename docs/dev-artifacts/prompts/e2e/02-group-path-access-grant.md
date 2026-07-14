בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# G.4d (A2) — e2e negative+positive: group-path access grant

## Цель
Закрыть единственный непокрытый e2e access-путь: **членство в группе + chgrp +
group-bits грантит доступ** не-владельцу; удаление из группы → снова `access_denied`.
Сейчас group-ветка `authorize_access` покрыта ТОЛЬКО Rust-unit
(`enforcement_tests::group_member_authorized_via_group_bits`), не e2e.

## Контекст — что УЖЕ покрыто (НЕ дублировать)
e2e-permissions.test.ts после G.4c уже покрывает (enforced-дефолт реальный):
- A1 не-superuser denied на createDb (create-ops = is_admin, superuser-only на wire).
- A2/A3/B3/B4 — denied read/write на owner-only (enforced 0o700).
- A4/B5 — chmod 0o777 → read проходит; A5/B6 — chmod 0o700 → снова denied.
- B7 — grant → доступ. A10 — insert после открытия.
- G3-chgrp — chgrp + accessTree readback группы.
**G.4d добавляет ТОЛЬКО group-membership-path** (other-bits уже покрыт chmod-тестами).

> Примечание для итогового отчёта (НЕ код): create-гейт G.4b (Action::Create) не
> выразим в e2e — create-ops superuser-gated на wire (is_admin), не-superuser до
> него не доходит. Его enforced-покрытие — в Rust-интеграции (sec1_ddl_gate_e2e,
> facade_gateway_acl_tests), уже зелёной после G.4c. Гейт authorize_access стоит
> на всех admin-путях (16 файлов) — подтверждено разведкой.

## Заземление (билдеры, всё существует)
- `admin.createGroup(name)` → результат содержит числовой `group_id` (см. G3-chgrp).
- `admin.addGroupMember(ref, user)` / `admin.removeGroupMember(ref, user)` — admin.ts:146/159.
  `ref` = `admin.groupId(gid)` или `admin.groupName(name)`; `user` принимает
  username-строку (хэшится principalId) ИЛИ bigint. e2e-principal доказал, что
  TS principalId совпадает с серверным.
- `admin.chgrp(resource, group: number|null)` — admin.ts:121.
- `admin.chmod(resource, mode)` — admin.ts:104. `admin.refDatabase/refStore/refTable`.
- Образец: e2e-permissions.test.ts A4 (:183) — chmod db+store+table, затем
  `Query.from(table)` от userClient. Хелперы: `createUserAndConnect`, `setupDb`,
  `seed`, `br`, `connectAs`, `Query.from`.

## Срез — 1 новый it() в e2e-permissions.test.ts (внутри главного describe)
`it('A11/G4d-group: group membership + chgrp + group bits grant read; removal re-denies', ...)`:
1. Свежий db через admin: `const gdb = await setupDb(adminClient!, 'perm_grp', ['vault'])`;
   `seed(adminClient!, gdb, 'vault', [{ id: 'g1', secret: 'group-only' }])`.
2. Свежий не-superuser юзер: `const gUser = 'perm_g_${process.pid}'`;
   `const gClient = await createUserAndConnect(gUser, '<pw>')`.
3. Precondition — БЕЗ группы юзер denied (enforced 0o700 дефолт):
   `Query.from('vault')` от gClient → `.rejects` / catch `access_denied`.
4. createGroup → `gid`; `addGroupMember(admin.groupId(gid), gUser)`.
5. chgrp db+store+table на `gid` (3 chgrp) И chmod db+store+table на `0o770`
   (owner-rwx + group-rwx: group получает x на предках для traversal и r на table).
   Всё одним batch'ем через adminClient на `gdb`.
6. Теперь gClient читает `vault` → УСПЕХ, `records[0].secret === 'group-only'`.
7. `removeGroupMember(admin.groupId(gid), gUser)` → gClient читает → снова
   `access_denied` (group-bits остались, но юзер уже не член; other=0).
8. (закрытие клиента gClient в afterAll-стиле или try/finally — свериться, как
   suite закрывает userAClient/userBClient: отдельный afterAll).

Точные формы (group_id из createGroup, форма ошибки) — подтверди эмпирически на
запущенном сервере; ассерты зафиксируй по факту.

## Гейт
- Серверный код НЕ менялся (только TS-тест) → бинарь актуален:
  `D:/dev/rust/.cargo-target/debug/shamir-server.exe`. Пересборка НЕ нужна.
- Прогон:
  ```
  cd crates/shamir-client-ts && \
  SHAMIR_SERVER_BIN=D:/dev/rust/.cargo-target/debug/shamir-server.exe \
  npx vitest run e2e-permissions 2>&1 | tail -50
  ```
  Новый it() должен быть PASS (не SKIP). Затем для надёжности — полный
  `npx vitest run` (весь набор зелёный, 708+1).

## Дисциплина (ОБЯЗАТЕЛЬНО)
- ⛔ НЕ используй agent/sub-agent — падает context-canceled. Читай файлы напрямую.
- ⛔ NEVER git reset/checkout/clean/stash/restore/rm или любую мутирующую git-команду.
  Только редактируй файлы. НЕ коммить — коммитит оркестратор.
- Queries — ТОЛЬКО билдеры (никакого raw JSON). Surgical: один новый it() (+ при
  необходимости afterAll для закрытия gClient). Не трогай существующие тесты.
- Заверши финальным текстом: имя нового it() + вывод vitest (PASS-строки).

## Коммит (оркестратор, после zero-trust verify)
`test(e2e): G.4d — group-membership access-grant path (chgrp + group bits)`
