# P1 G3 execution briefs (delegated stages for 197-p1-connect-session.md)

Two sequential worker chunks execute the cards of `197-p1-connect-session.md`.
Chunk 1: RV-CONNECT-064F9FE7, B11F48CA, F8A5E821, 226DEDF4, C9066D95, EAB07B19, 34144ECE.
Chunk 2: RV-CONNECT-2A423D73, 60D4DAB1, 94E37810 (after chunk 1 lands).
Each worker carries the compile/build/test policy from AGENTS.md verbatim; allowed
local gates only (check/clippy/fmt scoped -p shamir-connect, layout), CARGO_BUILD_JOBS=2,
one cargo process at a time. No git mutation by workers; the orchestrator commits.
