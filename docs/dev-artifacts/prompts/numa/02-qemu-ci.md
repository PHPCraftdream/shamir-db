בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief: NUMA N2 — QEMU CI harness

## Цель

Заполнить tier3-qemu job в `.github/workflows/numa.yml` (сейчас no-op echo) реальным harness'ом: boot 2-node QEMU гостя, запустить shamir-numa тесты внутри гостя, отрепортить результаты.

## Контекст

`numa.yml` уже существует. tier3-qemu job — opt-in через `[numa-qemu]` flag в commit message, runs-on `ubuntu-latest`. Действующий LinuxTopology (N1) уже в дереве на момент этой задачи. Цель — дать way тестировать multi-socket логику без физического железа.

## Что делать

### 1. `scripts/ci-qemu-numa-test.sh`

Новый скрипт, исполняемый. Boot QEMU гостя с 2 NUMA-нодами, прокинуть workspace внутрь, прогнать `cargo test -p shamir-numa --lib --locked` + `cargo test -p shamir-numa --test linux_topology --locked`.

Простейший harness — через `qemu-system-x86_64` user-mode networking + virtfs (9p share) workspace mount. Альтернативно — `cloud-init` с Ubuntu cloud image и SSH. Выбери проще и стабильнее.

Скелет команды (под выбранную стратегию docstring в шапке скрипта):

```bash
#!/usr/bin/env bash
set -euo pipefail

# Параметры NUMA-гостя:
#   2 сокета × 2 CPU = 4 vCPU
#   2 GB RAM split поровну
#   node 0: CPUs 0-1
#   node 1: CPUs 2-3
QEMU_ARGS=(
  -nographic -m 2G -smp 4,sockets=2,cores=2,threads=1
  -numa node,cpus=0-1,nodeid=0,mem=1G
  -numa node,cpus=2-3,nodeid=1,mem=1G
  ...
)

# Запустить тесты внутри гостя любым transport:
#   - virtfs 9p mount workspace → cargo test внутри
#   - или ssh в boot'нутый guest
```

Реальные детали (cloud image / kernel / initrd / mount setup) — твоя зона; главное чтобы скрипт **на ubuntu-latest GitHub runner** прогнал тесты и вернул ненулевой exit на failure.

Если простой подход не выходит за разумное время (см. ограничение ниже) — оставь скрипт как **минимальный smoke**: `lscpu | grep NUMA` внутри гостя + явный TODO с описанием что нужно для полной интеграции. Документируй честно.

### 2. `numa.yml` — flesh out tier3-qemu

Замени блок:

```yaml
      - name: Boot 2-node QEMU and run Linux integration tests
        run: |
          echo "Tier-3 QEMU NUMA harness is a Фаза 1b deliverable."
          ...
```

На:

```yaml
      - name: Boot 2-node QEMU and run Linux integration tests
        timeout-minutes: 30
        run: bash scripts/ci-qemu-numa-test.sh
```

Сохрани остальную обвязку (`if: contains(github.event.head_commit.message, '[numa-qemu]')`, install шаг QEMU + numactl).

### 3. Прагматичная граница

QEMU NUMA harness — большой кусок. Если ты упираешься в:
- невозможность собрать stable boot pipeline за разумное время,
- интерактивная boot последовательность которую не получается автоматизировать,
- KVM accel недоступен на GitHub runners (это известный констрейт),

— честно оставь скрипт **smoke-уровня** (boot, проверка `lscpu`, exit) и в шапке скрипта + в README документируй что нужно для полной integration. tier3 — opt-in, не блокирует merge. Лучше честный skeleton с TODO чем фальшивый зелёный CI.

## Discipline

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`. Только редактирование файлов.

- Скрипт `#!/usr/bin/env bash` + `set -euo pipefail`.
- ShellCheck-clean (если есть SC-замечания, дави их по делу или explain inline).
- Команды QEMU обязательно с явным `-no-reboot` чтоб не зависнуть.
- `timeout-minutes: 30` в job обязателен.

## Done =

1. `scripts/ci-qemu-numa-test.sh` создан, executable (`chmod +x`), `#!/usr/bin/env bash`.
2. `.github/workflows/numa.yml` tier3-qemu вызывает скрипт.
3. README.md в `crates/shamir-numa/` (или comment в скрипте) объясняет что harness делает + любые открытые TODO.
4. Файлы uncommitted (orchestrator коммитит).
