#!/usr/bin/env bash
# Извлечь плоский self-time топ из perf.data.
DATA="${1:-/tmp/perf-tx_pipeline.data}"
PERF=/usr/lib/linux-tools-6.8.0-124/perf
OUT="${2:-/tmp/perf-flat.txt}"

echo "=== file ==="
ls -la "$DATA"
echo ""
echo "=== TOP self-time symbols ==="
"$PERF" report --input="$DATA" --stdio --no-children -g none 2>/dev/null | grep -vE "^#|^$" | head -40 | tee "$OUT"
echo ""
echo "=== TOP inclusive (with children, --sort=symbol) ==="
"$PERF" report --input="$DATA" --stdio --children --sort=overhead,symbol -g none 2>/dev/null | grep -vE "^#|^$" | head -30
