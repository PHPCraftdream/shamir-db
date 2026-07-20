# Changelog

All notable changes to ShamirDB will be documented here.

The project is currently in alpha and has not published a stable release. Until the first tagged release, the `master` branch is the primary development line and compatibility is not guaranteed.

## Versioning scheme

During the alpha phase this project uses `MAJOR.MINOR.PATCH-alpha.N` (for example `0.1.0-alpha.1`). The `alpha.N` suffix is a SemVer pre-release identifier — incrementing `N` does **not** imply any compatibility guarantee between two alpha releases.

> ⚠️ **Alpha-to-alpha compatibility: NOT guaranteed.** The on-disk storage format, the wire protocol, and the public API (Rust, TypeScript, and WASM) MAY change incompatibly between any two `0.1.0-alpha.N` releases. There is **no supported in-place upgrade path** between alpha versions yet — upgrading between alphas may require an **export/import cycle** (dump data with the old version, re-import with the new one). Treat each alpha as throwaway and keep independently verifiable backups. This is the single most important operational fact for an early adopter.

This file follows [Keep a Changelog](https://keepachangelog.com) conventions loosely and will adhere to [Semantic Versioning 2.0.0](https://semver.org) pre-release semantics once stable releases begin.

## [Unreleased]

- Ongoing development of the Rust workspace, server, clients, storage backends, query engine, authenticated transports, and WASM integration points.
- Release infrastructure: version all workspace crates at `0.1.0-alpha.1`, add `publish = false` to every crate as an accidental-publish safety net, and introduce this CHANGELOG with the alpha versioning scheme and the no-alpha-to-alpha-compatibility statement above.
