# ADR-0005: DAG-based parallel scheduler for Probes and Checks

**Status:** Accepted
**Date:** 2026-04-14

## Context

A preset typically runs a dozen or more Probes and Checks against a client repo. Many of them are independent: `git-history` and `file-tree` do not block each other, and dozens of Checks can run against the same cached Probe outputs in parallel. Sequential execution wastes CPU on multi-core machines. Unconstrained parallelism risks thrashing disk I/O and exceeding external rate limits (notably for agent-backed Checks, which hit shared API quotas).

Consultants especially care about **cold-start latency** on the first run against a fresh client repo, where nothing is cached and total runtime is dominated by the Probe layer.

## Decision

### Execution graph

Probes and Checks form a directed acyclic graph:
- **Nodes**: Probe invocations and Check invocations.
- **Edges**: dependencies declared in each node's manifest. A Check that consumes `git-history` depends on the `git-history` Probe node. A Check that composes another Check depends on that Check's node. A Probe that derives data from another Probe (e.g., `dependency-graph` reading `package-manifests`) depends on that Probe's node.

Probes may depend on other Probes. This was an explicit question during design — the answer is yes, for two reasons:
1. Many Probes naturally derive from more primitive Probes (dependency graphs from manifest parsers, symbol indices from AST parsers).
2. The DAG already handles dependencies; reusing it for Probe-to-Probe edges is free and avoids forcing Checks to re-run derived computations that another Check could share.

Probes remain pure-data (no Findings, no judgment); Checks remain the only layer that produces Findings.

### Scheduling

The core scheduler (`qualitool-core`) executes the DAG with the following rules:

- **Topological order with parallelism**: nodes whose dependencies are satisfied are eligible to run. The scheduler runs up to `max_parallelism` eligible nodes concurrently.
- **Default max parallelism**: `num_cpus() - 1` (leaves one core for the main thread and OS).
- **Probe cache sharing**: a Probe invoked by multiple Checks runs once; its output is shared across the DAG.
- **Cycle detection at preset load time**: cycles in the dependency graph are detected before execution begins, not at runtime. A cyclic preset fails to load with a clear error naming the cycle.

### Agent-backed Checks

Checks that emit a `CallAgent` effect (see ADR-0010) do not contend for the scheduler's general parallelism budget. Instead they go through a **per-agent queue** with its own concurrency limit (default `max_concurrent = 3`, configurable per agent). This ensures:
- Agent-backed Checks respect the external rate limit of whichever agent the consultant configured.
- Non-agent Checks continue to run freely in parallel while agent calls are queued.
- Different agents have independent queues — ten Checks calling `agents.fast` do not block three Checks calling `agents.default`.

### Failure semantics

- A failed Probe fails all dependent Checks with a structured "dependency failed" error. The scheduler continues executing nodes that do not depend on the failure.
- A failed Check does not cascade unless another Check depends on it.
- The run is reported as failed (non-zero exit code) if any node failed.

## Consequences

**Positive:**
- Cold-start latency is dominated by the longest *path* through the Probe DAG, not the total Probe time. On a large repo this is a major win.
- The scheduler model maps cleanly to the effects-as-data pattern (ADR-0010): the effect handler for `CallAgent` is just another scheduler step.
- Cycle detection at load time catches misconfigured dependency graphs before any work starts.
- Probe-to-Probe dependencies let authors factor shared computation without duplication.

**Negative:**
- Scheduler complexity is higher than a sequential runner (topological ordering, cycle detection, per-agent queues, cache sharing).
- Debugging parallel log streams is harder than sequential ones. Mitigation: each node runs with a structured log scope (`check_id` / `probe_id` + invocation ID) so log lines can be filtered and re-ordered post-hoc.
- Progress reporting to the user is inherently nonlinear ("3 of 14 running, 6 waiting, 5 done") rather than a simple progress bar.

## Alternatives Considered

- **Sequential execution**: rejected. Wastes parallelism on multi-core machines and produces unacceptable cold-start latency on large repos. A consultant opening a 500 kLOC codebase for the first time cannot wait for sequential Probe runs.
- **Unbounded parallelism (fork-join without limits)**: rejected. Saturates disk I/O, blows memory on Probes that hold AST data, and instantly exceeds agent rate limits.
- **Strict Probe-as-leaves model (no Probe-to-Probe dependencies)**: rejected. Forces Checks to duplicate derived computations or forces Probe authors to produce overbroad outputs. The DAG already supports arbitrary dependencies; restricting Probes to leaves adds complexity for no benefit.
