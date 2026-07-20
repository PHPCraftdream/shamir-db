# Multi-stage build: rust:bookworm-slim builder → debian:bookworm-slim runtime.
#
# Why not alpine for runtime: aws-lc-rs (used by rustls) builds fine on
# musl but the resulting binary needs `libc.musl-x86_64.so.1` and we'd
# need `RUSTFLAGS=-C target-feature=-crt-static` etc. Easier path is
# a tiny debian-slim base — only ~30 MB of OS + ca-certs and curl.
#
# Build:    docker build -f deploy/Dockerfile -t shamir-db:dev .
# Run:      docker run -d --name shamir-db \
#             -v shamir-data:/var/lib/shamir-db \
#             -v $(pwd)/server.ktav:/etc/shamir/server.ktav:ro \
#             -p 7331:7331 -p 9090:9090 \
#             shamir-db:dev

FROM rust:1.93-bookworm AS builder
WORKDIR /build
# Copy the workspace metadata first so layer caching kicks in even when
# only crate sources change (not Cargo.lock / Cargo.toml).
# This is a virtual workspace ([workspace] with no [package] in the root
# Cargo.toml, members = ["crates/*"]) -- there is no root `src/` to copy.
COPY Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo
COPY crates ./crates
RUN cargo build --release -p shamir-server

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*

# Non-root user matching the systemd unit's `User=shamir`.
RUN useradd --system --no-create-home --uid 1000 --user-group shamir

# Pre-create the dirs the server expects (config + data).
RUN mkdir -p /etc/shamir /var/lib/shamir-db /var/log/shamir-db \
 && chown -R shamir:shamir /var/lib/shamir-db /var/log/shamir-db

COPY --from=builder /build/target/release/shamir-server /usr/local/bin/shamir-server

USER shamir

# Convention ports — operator can remap with `-p`. 7331 = TCP+TLS,
# 7332 = WSS native, 7333 = WSS browser, 9090 = observability HTTP.
EXPOSE 7331 7332 7333 9090

# Healthcheck talks to the same observability server the K8s liveness
# probe would; works inside the container without external network.
HEALTHCHECK --interval=10s --timeout=3s --start-period=15s --retries=3 \
  CMD curl -fsS http://127.0.0.1:9090/healthz || exit 1

# Volume the operator will likely mount.
VOLUME ["/var/lib/shamir-db"]

ENTRYPOINT ["shamir-server"]
CMD ["--config", "/etc/shamir/server.ktav"]
