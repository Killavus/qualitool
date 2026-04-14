# ADR-0010: AI-backed Checks via consultant-owned agents (effects-as-data)

**Status:** Accepted
**Date:** 2026-04-14

## Context

Many high-value analyses are best served by an LLM reasoning over structured data — architectural smell detection, naming consistency review, implicit convention discovery. Qualitool must support LLM-backed Checks as a first-class capability. At the same time:

1. Qualitool must not ship an LLM provider integration, because that would bind it to one or more specific providers, drag in API-key management, cost tracking, rate limiting, and provider-specific code paths that the core has no business owning.
2. Consultants already have a preferred agent configured — Claude Code, aider, Cursor, ollama, a custom shell script — with credentials, model choice, and rate limits already set up. Reinventing that layer is waste and creates a second trust boundary.
3. The effects of an LLM call (network I/O, external credentials, client data leaving the machine) must be loud and auditable, not hidden inside a Check implementation.

## Decision

### Consultant brings their own agent

Qualitool does not embed any LLM client. The consultant configures one or more **agents** — subprocess commands that qualitool invokes with structured input and whose structured output is consumed as a Check's Findings. The consultant's agent owns credentials, model selection, provider choice, rate limits, and cost tracking. Qualitool never touches any of those concerns.

### Effects as data

Check execution produces an **action**, not direct Findings. The action is one of:

```rust
enum CheckOutput {
    Findings(Vec<Finding>),     // terminal: Check is done, here are the Findings
    CallAgent(AgentRequest),    // the core fulfills this; the validated response
                                 // becomes the Check's Findings
}

struct AgentRequest {
    agent_hint: Option<String>,       // None → use ai.default_agent from config
    prompt: String,                   // rendered from a prompt template
    include_probes: Vec<ProbeRef>,    // which probe outputs to pass through
    response_schema: JsonSchema,       // must match the Check's declared payload schema
    constraints: AgentConstraints,     // max_tokens, timeout override, read_only flag
}
```

This is a **single-turn effect**. A Check emits one action. If the action is `CallAgent`, the core invokes the agent, validates the response against `response_schema`, and the validated response *is* the Check's Findings. The Check is not re-invoked after the agent call. A Check that needs multi-step reasoning must be decomposed into two Checks composed through an explicit DAG edge (per ADR-0005).

### Agent registry

Agents are declared in config (layered per ADR-0006):

```toml
[agents.default]
command = "claude"
args = ["--print", "--output-format", "json"]
input_mode = "stdin-json"
output_mode = "stdout-json"
timeout_seconds = 120
max_concurrent = 3
read_only = true

[agents.fast]
command = "aider"
args = ["--yes", "--no-git", "--message-file", "{prompt_file}"]
input_mode = "prompt-file"
output_mode = "stdout-json"
timeout_seconds = 30

[ai]
default_agent = "default"
```

Input modes supported in v1:
- `stdin-json` — the canonical envelope `{prompt, probes, response_schema, constraints}` written to the agent's stdin as JSON.
- `stdin-prompt` — free-text prompt on stdin; probes serialized into the prompt text.
- `prompt-file` — envelope written to a temp file; the file path is substituted into `args` at the `{prompt_file}` placeholder.
- `args-prompt` — prompt substituted directly into an `args` entry (for single-shot CLIs that take a prompt as a command-line argument).

Output modes supported in v1:
- `stdout-json` — single JSON document on stdout matching `response_schema`.
- `stdout-json-extract` — first JSON block extracted from free-text output (fallback for agents that wrap JSON in chatter).

Adapters for any agent that does not fit one of these modes are written as thin shell wrappers by the consultant.

### qualitool-agent module

A dedicated core crate `qualitool-agent` owns:

- Subprocess spawn with the configured command and args.
- Writing the input envelope according to `input_mode`.
- Reading and parsing the output according to `output_mode`.
- Validating the output against the Check's `response_schema`.
- One retry on schema validation failure, with a correction prompt ("your previous response did not match the required schema; please return exactly this schema: ..."). After one retry, failure is surfaced as a typed error Finding under the Check's id.
- Per-agent concurrency queue (default `max_concurrent = 3`).
- Timeout enforcement.
- A `host.agent.complete` JSON-RPC method exposed to subprocess extensions, so third-party Checks can request agent calls without seeing agent credentials or raw output.

The core has zero LLM-provider code. No `anthropic`, `openai`, `ollama`, `reqwest` HTTP clients, no API-key handling, no token counting, no cost tracking. All of that belongs to the consultant's agent.

### Prompt authoring

Default prompts ship as plain text files (`prompts/<check_id>.md`) alongside the Check's source code, loaded via `include_str!` in Rust or equivalent in other languages. Presets can override the prompt per-Check by setting `checks.<id>.prompt_file = "..."` or inlining `checks.<id>.prompt = "..."` in the preset TOML. Prompt tuning is the iterative part of AI-check authoring; keeping prompts in text files means consultants can PR prompt improvements without writing Rust.

### Probe data passing

Probe outputs are passed to the agent **as structured JSON alongside the prompt**, not interpolated into the prompt text. The envelope written to the agent's stdin looks like:

```json
{
  "check_id": "architecture-smell",
  "prompt": "Analyze the following project for architectural smells...",
  "probes": {
    "git-history": { "...typed probe output..." },
    "dependency-graph": { "...typed probe output..." }
  },
  "response_schema": { "...JSON schema..." },
  "constraints": {
    "read_only": true,
    "max_tokens": 8000
  }
}
```

The prompt refers to probe data by name ("see `probes.git-history.recent_commits`") rather than string-interpolating it. No template engine dependency; no string-escaping edge cases.

### Data-sensitivity consent

On the first use of a preset that will invoke an agent, qualitool prints a one-time consent banner:

```
This preset will run 4 checks that send data to agent `default` (`claude --print`).
Probes involved: dependency-graph (no source), git-history (no source),
ast (contains source excerpts from ~47 files).

Continue? [y/N]
Remember for this preset? [y/N]
```

If the consultant accepts "remember", the consent is persisted in the user overlay (ADR-0006) keyed by the preset id. Subsequent runs do not prompt until the consultant revokes with `qualitool config revoke-consent <preset>`.

### Caching

No caching of agent output in v1. Agents typically have their own caches (Claude Code, for example, caches prompts at the SDK level), and deterministic caching on non-deterministic output is fragile. Probe caches (ADR-0007) cover the expensive parts; re-invoking the agent on a re-run is acceptable cost.

## Consequences

**Positive:**
- Qualitool has zero LLM provider code, zero secrets management, zero cost tracking. Minimal trust surface and minimal maintenance burden.
- Consultants use whatever agent they already trust, already have credentials for, and already know how to configure.
- Effects-as-data makes Checks trivially unit-testable: given probe inputs, assert the emitted `CallAgent` request has the expected prompt and schema. No subprocess needed in tests.
- All agent invocations flow through one choke point in `qualitool-agent` — single audit log, single rate limiter, single sensitivity banner, single retry logic.
- Subprocess extensions get agent access via `host.agent.complete` without ever seeing credentials or raw agent output.

**Negative:**
- Consultants without any configured agent cannot use AI-backed Checks. Qualitool ships no built-in fallback.
- Output quality depends entirely on the consultant's chosen agent. A weak agent produces weak Findings; qualitool cannot compensate.
- Agents that do not support structured output natively need a wrapper shim to produce schema-valid JSON. For aider, ollama, and similar tools, this is ~20 lines of shell but it is homework the consultant must do.

## Deferred

**Per-project AI policy enforcement** (locking agent classes, forbidding specific probes in prompts, requiring local-only agents, requiring read-only mode) was designed during the Q&A and deliberately deferred to a later ADR. The shape is known: policy is a filter on the resolved agent config, projects declare constraints in `qualitool.toml`, user overlay overrides cannot silently bypass policy, and violations produce named errors with an audit trail. It is not part of v1.

## Alternatives Considered

- **Built-in LLM client per provider (Anthropic, OpenAI, Ollama)**: rejected. Duplicates work the consultant's agent already does, takes on secrets/cost/rate-limit concerns forever, and forces qualitool to chase provider API changes.
- **One embedded provider chosen by the maintainer**: rejected. Picks a winner in a market that still churns quarterly; excludes consultants who have already standardized on a different tool.
- **Multi-turn Check-to-agent loops where the Check can call the agent multiple times**: rejected for v1. A single-turn effect keeps the scheduler's view of each Check as one DAG node and avoids re-entrancy complexity. Compositions of Checks cover multi-step needs.
- **AI-checks as a separate primitive category (not regular Checks)**: rejected. Creates a second type hierarchy for no benefit. AI-checks are regular Checks that happen to emit `CallAgent` instead of `Findings`.
