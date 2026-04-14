# Qualitool — Architecture Overview

## Purpose

Qualitool is a local-first toolkit for software consultants to rapidly onboard to client codebases and assess architecture, code quality, and audit concerns. The target workflow is "first day on an engagement" — forming an informed opinion fast against an unfamiliar repository. It runs against a Git repository or a standalone folder on the consultant's machine, produces structured Findings, and supports both interactive exploration and CI execution.

## Architecture Goals

1. **Local-first.** Runs entirely on the consultant's machine. No telemetry. No network I/O from the core. Client code never leaves the machine unless the consultant explicitly enables an AI-backed preset whose agent performs the network call.
2. **CI-capable.** Single binary, deterministic exit codes, structured JSON output. CI integration is trivial and does not require a long-running server.
3. **Extensible.** New Probes, Checks, and Presets can be authored by maintainers and third-party consultants without modifying the core.
4. **Language-neutral extensions.** Third-party extensions run as subprocesses speaking JSON-RPC; authors can use any language.
5. **Trust-preserving.** The core handles no secrets, no API keys, no LLM credentials. AI capabilities come from the consultant's own pre-configured agent.
6. **Fast cold start.** Parallel execution of independent work. Expensive data-gathering is cached per repo, outside the client repo itself.

## Architecture Diagram

```
┌──────────────────────────────────────────────────────────────────┐
│                       qualitool (single binary)                   │
│                                                                    │
│  ┌───────────┐  ┌───────────┐  ┌───────────┐  ┌─────────────┐    │
│  │ cli run   │  │ cli list  │  │ cli serve │  │ cli info    │    │
│  └─────┬─────┘  └─────┬─────┘  └─────┬─────┘  └──────┬──────┘    │
│        └────────┬─────┴───────┬──────┴───────────────┘           │
│                 ▼             ▼                                   │
│        ┌────────────────────────────────┐                         │
│        │        qualitool-core          │                         │
│        │  DAG scheduler · effect loop · │                         │
│        │  config layering · cache       │                         │
│        └────┬──────────┬──────────┬─────┘                         │
│             ▼          ▼          ▼                               │
│    ┌───────────┐ ┌──────────┐ ┌─────────────┐                    │
│    │  probes   │ │  checks  │ │   agent     │                    │
│    │ (built-in)│ │(built-in)│ │  runtime    │                    │
│    └───────────┘ └──────────┘ └──────┬──────┘                    │
└─────────────────────────────────────┼─────────────────────────────┘
                                      │ subprocess + JSON-RPC
                   ┌──────────────────┼───────────────────────────┐
                   ▼                  ▼                            │
            ┌─────────────┐    ┌──────────────┐                    │
            │ third-party │    │ consultant's │                    │
            │  extensions │    │    agent     │                    │
            │ (subprocess │    │  (claude,    │                    │
            │  + JSON-RPC)│    │  aider,      │                    │
            └─────────────┘    │  ollama…)    │                    │
                               └──────────────┘                    │
                                                                    │
  ┌─────────────────┐   HTTP/WebSocket                             │
  │ packages/web-ui │◀─────────────────── qualitool serve ─────────┘
  │ (React + TS)    │
  └─────────────────┘
```

## Architecture Constraints

- Single binary `qualitool`, distributed as a static Rust build.
- Rust stable toolchain only. No nightly features.
- Cache lives outside client repos at `$XDG_CACHE_HOME/qualitool/<repo-hash>/` (ADR-0007).
- No network I/O from the core. Agent subprocesses may perform network I/O with explicit consent.
- No mutation of client repos. All core operations are read-only.
- Third-party extensions are always subprocess-isolated (ADR-0004).
- No embedded LLM provider code. The core holds no API keys (ADR-0010).

## Module Map

### Rust crates (`crates/`)

| Crate | Responsibility | Depends on |
|---|---|---|
| `qualitool-core` | DAG scheduler, effect loop, config layering, cache, Probe/Check traits | `qualitool-protocol` |
| `qualitool-protocol` | JSON-RPC schemas, wire types, TypeScript codegen source | — |
| `qualitool-agent` | Agent registry, subprocess spawn, envelope I/O, schema validation, per-agent concurrency queue | `qualitool-protocol` |
| `qualitool-probes` | Built-in Probes (git-history, file-tree, ast, dependency-graph, ci-config, …) | `qualitool-core` |
| `qualitool-checks` | Built-in Checks (cyclomatic-complexity, churn-hotspots, dependency-smells, …) | `qualitool-core` |
| `qualitool-sdk` | Helper crate for Rust-native third-party extension authors | `qualitool-protocol` |
| `qualitool-cli` | The `qualitool` binary with subcommands `run`, `list`, `serve`, `info`, `config`, `cache` | `qualitool-core`, `qualitool-probes`, `qualitool-checks`, `qualitool-agent` |

### TypeScript packages (`packages/`)

| Package | Responsibility | Depends on |
|---|---|---|
| `web-ui` | React + TypeScript Web UI; talks to `qualitool serve` over HTTP/WebSocket | `protocol-ts` |
| `protocol-ts` | Generated TypeScript types from `qualitool-protocol` | — (code-generated) |
| `sdk-ts` | Helper package for TypeScript-native third-party extension authors | `protocol-ts` |

### Dependency rules

- Frontends (the `qualitool-cli` subcommands and the Web UI) depend on `qualitool-core`, never directly on `qualitool-agent`.
- `qualitool-checks` does not depend on `qualitool-agent` — Checks emit `CheckOutput::CallAgent` as data, and the core's effect loop routes it to the agent runtime (ADR-0010).
- `qualitool-protocol` is the single source of truth for wire types; both Rust and TypeScript consumers import from it (TypeScript via generated bindings).
- No crate imports from frontends (no upward edges).

## Important Concepts

- **Probe** — Deterministic, read-only data-gathering primitive. Cacheable. See ADR-0003.
- **Check** — Consumes Probe outputs and produces Findings. The judgment layer. See ADR-0003.
- **Preset** — Named, configured bundle of Checks, defined in TOML. See ADR-0003.
- **Finding** — A single reportable observation with a canonical envelope (id, severity, title, location, tags) and a typed per-Check payload. See ADR-0009.
- **Agent** — A subprocess configured by the consultant (e.g. `claude`, `aider`, `ollama`) that fulfills AI-backed Check requests. See ADR-0010.
- **AgentRequest** — An effect emitted by a Check (`CheckOutput::CallAgent`) instead of direct Findings; the core's effect loop invokes the agent and the validated response becomes the Check's Findings. See ADR-0010.
- **Extension** — A third-party subprocess providing additional Probes or Checks, loaded from `.qualitool/extensions/` or `~/.qualitool/extensions/`. See ADR-0004.
- **Overlay** — A per-repo user-side config file at `$XDG_CONFIG_HOME/qualitool/overlays/<repo-hash>.toml` that overrides global and per-project settings for a specific engagement. See ADR-0006.
- **Effect loop** — The core's scheduler pattern: Checks emit actions (`Findings` or `CallAgent`), the core fulfills effects and feeds the result back as the Check's output. See ADR-0010.
- **Repo content hash** — Short SHA-256 derived from project root path + Git HEAD + schema version. Used as the cache key and the overlay scope key. See ADR-0007, ADR-0006.

## Repository Structure

```
qualitool/
├── crates/                          # cargo workspace
│   ├── qualitool-core/
│   ├── qualitool-protocol/
│   ├── qualitool-agent/
│   ├── qualitool-probes/
│   ├── qualitool-checks/
│   ├── qualitool-sdk/
│   └── qualitool-cli/
├── packages/                        # pnpm workspace
│   ├── web-ui/
│   ├── protocol-ts/
│   └── sdk-ts/
├── presets/                         # shipped preset TOML files
├── docs/
│   └── architecture/
│       ├── OVERVIEW.md              # this file
│       └── adrs/                    # ADR-0001 through ADR-0010
├── Cargo.toml                       # cargo workspace root
├── pnpm-workspace.yaml              # pnpm workspace root
├── justfile                         # top-level command runner
└── README.md
```

## Architecture Decision Records

- [ADR-0001: Core library with multiple frontends in a single binary](adrs/0001-core-and-frontends-single-binary.md)
- [ADR-0002: Monorepo layout with cargo, pnpm, and just](adrs/0002-monorepo-cargo-pnpm-just.md)
- [ADR-0003: Probe / Check / Preset as the core analysis primitives](adrs/0003-probe-check-preset-terminology.md)
- [ADR-0004: Extension boundary via subprocess and JSON-RPC](adrs/0004-extension-boundary-subprocess-jsonrpc.md)
- [ADR-0005: DAG-based parallel scheduler for Probes and Checks](adrs/0005-dag-parallel-scheduler.md)
- [ADR-0006: Layered configuration model](adrs/0006-layered-configuration-model.md)
- [ADR-0007: Per-user content-hashed cache location](adrs/0007-cache-per-user-content-hashed.md)
- [ADR-0008: Explicit project identity via --project flag](adrs/0008-explicit-project-identity.md)
- [ADR-0009: Findings envelope with typed per-Check payload](adrs/0009-findings-envelope-typed-payload.md)
- [ADR-0010: AI-backed Checks via consultant-owned agents](adrs/0010-ai-checks-bring-your-own-agent.md)

## Glossary

- **AgentRequest** — Structured request emitted by a Check asking the core to invoke an agent on its behalf.
- **Check** — Analysis primitive that consumes Probe outputs and emits Findings or an AgentRequest.
- **Effect loop** — Scheduler pattern translating Check actions into fulfilled effects before reporting Findings.
- **Extension** — Third-party Probe or Check provider running as a subprocess, discovered from well-known directories.
- **Finding** — Canonical reportable observation with envelope + typed payload.
- **Overlay** — Per-repo user-side config file overriding global and per-project settings.
- **Preset** — Named, configured bundle of Checks, stored as TOML.
- **Probe** — Deterministic, read-only data-gathering primitive. Cacheable.
- **Repo hash** — Short content hash used as cache key and overlay scope key.
