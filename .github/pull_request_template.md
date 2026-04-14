# Pull Request

## Overview

<!--
What does this PR do, and why? One or two short paragraphs.
Focus on intent and user-visible effect, not a file-by-file recap.
-->

## Linear issue

<!--
Link the Linear issue this PR implements, e.g.:

- [QUAL-123 — Short title](https://linear.app/…)

One issue per PR is strongly preferred. If this PR closes the issue, prefix
the link with `Closes` so Linear auto-transitions it.
-->

## Non-trivial changes to existing code

<!--
List notable edits to code that existed before this PR. Purely additive
changes (new files, new modules, new functions that nothing previously
called) do NOT need to be listed — this section is for modifications to
existing behavior, semantics, signatures, or invariants.

For each item: path + one line on what changed and why.

Example:
- `crates/qualitool-core/src/scheduler.rs` — `run()` now walks the DAG in
  topological order instead of insertion order; required so the new parallel
  executor sees a stable dependency frontier.
- `packages/web-ui/src/api/client.ts` — `fetchFindings()` now throws on
  non-2xx instead of returning `null`; callers updated.

If there are no non-trivial changes, write: _None — additive only._
-->

## How this was tested

<!--
Be explicit. Reviewers should be able to reproduce the validation.

If covered by the existing test suite, a command is enough:

- `cargo test -p qualitool-core`
- `just test`
- `pnpm --filter web-ui test`

For manual / integration testing, give numbered repro steps and the
expected result:

1. `just build`
2. `./target/debug/qualitool run --preset quickscan --project ./fixtures/sample-repo`
3. Expect: exit code 0, JSON on stdout containing at least one finding of
   kind `churn-hotspot`.

If the change is UI-facing, say which browser / flow you exercised.
-->

## ADRs taken into account

<!--
Which Architecture Decision Records shaped this implementation, and how
did they constrain or direct the code? One bullet per relevant ADR.
Link to the ADR file, then a one-line note on its concrete effect here.

Example:
- [ADR-0004 — Extension boundary via subprocess and JSON-RPC](docs/architecture/adrs/0004-extension-boundary-subprocess-jsonrpc.md):
  new probe loader dispatches through the JSON-RPC transport; no
  in-process extension calls were added.
- [ADR-0010 — AI-backed Checks via consultant-owned agents](docs/architecture/adrs/0010-ai-checks-bring-your-own-agent.md):
  the new `semantic-duplication` Check emits `CheckOutput::CallAgent`
  rather than calling an LLM directly; no API keys touched.

If no ADRs applied, write: _None._
-->
