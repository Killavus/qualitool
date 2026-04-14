# ADR-0003: Probe / Check / Preset as the core analysis primitives

**Status:** Accepted
**Date:** 2026-04-14

## Context

Qualitool needs primitives for (1) gathering data from a client repo, (2) evaluating that data into reportable observations, and (3) bundling configurations so consultants can run a named workflow ("new-engagement", "churn-hotspot", "pre-audit") with a single command.

Earlier proposals considered the terminology **workflow / analysis / task**, but two problems emerged:

1. **"Task" is already taken.** In software it universally means "a unit of execution" — reusing it for "a leaf analysis primitive" creates permanent cognitive friction for anyone reading the docs.
2. **"Analysis" is overloaded.** Making it the mid-tier concept *and* the generic umbrella word for everything qualitool does is ambiguous.

A second, deeper issue: the earlier terminology collapsed data-gathering and judgment into one layer. The critical architectural axis is caching — **data gathering is expensive and cacheable, judgment is cheap and re-runnable**. The primitive layering must reflect that.

## Decision

Three primitives with strict responsibility boundaries:

### Probe
A read-only data-gathering primitive. Reads source files, git history, dependency graphs, CI configuration, package metadata, etc. Produces **typed structured data**. Side-effect-free. Deterministic over a content hash. Cacheable.

Examples: `git-history`, `file-tree`, `dependency-graph`, `ast` (tree-sitter-backed), `ci-config`, `package-manifests`.

### Check
Consumes one or more Probe outputs (and optionally other Check outputs) and produces **Findings**. This is where judgment lives: thresholds, rules, heuristics, scoring, AI-agent calls. Not cached by default (consultants iterate on thresholds and want fresh results).

Examples: `cyclomatic-complexity`, `churn-hotspots`, `dependency-smells`, `ownership-bus-factor`, `architecture-layering`.

### Preset
A named, configured bundle of Checks (and, transitively, their Probe dependencies). A Preset is a TOML file containing Check selections and parameter overrides. No logic.

Examples: `new-engagement`, `churn-hotspot`, `pre-audit`, `security-triage`.

### Responsibility boundaries (strict)

- Probes **never** produce Findings. They produce typed data.
- Checks **never** gather raw data directly. They consume Probe outputs.
- Presets contain **configuration only**. They do not contain logic or code.
- Checks may depend on other Checks (composable).
- Probes may depend on other Probes (see ADR-0005 for the DAG model).
- Built-in Probes and Checks live in `qualitool-probes` and `qualitool-checks` crates and are linked into the core binary. Third-party Probes and Checks come in via the extension boundary (ADR-0004).

## Consequences

**Positive:**
- The caching boundary is self-evident: aggressively cache Probes, never cache Checks. This matches the performance axis that actually matters.
- Probes are trivially parallelizable and sandbox-safe (read-only).
- Separating gathering from judgment makes each layer unit-testable in isolation.
- Terminology is unambiguous: "task" retains its standard meaning, "analysis" becomes an informal umbrella word rather than a load-bearing type name.

**Negative:**
- Forces authors to split their mental model. A simple "grep for X and report" check must be decomposed into a Probe (performs the grep, returns results) and a Check (decides what to report). For trivial one-shot checks, this feels like overhead.
- Two layers of configuration to learn (Probe parameters vs. Check parameters), though in practice most Probes have few parameters.

## Alternatives Considered

- **Workflow / Analysis / Task** (original proposal): rejected on terminological grounds and because it collapses the cache boundary.
- **Two-layer model (Checks only, with "gather" and "evaluate" phases inside each)**: rejected. It makes cache sharing across Checks harder (each Check owns its own gather phase) and couples data collection to judgment in a way that resists reuse.
- **Single-layer model (everything is a "rule")**: rejected. Impossible to cache at the right granularity; every rule re-parses source from scratch.
