בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Task #512: resumption-ticket channel-binding — design decision

Finding 1d from `docs/dev-artifacts/audits/2026-07-06-security-network-surface.md`.
An earlier attempt (task #495) added a raw equality check between
`TicketPlain::channel_binding_at_auth` and the new connection's
`channel_binding_now` in `crates/shamir-connect/src/server/resume.rs` —
this was reverted after it broke every legitimate cross-connection
resume (confirmed by two failing e2e tests). The revert's inline comment
already lays out why the naive fix is wrong; this doc completes the
investigation and records the decision.

## The problem, precisely

A resumption ticket lets a client skip a full SCRAM handshake on
reconnect. The audit's concern: a ticket is a bearer credential — if
stolen (e.g. exfiltrated from client storage, or intercepted before TLS
termination somewhere it shouldn't be), an attacker can present it on
THEIR OWN TLS connection and resume the victim's session. Binding the
ticket to something that proves "the SAME connection/channel that
completed the original auth" would close this — the audit's suggested
mechanism was TLS channel binding (RFC 9266's `tls-exporter`).

## Why raw exporter-value equality cannot work here

Per RFC 9266 §4, a `tls-exporter` channel-binding value is derived from
TLS 1.3 key material that is **unique per connection**, including across
session resumption (PSK-based 0-RTT/1-RTT reconnects intentionally derive
fresh exporter material so that channel-binding data from an OLD
connection can never be replayed on a NEW one — this is a deliberate
RFC 9266 security property, not an oversight). The whole POINT of
ticket-based resume in this codebase is "let a legitimate client skip
full auth on a brand-new TLS connection" — which by construction always
has a DIFFERENT exporter value than the connection that minted the
ticket. Requiring equality therefore rejects every legitimate resume
exactly as readily as it rejects a stolen-ticket replay: it cannot
distinguish the two cases at all. This is a hard cryptographic fact, not
an implementation bug to work around.

## What WOULD work, and why it's not available here

The standard way to bind a bearer credential to a channel identity that
DOES persist across reconnects is to anchor it to a stable identity
proven by the TLS layer itself — the canonical case is **mutual TLS**:
bind the ticket to the client certificate's public key (or a hash of
it), which is presented and verified fresh on every connection but is
the SAME key across reconnects, unlike the per-connection exporter.

**This codebase does not implement mutual TLS / client certificates.**
`BindingMode` (`crates/shamir-connect/src/common/types.rs`) only has
three variants — `None`, `TlsExporter`, `TlsNoExport` — none of which
carry a persistent client identity; they only describe transport
STRENGTH (is there TLS at all, and does the client have exporter API
access). Adding a genuine identity-anchored ticket binding would require
building mutual-TLS support (client cert issuance/verification, a new
`BindingMode` variant, wiring through the connection layer) — a
substantial new capability, not a fix to this finding. It is explicitly
out of scope for this task.

## What's already in place, and why it's a reasonable mitigation

`crates/shamir-connect/src/server/resume.rs`'s `ConsumedCounterStore`
(`try_advance`, ~line 45) already enforces a **strictly monotonic,
durably-persisted, per-(user_id, ticket_family_id) counter**: every
successful resume issues a NEW ticket with `family_counter + 1`, and any
attempt to resume with a counter ≤ the last-observed value is rejected.
This is a first-use-wins, ticket-ROTATION design (not merely a
freshness check) with a specific security property: if an attacker
steals a ticket and uses it before the legitimate client does, the
legitimate client's SUBSEQUENT resume attempt (with the now-stale
counter) is rejected — the theft causes a loud, immediately-detectable
failure for the legitimate party, not a silent co-existence. This
significantly bounds the blast radius of ticket theft independent of
channel binding: at most ONE party (attacker or legitimate client,
whoever resumes first) gets to use a given ticket-family generation
before the other is locked out and forced to re-authenticate (or raise
an incident).

Combined with:
- `check_anti_downgrade` (already correct, untouched) — prevents a
  resumed connection from silently downgrading binding-mode strength
  relative to the original auth (e.g. auth'd over `TlsExporter`, resume
  can't claim `None`).
- Ticket TTL (`resumption_expires_at_ns`) — bounds the exposure window
  of a stolen ticket in time.

...the system already has real, complementary mitigations for
ticket-theft risk that do not depend on (and are not weakened by the
absence of) TLS-exporter equality.

## Decision

**No code change for this finding.** Channel-binding-by-exporter-equality
is not a valid fix for this codebase's threat model — RFC 9266 makes it
cryptographically impossible to distinguish "legitimate resume" from
"stolen ticket replayed on attacker's connection" via exporter equality,
since both produce a fresh, non-matching exporter value by design. The
existing monotonic-counter ticket rotation is the correct mitigation
class for this specific residual risk (bearer-credential theft), and
`check_anti_downgrade` already covers the adjacent binding-STRENGTH
downgrade concern the audit also raised.

**If a stronger guarantee is genuinely required in the future**, the
correct next step is a NEW feature — mutual TLS with client-certificate-
anchored ticket binding — scoped and designed as its own project, not
retrofitted as a fix to this finding. Filing that as a separate,
explicitly-optional follow-up rather than blocking this finding's
closure on it.

This closes task #512 as "investigated, fix confirmed infeasible as
originally conceived, existing mitigations documented as sufficient for
the current threat model."
