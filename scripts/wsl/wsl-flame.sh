#!/usr/bin/env bash
# Прогон cargo flamegraph под WSL на shamir-engine lib-тестах.
# Отдельный CARGO_TARGET_DIR, чтобы не перебить Windows target/.
# WSL workaround: /usr/bin/perf — wrapper, ищет perf под current kernel
# (6.18-microsoft, для которого Microsoft не публикует linux-tools).
# Реальный бинарь — linux-tools-6.8 (Ubuntu); kernel ABI совместима для record/report.
set -u
cd /mnt/d/dev/rust/shamir-db
PERF_DIR=$(ls -d /usr/lib/linux-tools-*-generic 2>/dev/null | head -1)
[ -z "$PERF_DIR" ] && PERF_DIR=$(ls -d /usr/lib/linux-tools-* 2>/dev/null | grep -v generic | head -1)
export PATH="$PERF_DIR:$PATH"
export CARGO_TARGET_DIR="$HOME/.cargo-target-wsl-shamir"
mkdir -p "$CARGO_TARGET_DIR"
echo "=== CARGO_TARGET_DIR=$CARGO_TARGET_DIR ==="
echo "=== perf: $(which perf) ==="
perf --version 2>&1 | head -1
echo "=== START ==="
date +%H:%M:%S.%3N
# --dev: использовать dev-профиль (символы качественные, не нужна release-пересборка
# с debuginfo). Тесты будут медленнее, но flamegraph правдивее (без inlining-сжатия).
# WSL2 quirk: dwarf default stack 8192 даёт "Bad address" (ядро 6.18 vs perf-6.8
# mmap-протокол расходится). Уменьшаем до 4096 + понижаем sampling rate.
cargo flamegraph --dev -p shamir-engine --unit-test \
  -c "record -F 50 --call-graph dwarf,4096 -g -o /tmp/perf.data" \
  -o /tmp/engine-flame.svg
rc=$?
echo "=== cargo exit=$rc ==="
date +%H:%M:%S.%3N
ls -la /tmp/engine-flame.svg 2>/dev/null || echo "(no SVG produced)"
exit $rc
