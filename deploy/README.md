# Deployment artefacts

This directory packages everything an operator needs to run `shamir-server`
on a production host.

## Files

| File | What it is |
|------|------------|
| `shamir-db.service` | systemd unit. Drop into `/etc/systemd/system/` and `systemctl enable --now shamir-db` |
| `Dockerfile` | Multi-stage Docker image (Rust 1.93 bookworm builder → debian:bookworm-slim runtime, ~80 MB) |
| `server.example.ktav` | Annotated config template — copy to `/etc/shamir/server.ktav` and adjust |

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

First boot: a random `bootstrap_token.txt` appears in `data_dir/` AND
in the service log at WARN level. Use it for the first SCRAM login,
then have an admin pre-create operator users via `CreateScramUser`
and delete the bootstrap token file.

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
