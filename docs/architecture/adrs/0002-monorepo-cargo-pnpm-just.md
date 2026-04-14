# ADR-0002: Monorepo layout with cargo workspaces, pnpm workspaces, and just

**Status:** Accepted
**Date:** 2026-04-14

## Context

The project spans two ecosystems:

- **Rust**: core, CLI, built-in probes and checks, extension SDK, agent runtime.
- **TypeScript**: Web UI, extension SDK, generated protocol types.

A dedicated monorepo tool could provide cross-language task caching and unified orchestration. Candidates evaluated: Nx, Turborepo, Bazel/Pants, Moon, and the "native tooling + task runner" combination.

Key project constraints:
- Rust is the primary language; the core and most analysis logic live there.
- Expected scale is small: fewer than 10 Rust crates and fewer than 5 TypeScript packages for the foreseeable future.
- The maintainer is a single consultant; developer experience and bootstrap time matter more than fine-grained caching of cross-language tasks.

## Decision

Use each ecosystem's native tooling, with `just` as the top-level command runner:

- **Rust**: `cargo` workspaces. Root `Cargo.toml` declares `members = ["crates/*"]`.
- **TypeScript**: `pnpm` workspaces. Root `pnpm-workspace.yaml` declares `packages: ["packages/*"]`.
- **Top-level commands**: `justfile` at repo root with targets like `just build`, `just test`, `just run cli -- run`, `just codegen`.

No dedicated monorepo tool (Nx, Turborepo, Moon, Bazel) in v1.

Directory convention:
- `crates/` — all Rust crates
- `packages/` — all TypeScript packages
- `presets/` — shipped preset TOML files (language-neutral)

## Consequences

**Positive:**
- Each ecosystem uses its best-in-class native tooling. `cargo` owns Rust incremental builds; `pnpm` owns TypeScript dependency resolution.
- Zero lock-in. Migrating to Moon (the likely graduation target if the project outgrows this setup) is a purely mechanical change.
- The `justfile` is a single-screen contract for every common workflow — new contributors read one file and know how to build, test, run, and generate code.

**Negative:**
- Cross-language task dependencies (e.g., "regenerate TypeScript types when `qualitool-protocol` changes") are declarative inside `just` rather than automated by a monorepo tool. Missed regenerations are caught by CI rather than by the build system locally.
- No cross-language build cache. A TypeScript test run doesn't know it can skip when only Rust files changed; CI will re-run TypeScript tests on every Rust change.

## Alternatives Considered

- **Nx**: rejected. The Rust plugin (`@nx/rust`) is experimental and has not been a priority for the Nx team. Nx treats non-JavaScript code as an opaque shell task, losing cargo's incremental build intelligence. Ceremony and configuration overhead are high for a small repo.
- **Turborepo**: rejected. Same root problem as Nx — wraps `cargo build` as an opaque task and loses cargo's incremental caching, test selection, and cross-crate intelligence. For a Rust-core project, Turborepo is actively worse than no tool at all.
- **Bazel / Pants**: rejected. Both are polyglot-first and would give real cross-language caching, but both carry significant DX and learning-curve costs that vastly exceed the needs of a single-maintainer project under 20 modules.
- **Moon**: the strongest alternative and the likely migration target if the project grows. Written in Rust, treats Rust and TypeScript as first-class polyglot citizens, supports cross-language task caching. Rejected for v1 because it adds a tool to learn and configure for benefits that only materialize at larger scale. The cargo+pnpm+just approach is a strict subset — graduation is mechanical.
