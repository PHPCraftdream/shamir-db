# ShamirDB documentation

Two top-level categories:

- **[`guide-docs/`](guide-docs/)** — user- and operator-facing documentation:
  guides, architecture, wire protocol, security model. Kept current and
  meant to be read by anyone using the project.
- **[`dev-artifacts/`](dev-artifacts/)** — internal development artefacts:
  audits, checkpoints, design records, perf research, prompts, roadmap.
  Historical/in-progress by nature; status is authoritative only when a
  document explicitly says a feature is implemented and the code and tests
  agree.

## Start here

- [Quick Start](guide-docs/guide/00-quickstart.md) — run the server and make the first request.
- [Guided documentation](guide-docs/guide/README.md) — progressive tour from basic queries to operations and interconnection.
- [Architecture](guide-docs/architecture/ARCHITECTURE.md) — storage, execution, transactions, indexes, and security boundaries.
- [Logic flow](guide-docs/architecture/LOGIC_FLOW.md) — how a request moves through the system.
- [Protocol specification](guide-docs/client-server-protocol-spec/README.md) — authentication, sessions, subscriptions, and transports.
- [Security and data protection](guide-docs/security/data-protection.md) — current guarantees and limitations.
- [Roadmap](dev-artifacts/roadmap/ROADMAP.md) — planned work and known gaps.

## By topic

- `guide-docs/guide/` — progressive user and operator guide.
- `guide-docs/architecture/` — DB internals (storage, types, indexes, transactions).
- `guide-docs/client-server-protocol-spec/` — wire protocol specification.
- `guide-docs/security/` — security model and data-protection guarantees.
- `dev-artifacts/design/` — design decisions and implementation plans.
- `dev-artifacts/ops/` — operating, capacity, and performance guidance.
- `dev-artifacts/audits/` — security, performance, and architecture reviews.
- `dev-artifacts/benchmarks/` and `dev-artifacts/perf/` — benchmark methodology and measured results.
- `dev-artifacts/checkpoints/` — historical development notes; these are not release notes.
- `dev-artifacts/prompts/` — briefs used to delegate work to sub-agents (prompt-first discipline).
- `dev-artifacts/research/` — investigation notes that informed design decisions.
- `dev-artifacts/roadmap/` — planned work and known gaps.
- `dev-artifacts/pre-transactional/` — notes from before the transactional engine existed.
- `dev-artifacts/AUDIT_REPORT.md`, `dev-artifacts/BACKLOG.md`, `dev-artifacts/PROJECT_STATE.md` — top-level tracking documents.

Some documents describe proposed or incomplete work. Their status is authoritative only when the document explicitly says that the feature is implemented and the code and tests agree.
