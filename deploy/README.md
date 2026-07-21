# Deployment artefacts

This directory packages everything an operator needs to run `shamir-server`
on a production host.

## Files

| File | What it is |
|------|------------|
| `shamir-db.service` | systemd unit. Drop into `/etc/systemd/system/` and `systemctl enable --now shamir-db` |
| `Dockerfile` | Multi-stage Docker image (Rust 1.93 bookworm builder → debian:bookworm-slim runtime, ~80 MB) |
| `server.example.ktav` | Annotated reference config (all fields shown) — copy to `/etc/shamir/server.ktav` and adjust |
| `server.small.example.ktav` | Resource profile for a 1–2 GiB container (≈1.5 GiB budget): Argon2 64 MiB × 6, 500 conns, 32 MiB result cap |
| `server.medium.example.ktav` | Resource profile for a 4–8 GiB container (≈6 GiB budget): Argon2 128 MiB × 12, 2000 conns, 64 MiB result cap |

## Resource profiles

Pick a profile by your container/host RAM budget and start from it instead of
the all-fields reference (`server.example.ktav`):

| Profile | Target RAM | When to pick |
|---------|-----------|--------------|
| `server.small.example.ktav` | 1–2 GiB (sized for ~1.5 GiB) | Single-tenant / dev / small VPS. Argon2 auth-RAM ceiling ≈ 384 MiB. |
| `server.medium.example.ktav` | 4–8 GiB (sized for ~6 GiB) | Small-to-medium production. Argon2 auth-RAM ceiling ≈ 1.5 GiB. |
| `server.example.ktav` | n/a (reference) | Every-field-shown template — copy & trim when neither small nor medium fits. |

**Argon2 sizing formula** (also embedded as a comment at the top of each
profile):

```
argon2_concurrent_max × kdf_defaults.memory_kb (KiB)  ≤  ~25% of your
container/host RAM (KiB).
```

Example: a 4 GiB container → ~1 GiB budget → `memory_kb = 131072` (128 MiB)
allows `argon2_concurrent_max` up to ~8. The two profiles above pin
`argon2_concurrent_max` to ~25% of their respective budgets; if your budget
differs, derive your own pair from the formula and update both fields together.

## Quick start (systemd)

```bash
# 1. User + dirs
sudo useradd --system --no-create-home shamir
sudo mkdir -p /etc/shamir /var/lib/shamir-db /var/log/shamir-db
sudo chown -R shamir:shamir /var/lib/shamir-db /var/log/shamir-db

# 2. Binary + config
sudo cp target/release/shamir-server /usr/local/bin/
sudo cp deploy/server.example.ktav /etc/shamir/server.ktav
# edit /etc/shamir/server.ktav: addrs, listeners, browser_origin_allowlist

# 3. Service
sudo cp deploy/shamir-db.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now shamir-db

# 4. Verify
journalctl -u shamir-db -f
curl -fsS http://127.0.0.1:9090/healthz   # → "ok"
curl -fsS http://127.0.0.1:9090/readyz    # → "ready"
```

First boot: a random bootstrap token is generated and its **path** (never
the token itself) is logged at WARN level. By default it is written to
`data_dir/bootstrap_token.txt` — for backward compatibility only.
**Operators SHOULD instead pass `--bootstrap-token-path
/run/shamir/bootstrap_token.txt`** (tmpfs, per the `RuntimeDirectory=shamir`
directive in `shamir-db.service`) so the token is never captured by a
`backup --to` snapshot of `data_dir`. If left at the default `data_dir`
path, the token **will** be captured verbatim by any `backup --to` run
that happens before it is consumed or expired.

The token auto-deletes itself: the server removes the file and consumes
the record on the **first successful SCRAM login** for the bootstrap
username, or via a **24h TTL** boot-time sweep for any token nobody ever
used — whichever comes first. Manual deletion right after reading the
token and logging in remains a safe, optional immediate step; it is no
longer the primary cleanup mechanism. After logging in, have an admin
pre-create operator users via `CreateScramUser`.

## Quick start (Docker)

```bash
docker build -f deploy/Dockerfile -t shamir-db:dev .

docker run -d --name shamir-db \
  -v shamir-data:/var/lib/shamir-db \
  -v $(pwd)/deploy/server.example.ktav:/etc/shamir/server.ktav:ro \
  -p 7331:7331 \
  -p 7332:7332 \
  -p 7333:7333 \
  -p 9090:9090 \
  --memory 4g --cpus 2 \
  shamir-db:dev

docker exec shamir-db cat /var/lib/shamir-db/bootstrap_token.txt
```

## Backup

```bash
# Stop-and-copy is the supported path. Fjall is journal-based, so a copy
# that races an in-flight append loses only the torn tail batch on next
# open (RecoveryMode::TolerateCorruptTail truncates back to the last
# fully-checksummed batch) — earlier committed batches stay intact — but
# stop-and-copy is the strongest guarantee.
sudo systemctl stop shamir-db
shamir-server --config /etc/shamir/server.ktav backup --to /backups/
sudo systemctl start shamir-db

# Cron daily — BEST-EFFORT LIVE SNAPSHOT (no service stop): recovers, on
# next open, to the last complete journal batch; for a guaranteed-
# consistent snapshot, stop the service first (see the block above).
0 3 * * * /usr/local/bin/shamir-server --config /etc/shamir/server.ktav backup --to /backups/
```

Restore = stop service → `cp -r /backups/<timestamp>/* /var/lib/shamir-db/` → start.

## Scrape with Prometheus

```yaml
# prometheus.yml
scrape_configs:
  - job_name: 'shamir-db'
    static_configs:
      - targets: ['shamir-db.internal:9090']
```

Standard `process_*` series populate Grafana's
[Process Exporter dashboard](https://grafana.com/grafana/dashboards/?search=process)
out of the box.

## Common alerts

```yaml
# Liveness gone
- alert: ShamirDbDown
  expr: up{job="shamir-db"} == 0
  for: 30s
  labels: { severity: critical }

# RAM pressure
- alert: ShamirDbHighMemory
  expr: process_resident_memory_bytes{job="shamir-db"} > 3.5e9
  for: 5m
  labels: { severity: warning }

# Disk filling — alert when audit log dir crosses 80 % of expected
# steady-state for the configured retention
- alert: ShamirDbAuditDiskHigh
  expr: node_filesystem_avail_bytes{mountpoint="/var/lib/shamir-db"} < 5e9
  for: 5m
  labels: { severity: warning }
```

## See also

* `docs/dev-artifacts/ops/CAPACITY_PLANNING.md` — sizing reference: per-unit RAM,
  CPU costs, disk growth rates, recommended VM sizes.
* `docs/dev-artifacts/roadmap/PRODUCTION_HARDENING_ROADMAP.md` — what's still
  outstanding for full production maturity.
