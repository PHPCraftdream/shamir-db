בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# perf-#294a — captrack telemetry: инструментирование tx_pipeline bench

> **Target:** добавить вызов `captrack::dump_capacity_stats(path)` в
> `crates/shamir-engine/benches/tx_pipeline.rs`. При сборке с
> `--features captrack/telemetry` он должен сбрасывать JSON-статистику.
> Off-feature — `dump_capacity_stats` это no-op (захардкожено в captrack), поэтому
> вызывать его можно без cfg-гарда.

## ⛔ Запреты

- НЕ `git reset/checkout/clean/stash/restore/rm` и любая git-мутация дерева/индекса.
  Только редактируй; коммитит оркестратор. НЕ удаляй отслеживаемые. НЕ sub-agent.
- Тесты — ТОЛЬКО `./scripts/test.sh` (raw `cargo test` заблокирован).
- Скоуп СТРОГО один файл: `crates/shamir-engine/benches/tx_pipeline.rs`.
- НЕ трогать другие benches, src, тесты.

## Прочитанная реальность

`tx_pipeline.rs:898` использует:
```rust
criterion_group!(
    benches,
    bench_insert_tx_vs_non_tx,
    bench_batch_insert_pipeline,
    bench_commit_tx_phase_breakdown,
    bench_provider_overhead,
    bench_commit_phase5c_indexed_sled,
    bench_async_commit_index_heavy,
    bench_read_scan,
);
criterion_main!(benches);
```

`criterion_main!(benches)` раскрывается в:
```rust
fn main() {
    benches();
    Criterion::default().configure_from_args().final_summary();
}
```

Нам нужно подменить `criterion_main!(benches);` на hand-written `main()`,
который дополнительно вызывает `captrack::dump_capacity_stats(...)`.

captrack уже path-dep в `Cargo.toml:30` (`features = ["fxhash"]`). Feature
`telemetry` пробрасывается через CLI (`--features captrack/telemetry`).

`captrack::dump_capacity_stats<P: AsRef<Path>>(path) -> std::io::Result<()>`:
- Если `telemetry` ON — пишет JSON по path (создаёт parent dir).
- Если OFF — no-op, возвращает `Ok(())` сразу. Безопасно вызывать без cfg.

## Задача

Заменить в `tx_pipeline.rs`:

```rust
criterion_main!(benches);
```

на:

```rust
fn main() {
    benches();
    criterion::Criterion::default()
        .configure_from_args()
        .final_summary();

    // captrack: при сборке с --features captrack/telemetry — сбросит JSON
    // в путь из env CAPTRACK_DUMP, или дефолт. Off-feature — no-op.
    let dump_path = std::env::var("CAPTRACK_DUMP")
        .unwrap_or_else(|_| "target/capacity-stats/tx_pipeline.json".to_string());
    let _ = captrack::dump_capacity_stats(&dump_path);
}
```

(Если в шапке файла нет `use criterion::Criterion` — оставь fqn
`criterion::Criterion::default()`. Имя `criterion_main!` можно убрать из
imports.)

## Гейт (агент сам прогоняет, приложи ПОЛНЫЙ вывод)

```
cargo build -p shamir-engine --benches
cargo build -p shamir-engine --benches --features captrack/telemetry
cargo clippy -p shamir-engine --benches -- -D warnings
cargo fmt -p shamir-engine -- --check
```

Всё четыре — зелёное. `clippy` особенно важно: новый `fn main` не должен
ловить `clippy::missing_docs_in_private_items` / `dead_code` / прочее.

## Финальный отчёт

- diff (полный) затронутого файла;
- вывод гейта (все 4 команды);
- одна короткая фраза про то, что telemetry-OFF путь сохранён (no-op `dump_capacity_stats`
  при отсутствии feature).

Bench-запуск — оркестратор делает сам после успешного гейта.
