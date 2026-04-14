# ADR-0006: Layered configuration model

**Status:** Accepted
**Date:** 2026-04-14

## Context

Consultants run qualitool against many client repos across engagements. Each engagement has different conventions, thresholds, and preset preferences. At the same time, consultants carry personal defaults between engagements — a preferred CLI output style, a favorite agent configuration, customized severity thresholds.

There are three distinct concerns, and they must not collide:

1. **Consultant's personal defaults** — things the consultant wants to apply to every engagement by default.
2. **Project's declared configuration** — what the client's repo (or a teammate on the same engagement) says the project needs.
3. **Per-engagement personal overrides** — tweaks the consultant makes locally for a specific engagement that should not bleed into other engagements and should not pollute the client's repo.

## Decision

Three configuration layers, deep-merged in priority order (lowest to highest):

### Layer 1 — Global defaults
**Location**: `$XDG_CONFIG_HOME/qualitool/config.toml` (falls back to `~/.config/qualitool/config.toml` if `XDG_CONFIG_HOME` is unset).

Holds the consultant's personal defaults across all engagements. Typical contents: preferred agent configuration, default output format, personal severity thresholds, default presets.

### Layer 2 — Per-project
**Location**: `<project-root>/qualitool.toml`

Holds the project's declared configuration. Committed to the client repo (or stored alongside it). Typical contents: project-specific preset definitions, Check parameter overrides that reflect the client's conventions, declared project name, excluded paths.

### Layer 3 — User overlay (highest priority)
**Location**: `$XDG_CONFIG_HOME/qualitool/overlays/<repo-hash>.toml`

Per-engagement personal overrides. Scoped to a specific project by its repo content hash, so overrides cannot leak across engagements. Typical contents: personal tweaks the consultant makes locally ("I want to bump this threshold for this engagement"), agent overrides for this specific engagement, temporary Check disables.

### Merge semantics

- **Deep merge**: nested TOML tables merge key-by-key. Arrays replace wholesale (no element merging).
- **Later wins**: Layer 3 overrides Layer 2 overrides Layer 1.
- **CLI flags override all layers**: explicit command-line flags sit above the merge result.
- **Traceability**: `qualitool config --show-sources` prints the effective config with each field annotated by the layer that supplied it.

### Repo hash for overlay scoping

The repo content hash used to scope Layer 3 is derived from:
- The absolute project root path.
- The current Git HEAD commit hash (or, for non-Git projects, a stable hash of the file tree).
- The qualitool schema version (to invalidate overlays after breaking config changes).

This is the same hash used by the cache (see ADR-0007) so overlays and cache entries are co-scoped.

## Consequences

**Positive:**
- Consultants can carry global preferences across engagements, respect client conventions per-project, and make personal tweaks without dirtying the client's repo.
- Overlay scoping by repo hash prevents unintended cross-engagement state (tweaks from client A never apply to client B).
- `--show-sources` makes debugging "why is this field set to X?" tractable.
- The overlay file is personal and local — it never needs to be committed anywhere.

**Negative:**
- Debugging the effective config requires understanding three files and a merge. Mitigation: `qualitool config --show-sources`.
- Overlay files accumulate under `~/.config/qualitool/overlays/` without automatic eviction. Mitigation: `qualitool config clean` as a manual command.
- The repo hash changes on every HEAD commit, which would normally invalidate overlays. This is deliberate: overlays are per-commit to prevent overlays authored against an old codebase from silently applying to a refactored one. Consultants who want sticky overlays can edit the overlay path manually.

## Alternatives Considered

- **Single config file**: rejected. No way to separate "my global preferences" from "this project's declared config" from "my personal tweaks for this engagement." Forces consultants to either pollute the client repo or pollute their global config with engagement-specific state.
- **Two layers (global + per-project only)**: rejected. Forces per-engagement overrides into either the global file (pollution across engagements) or the project file (pollution into the client repo).
- **Single global overlay file applying to every repo**: rejected. Mixes state across engagements; an override for client A silently applies to client B.
- **Project file as highest priority (inverted ordering)**: rejected. The consultant is the operator and their overlay represents their current intent for this engagement; the project file represents what someone (possibly themselves, possibly a teammate, possibly committed long ago) wrote in the repo. Consultant intent should win. If the project needs to enforce constraints, that belongs to a future "policy" mechanism (deferred; see the AI-checks ADR for the shape of what was considered).
