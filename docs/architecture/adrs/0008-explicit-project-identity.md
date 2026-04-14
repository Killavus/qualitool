# ADR-0008: Explicit project identity via --project flag

**Status:** Accepted
**Date:** 2026-04-14

## Context

A client repo might contain multiple independent sub-projects — a Rails app at the root, an iOS companion app under `mobile/`, a worker service under `services/worker/`, each with its own conventions, thresholds, and relevant Checks. Qualitool needs to know what "the project" is for three reasons:

1. To load the correct `qualitool.toml` (per ADR-0006).
2. To compute the right cache key (per ADR-0007).
3. To scope output and Findings to the right project in multi-project runs.

Autodetection by walking upward from the current working directory to find the nearest `qualitool.toml` is seductive but magical. In multi-project repos it produces surprising behavior: running from `services/worker/src/` picks one project, running from `mobile/app/` picks another, and the user has to mentally simulate the walk to know which config is active.

## Decision

Project identity is **explicit**. Users name the project root via a flag; qualitool does not walk the filesystem to guess.

- `qualitool run --project <path>` names the project root explicitly. All other commands that operate on a project (`list`, `serve`, `info`, `cache`, `config`) accept the same flag.
- When `--project` is omitted, the project root defaults to the **current working directory**. Qualitool does not walk upward to find a parent `qualitool.toml`.
- Each project has its own `qualitool.toml` at its root and its own cache key derived from that root path.
- Multi-project repos are handled by running qualitool multiple times with different `--project` arguments, typically from a small shell script or `just` target.

Error handling: if the CWD does not contain a `qualitool.toml`, qualitool errors out with a message like:

```
error: no qualitool.toml found at /clients/acme/services/worker
hint: pass --project to name a project root, or run from a directory containing qualitool.toml
```

No attempt to "helpfully" walk up and find one anywhere.

## Consequences

**Positive:**
- No magic. The user always knows which project is active because they either named it explicitly or are sitting in its root.
- Multi-project repos work out of the box via repeated invocation; no new config schema needed.
- CI scripts are explicit and reproducible: `qualitool run --project ./services/worker --format json`.
- The error message when a user is in the wrong directory is actionable — it tells them exactly what to do.

**Negative:**
- Users running `qualitool run` from a nested directory of a single-project repo will get a confusing "no qualitool.toml found" error. The hint helps, but it is a small ergonomic papercut compared to the autodetection alternative.
- No one-command way to run all sub-projects of a multi-project repo in a single invocation. A `justfile` target is the expected pattern; a future ADR may introduce a `--project-root-glob` or similar if demand materializes.

## Alternatives Considered

- **Walk upward to find nearest `qualitool.toml`**: rejected as the primary mechanism. Magical and non-obvious in multi-project repos. Users cannot predict which config is active without mentally simulating the walk. The consultant workflow values predictability over convenience.
- **Single `qualitool.toml` at repo root with sub-project declarations**: rejected. Complicates the config schema for a use case that is already solvable by "run qualitool per sub-project." If declared sub-projects become a strong pattern later, it can be layered on top of this ADR without breaking it.
- **Environment variable override (`QUALITOOL_PROJECT`)**: neither accepted nor rejected in v1. If autodetection pain becomes real for power users, an env var that supplies the default project root is a minimally invasive addition.
