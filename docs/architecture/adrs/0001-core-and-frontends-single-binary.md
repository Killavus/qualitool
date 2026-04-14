# ADR-0001: Core library with multiple frontends in a single binary

**Status:** Accepted
**Date:** 2026-04-14

## Context

Qualitool must support multiple frontends against a single source of truth for analysis logic:

- **CLI** for local consultant use and CI execution.
- **Web UI** for interactive exploration during engagements.
- Potentially more (editor plugins, report generators) over time.

The consultant workflow demands fast bootstrap (one binary, zero deployment), CI compatibility (exit codes, JSON output), and a trust-preserving story (no long-running daemons, no network unless explicitly requested). Any design that splits core logic across multiple binaries or couples the Web frontend to a specific runtime (e.g., Node FFI into the core) adds deployment surface and makes version drift possible.

## Decision

- The core is a Rust library crate: `qualitool-core`.
- A single Rust binary `qualitool` statically links the core and exposes every frontend as a subcommand:
  - `qualitool run` — execute a preset or specific checks
  - `qualitool list` — list available probes, checks, presets
  - `qualitool serve` — start an HTTP + WebSocket server backing the Web UI
  - `qualitool info` — show resolved config, active policies, agent status
  - `qualitool config` — manage config files and overlays
  - `qualitool cache` — inspect and clean the cache
- The Web UI (`packages/web-ui`, React + TypeScript) talks to `qualitool serve` over HTTP/WebSocket. It is not linked into the core via FFI.
- No separate server binary. No NAPI-RS bindings. No long-running daemon in the default install.

## Consequences

**Positive:**
- One binary to distribute, one version to track; frontends always see the same core version.
- CI use case is trivial: `qualitool run --format json`, read exit code, read JSON from stdout.
- The Web UI is opt-in — the user explicitly starts `qualitool serve` when they want it.
- Future frontends follow the same pattern: another subcommand on the same binary.

**Negative:**
- Web UI development requires the Rust toolchain to run the backing server locally (mitigated by shipping `qualitool` as a prebuilt dev-dependency binary in `packages/web-ui`).
- The `qualitool` binary size grows with every frontend added (acceptable for now; revisit if it exceeds ~50 MB).

## Alternatives Considered

- **Separate `qualitool-server` crate and binary**: rejected, more deployment surface for no benefit; version skew risk between `qualitool-cli` and `qualitool-server`.
- **NAPI-RS bindings, core linked directly into Node**: rejected, couples Web UI to a Node runtime version, complicates local debugging, and drags in a second toolchain for the Web developers.
- **HTTP-only core exposed by a single server binary with the CLI as a thin HTTP client**: rejected, breaks CI ergonomics (server must be started before CLI commands) and conflicts with the local-first trust posture.
