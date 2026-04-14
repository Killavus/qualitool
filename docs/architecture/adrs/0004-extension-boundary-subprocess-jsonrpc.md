# ADR-0004: Extension boundary via subprocess and JSON-RPC

**Status:** Accepted
**Date:** 2026-04-14

## Context

Consultants and maintainers will author third-party Probes and Checks in different languages. Some authors prefer Rust, others TypeScript, others Python or Go. Qualitool must:

1. Support extensions written in any language without binding the core to a specific guest runtime.
2. Keep extensions from escaping the host's process or reading host secrets.
3. Enforce timeouts, memory limits, and kill switches on misbehaving extensions.
4. Not pay subprocess-spawn cost for **built-in** Probes and Checks, which are known to be trusted.

## Decision

### Process model

All **third-party** extensions run as subprocesses. **Built-in** Probes and Checks are linked directly into the `qualitool` binary and do not pay the subprocess cost.

### Wire protocol

JSON-RPC 2.0, framed LSP-style (`Content-Length: N\r\n\r\n<json-payload>`), over the extension's **stdin** (requests from host, responses from extension) and **stdout** (responses to host, notifications from extension). Stderr is captured by the host for logging only.

This framing is chosen because it is:
- Language-neutral (trivial to implement in any language).
- Already in wide use (LSP), so tooling and mental models carry over.
- Compatible with long-running extensions that handle multiple invocations per spawn (future optimization).

### Protocol surface (v1)

Methods from host to extension:
- `extension.describe` → extension declares the Probes and Checks it provides, with their input/output schemas.
- `probe.run` → host asks the extension to execute a specific Probe with given inputs.
- `check.run` → host asks the extension to execute a specific Check with given Probe outputs; extension responds with a `CheckOutput` envelope (`Findings` or `CallAgent` request, see ADR-0010).

Methods from extension to host:
- `host.log` → extension asks the host to log structured data under the extension's name.
- `host.agent.complete` → extension asks the host to fulfill an agent call on its behalf (see ADR-0010). The extension never sees agent credentials or the agent's raw output.

### Extension discovery

Two well-known directories scanned in order, each with "first match wins" semantics by extension name:

1. **Per-project**: `<project-root>/.qualitool/extensions/`
2. **Per-user**: `~/.qualitool/extensions/`

No `$PATH` fallback. No explicit config paths in v1. Extensions are executable files; the filename (minus any platform suffix) is the extension name. Conflicts between per-project and per-user directories are resolved in favor of per-project.

### Sandboxing and limits

- Host enforces a per-invocation timeout (default 60 s, configurable per extension).
- Host captures stderr and surfaces it as structured log events attributed to the extension.
- Host kills extensions that exceed the timeout or fail to respond to a heartbeat.
- Extensions inherit a minimal, scrubbed environment (no `QUALITOOL_*_API_KEY`, no `ANTHROPIC_API_KEY`, etc.). This is enforced even when extensions access agents via `host.agent.complete`.

## Consequences

**Positive:**
- Language-agnostic. Rust, TypeScript, Python, Go, and shell-script extensions all work uniformly.
- Sandboxing falls out of OS process isolation — simple, well-understood threat model.
- Extensions never see agent credentials, user secrets, or environment variables the host scrubs.
- Built-in primitives are free (no subprocess tax) because they skip the extension boundary entirely.

**Negative:**
- 5–20 ms spawn latency per extension invocation. Acceptable because Probes and Checks run on the order of O(once per repo), not O(per function). If any extension becomes hot, the protocol supports long-running extensions handling multiple invocations per spawn (future optimization).
- Extension authors must implement the JSON-RPC protocol. Mitigated by shipping SDKs (`qualitool-sdk` in Rust, `@qualitool/sdk` in TypeScript) that hide the framing and typed envelopes.
- Extension discovery scoped to two directories means authors cannot drop an extension into `$PATH` and expect qualitool to find it. This is a deliberate trade for explicitness.

## Alternatives Considered

- **In-process dynamic library loading (`.so` / `.dylib` / `.dll`)**: rejected. Breaks sandboxing, couples extension ABI to the Rust compiler version, and creates debugging nightmares when a faulty extension crashes the host.
- **WebAssembly guest runtime**: rejected for v1. Adds a runtime dependency (e.g., Wasmtime) and significant build/debug complexity for extension authors. WASM is a plausible v2 path once the extension ecosystem exists.
- **Fixed language choice (Rust-only or TypeScript-only extensions)**: rejected. Cuts the extension author population in half and forces non-Rust authors to learn a second language to contribute.
- **`$PATH` discovery with `qualitool-ext-*` naming**: rejected as the primary discovery path. Encourages global install pollution and makes it hard to ship per-project extensions with a client repo. A future ADR may add `$PATH` as a fallback if demand exists.
