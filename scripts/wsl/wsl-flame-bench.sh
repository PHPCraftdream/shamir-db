#!/usr/bin/env bash
# Flamegraph одного criterion-бенча под WSL — РЕАЛЬНЫЙ hot-path, без тест-обвязки.
# Использование: wsl-flame-bench.sh <crate> <bench_name> [criterion_filter] [profile_secs]
# Пример: wsl-flame-bench.sh shamir-engine tx_pipeline 'tx_overhead/batch_pipeline' 20
set -u
# Cargo PATH — на случай non-login shell (bash -c вместо bash -lc).
[ -r "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
export PATH="$HOME/.cargo/bin:$PATH"

CRATE="${1:?crate}"
BENCH="${2:?bench name}"
FILTER="${3:-}"
SECS="${4:-20}"

cd /mnt/d/dev/rust/shamir-db

# perf: реальный бинарь (wrapper /usr/bin/perf не находит WSL-ядро 6.18).
PERF_DIR=$(ls -d /usr/lib/linux-tools-*/ 2>/dev/null | grep -vi generic | head -1)
export PATH="${PERF_DIR%/}:$PATH"
# Чистим возможно битый symbol-кэш perf (addr2line "could not read first record").
rm -rf "$HOME/.debug" 2>/dev/null || true

# Отдельный Linux target-dir; bench-профиль с debuginfo для символов.
export CARGO_TARGET_DIR="$HOME/.cargo-target-wsl-shamir"
# Cargo.toml workspace ставит [profile.release] strip=true, debug=false и
# [profile.bench] inherits — без override весь Rust-стек резолвится как [unknown].
# Переопределяем ОБА уровня (bench И release — потому что bench-deps собираются
# в release-профиле LTO-зависимостями).
#
# debug=1 (line-tables-only), НЕ 2 (full DWARF). С debug=2 на binary с LTO
# perf script стоит часами на addr2line-резолве inlined frames; debug=1 даёт
# имена функций (что нам нужно для flamegraph) без полной inline-цепочки →
# addr2line работает в десятки раз быстрее, qualitative разница в SVG минимальна.
export CARGO_PROFILE_BENCH_DEBUG=1
export CARGO_PROFILE_BENCH_STRIP=false
export CARGO_PROFILE_RELEASE_DEBUG=1
export CARGO_PROFILE_RELEASE_STRIP=false
# Quick-mode бенчей не нужен — profile-time сам гоняет iter-loop N секунд.
mkdir -p "$CARGO_TARGET_DIR"

echo "=== perf: $(which perf) ($(perf --version 2>/dev/null)) ==="
echo "=== bench: $CRATE/$BENCH  filter='$FILTER'  profile-time=${SECS}s ==="
date +%H:%M:%S.%3N

OUT="/tmp/flame-${BENCH}.svg"
# -F 99 + dwarf,4096: 4096-стек обходит WSL2 "Bad address"; 99Hz × ~N*SECS = много сэмплов.
# Хвост после '--' идёт criterion-харнессу: --bench --profile-time запускает чистый
# iter-loop без stat-анализа; FILTER ограничивает набор бенч-функций.
cargo flamegraph --bench "$BENCH" -p "$CRATE" --no-inline \
  -c "record -F 99 --call-graph dwarf,4096 -g -o /tmp/perf-${BENCH}.data" \
  -o "$OUT" \
  -- --bench --profile-time "$SECS" $FILTER
rc=$?
echo "=== cargo exit=$rc ==="
date +%H:%M:%S.%3N
ls -la "$OUT" "/tmp/perf-${BENCH}.data" 2>/dev/null || echo "(no output produced)"
exit $rc
