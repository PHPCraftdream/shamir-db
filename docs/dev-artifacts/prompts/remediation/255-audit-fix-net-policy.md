# 255 — audit-fix-net-policy (P1 audit follow-up: G4 explicit non-routable IP policy, RV-WASM-HOST-E62F8E2C)

Source: post-merge audit of PR #326 (P1 G4) + @oh design decision (2026-10-29). Today `crates/shamir-wasm-host/src/gateway/net_gateway.rs` ~494-524 (`is_private_or_loopback_ip`) blocks loopback/private/link-local and exactly `0.0.0.0`; CGNAT, benchmark, multicast, reserved, documentation ranges etc. have no policy, so e.g. `check_host_allowed("100.64.0.1", ["*"])` succeeds. The card requires an EXPLICIT policy (plan row G4, P1.md ~6222). Read `CLAUDE.md`, `net_gateway.rs` (lines ~14-20 doc, 99-135 `check_host_allowed`, 191-247 `check_url_allowed_resolved`, 335-340, 484 mapped-v4 fold), `crates/shamir-db/src/shamir_db/gateway/curl_gateway.rs` (~70 redirects, ~110 curl config, 242-256 resolve pins), `gateway/tests/ssrf_tests.rs`.

## Design (decided - implement exactly)
Replace `is_private_or_loopback_ip` by ONE pure function `classify_ip(IpAddr) -> IpClass { Global, Internal, Forbidden }` driven by a constant prefix table, in a NEW file `crates/shamir-wasm-host/src/gateway/ip_policy.rs` (folder goes 5 -> 6 entries; one file = one primary export; declare it in the gateway `mod.rs`).

**Forbidden** (denied always, even for an exact allow-list entry):
- IPv4: `0.0.0.0/8`, `224.0.0.0/4` multicast, `240.0.0.0/4` reserved (includes `255.255.255.255`).
- IPv6: `::/128`, `ff00::/8`, `::/96` IPv4-compatible (classify `::1` BEFORE this rule), `100::/64`.

**Internal** (allowed only by an EXACT allow-list entry, never by `*` or `*.x`):
- IPv4: `127/8`, `10/8`, `172.16/12`, `192.168/16`, `169.254/16`, `100.64/10` (CGNAT), `192.0.0.0/24`, `192.0.2/24`, `198.51.100/24`, `203.0.113/24`, `198.18/15`, `192.88.99/24`.
- IPv6: `::1`, `fe80::/10`, `fc00::/7`, `fec0::/10`, `2001:db8::/32`, `3fff::/20`, `2001::/23` (one block on purpose), `64:ff9b:1::/48`, `5f00::/16`.

**Embedded IPv4:** classify by the embedded address and take the STRICTER class of {outer, embedded}: `::ffff:0:0/96` (already folded at ~:484), `64:ff9b::/96` NAT64 (low 32 bits; `64:ff9b::7f00:1` -> 127.0.0.1 -> Internal), `2002::/16` 6to4 (bits 16-47).

**Decision matrix** (replaces the two checks): wildcard entry -> only `Global`; exact IP literal -> `Global`+`Internal` (no DNS, empty pin, as today); exact HOSTNAME -> NEW: resolve and pin (remove the early return at ~:203-209 that skips DNS), `Global`+`Internal`. If ANY address in a DNS answer fails the check the whole request is denied (keep the existing ANY rule ~:222-230). Every validated IP is pinned through curl `resolve` lines (curl_gateway ~242-256). Redirects are already not followed.

**Proxy hardening (include):** curl honours `*_PROXY` env vars, in which case `--resolve` pins nothing. Add `noproxy = "*"` to the generated curl config (`curl_gateway.rs` ~110).

## Tests (table-driven; NEW `gateway/tests/ip_policy_tests.rs`, wired through the tests manifest)
- For every range: first address, last address, one before, one after; plus embedded forms (`64:ff9b::a9fe:a9fe`, `2002:7f00:1::`, `::ffff:0.0.0.0`).
- Matrix test: class x match kind -> allow/deny.
- `ssrf_tests.rs:14-15` currently asserts exact `0.0.0.0` is ALLOWED: flip it to a deny in the same test function (no test removed).
- Async test: an exact hostname resolving to loopback is allowed and `pinned_ips` is non-empty.
- curl-config test: the `noproxy` line is present.
- Compatibility note (PR body/changelog line in the final report): operators with an exact `0.0.0.0` or `::` entry lose it (intended); exact hostnames now resolve at check time, so failures surface as `NetGatewayError::Dns`.

## Rules
Touch only `shamir-wasm-host` (gateway + tests) and the one `curl_gateway.rs` line in `shamir-db`. No new dependencies (use `std::net`), no `#[allow]`, layout limits, imports at top, `mod.rs` = declarations only.

## Local gates (only these)
`cargo fmt -p shamir-wasm-host -p shamir-db -- --check`; `cargo clippy -p shamir-wasm-host --all-targets -- -D warnings`; `cargo check -p shamir-db --all-targets`; `python -I scripts/ci/check_layout.py`. Real exit codes.
