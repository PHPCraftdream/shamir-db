בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# WASM guest-source compilation: sanitization decision

Follow-up decision on the CRITICAL residual finding "компиляция
недоверенного Rust на хосте БД" — originally
`docs/audits/2026-07-06-security-network-surface.md` (Топ-5 #2), partially
closed by CRIT-6/#440 (`docs/prompts/audit/06-wasm-host-untrusted-compile-and-definer-security.md`),
re-confirmed as **CRITICAL (остаточная)** in
`docs/audits/2026-07-10-security-permission-wasm-functions.md` (находка #1)
and rolled up in `docs/audits/2026-07-10-security-permission-SUMMARY.md`.

## Decision

Гостевой Rust-исходник, присылаемый пользователем для WASM-функций,
**санитизируется перед компиляцией на хосте** — компиляция недоверенного
кода остаётся частью архитектуры (полный переход на "принимать только
провалидированный `.wasm`" не выбран как немедленный шаг), поэтому риск
закрывается на уровне пайплайна:

1. **Скан запрещённых макросов до записи исходника на диск и до запуска
   `cargo build`** — уже реализовано в
   `crates/shamir-wasm-host/src/compile.rs` (`FORBIDDEN_MACROS`:
   `include!`/`include_str!`/`include_bytes!`/`env!`/`option_env!`).
   Это первый слой санитизации: отклоняет исходник ДО того, как он вообще
   попадает компилятору.
2. **Allowlist окружения** для дочернего `cargo build` (только
   `PATH`/temp-dirs/cargo-home) — вторая часть санитизации: даже код,
   прошедший скан макросов, не получает доступа к секретам хоста через
   env.
3. **Wall-clock timeout** (`WASM_COMPILE_TIMEOUT`) на компиляцию —
   ограничивает риск compile-bomb/hang как часть той же дисциплины
   "не доверять исходнику".

## Остаточный риск и следующий шаг

Текущая санитизация — лексическая (скан по тексту исходника) и явно не
претендует на полноту: proc-macro/build-script зависимость может
обойти запрет на `include!`/`env!`, поскольку сам запрет проверяет только
литеральный исходник функции, а не транзитивные зависимости, которые
компилируются как часть той же `cargo build`. Это признано в
2026-07-10-security-permission-wasm-functions.md как CRITICAL по
остаточному риску: настоящая изоляция потребует запуска компиляции в
отдельном воркере (контейнер/gVisor или seccomp+rlimit+cgroup, read-only
FS, запрет сети, запрет path-deps proc-macro) — это отдельная, более
крупная задача, вне рамок точечной санитизации, зафиксированной здесь
как временная (но обязательная) мера.

## Статус

Санитизация исходника — принятый, действующий подход к смягчению этого
риска на сегодня. Полная изоляция компиляции (воркер/sandbox) остаётся
открытой рекомендацией в SUMMARY-отчёте и не входит в эту заметку.
