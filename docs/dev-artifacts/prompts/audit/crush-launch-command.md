# Как запускать/продолжать crush-сессию неинтерактивно (без ожидания ввода)

Проблема: если вставить промпт как позиционный аргумент прямо в свой
интерактивный терминал, `crush run` может оборваться с `ERROR Context
canceled` при потере фокуса/Ctrl+C/закрытии окна — то есть при любом
взаимодействии с этим же терминалом до завершения команды.

Решение: промпт — в файл, ввод/вывод — редиректом, процесс — в фон
(`&` + `disown`). Тогда команда не привязана к интерактивному stdin и
не может быть случайно "отменена" через терминал.

## Первый запуск (новая сессия)

```bash
crush run --role smart --session <slug> --timeout 60m < docs/dev-artifacts/prompts/audit/<NN>-<slug>.md \
  > .crush/stdin/<slug>.out 2> .crush/stdin/<slug>.err &
disown
```

## Продолжение существующей сессии (тот же --session id)

1. Промпт-продолжение — в отдельный файл:
```bash
echo "Продолжай, друг" > .crush/stdin/<slug>-continue.prompt
```

2. Запуск с тем же `--session`, редирект в фон:
```bash
crush run --role smart --session <slug> --timeout 60m < .crush/stdin/<slug>-continue.prompt \
  > .crush/stdin/<slug>-continue.out 2> .crush/stdin/<slug>-continue.err &
disown
```

crush подхватывает полную историю сессии по `--session <slug>` и
продолжает с того места, где остановился.

## Проверка результата (не доверять отчёту агента "на слово")

```bash
git status --short <crate-paths>          # какие файлы реально тронуты
git diff <файлы>                            # реальный дифф
crush sessions locks <slug>                 # жив ли процесс (alive/offline)
crush sessions show <slug> --with-messages  # полная история сообщений
```

## Пример из этой сессии (A3, MVCC/SSI)

```bash
echo "Continue from current file state (check git diff first). Finish implementing the production fix in table_manager_streaming.rs (the .min() clamp on version_of vs tx.snapshot_version applied uniformly to read_one_tx, read_one_tx_bytes, and record_scan_reads), finish the Red->Green regression test in read_one_tx_tests.rs, run the full gate (cargo fmt -p shamir-engine -- --check, cargo clippy -p shamir-engine --all-targets -- -D warnings, ./scripts/test.sh -p shamir-engine, ./scripts/test.sh -p shamir-tx), and give the complete final report per the brief's Report format section. Finish the task now." \
  > .crush/stdin/ssi-read-set-a3-continue.prompt

crush run --role smart --session ssi-read-set-a3 --timeout 60m \
  < .crush/stdin/ssi-read-set-a3-continue.prompt \
  > .crush/stdin/ssi-read-set-a3-continue.out \
  2> .crush/stdin/ssi-read-set-a3-continue.err &
disown
```
