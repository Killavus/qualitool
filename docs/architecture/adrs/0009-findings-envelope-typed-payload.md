# ADR-0009: Findings use a canonical envelope with typed per-Check payload

**Status:** Accepted
**Date:** 2026-04-14

## Context

Multiple frontends — CLI text output, JSON export, a future Web UI, a future SARIF export for IDE integration — need to display, sort, filter, group, and export Findings across every Check in a uniform way. At the same time, each Check produces domain-specific data:

- A churn-hotspot Check wants to show a histogram of edits per file over time.
- A dependency-smell Check wants to show a graph fragment highlighting a problematic edge.
- An ownership Check wants to show a table of files and their dominant authors.

A flat "one canonical struct for everything" loses per-Check richness. An opaque "every Finding is a blob of JSON" loses the ability for frontends to do anything generic.

## Decision

Every Finding has a **canonical envelope** plus a **check-specific typed payload**:

```rust
struct Finding {
    id: FindingId,              // stable across runs given identical inputs
    check_id: CheckId,          // which Check produced this Finding
    severity: Severity,         // Info | Low | Medium | High | Critical
    title: String,              // short human-readable title
    summary: String,            // one-line human description
    location: Option<CodeLocation>,  // file:line:col if applicable
    tags: Vec<String>,          // e.g. ["performance", "security", "hotspot"]
    payload: serde_json::Value, // check-specific typed data
}

struct CodeLocation {
    file: PathBuf,              // relative to project root
    line_start: u32,
    line_end: Option<u32>,
    col_start: Option<u32>,
    col_end: Option<u32>,
}

enum Severity { Info, Low, Medium, High, Critical }
```

### Payload schema

Each Check declares the JSON Schema of its `payload` field as part of its `extension.describe` response (for third-party Checks) or its static manifest (for built-ins). The core does not enforce the schema at runtime — Checks are trusted to produce payloads matching their declared schema — but frontends can use it to generate typed renderers.

### FindingId stability

The `FindingId` is a deterministic hash of `(check_id, location, payload-stable-fields)` where `payload-stable-fields` are payload fields the Check declares as "identity-contributing." This makes Findings comparable across runs — useful for diffing Findings between two runs to see what changed.

### Frontend behavior

- **CLI text renderer**: renders the envelope uniformly (severity-colored bullet list with title, location, summary). The payload is rendered only when `--detail` is passed or when the Check provides a specific text renderer.
- **CLI JSON exporter**: emits the full Finding (envelope + payload) as-is.
- **Web UI**: list view uses the envelope; detail view dispatches to a per-Check renderer for the payload (with a generic JSON-tree fallback for Checks that have no custom renderer).
- **SARIF exporter**: envelope fields map directly to SARIF result fields; payload goes into SARIF `properties`.

## Consequences

**Positive:**
- CLI, JSON, and SARIF exporters work for every Check without per-Check code.
- The Web UI can progressively enhance — list view works instantly on day one, detail view can grow custom renderers for high-value Checks over time.
- FindingId stability enables cross-run diffing: "what Findings appeared since yesterday's run?"
- Checks retain full freedom to produce whatever structured data makes sense for their analysis.

**Negative:**
- Check authors must design both an envelope summary (short and display-oriented) and a payload structure (rich but well-typed). This is two artifacts where a simpler model would have one.
- The payload is an unstructured `serde_json::Value` at the scheduler level; type safety exists only at the Check boundary (where the Check serializes its typed payload) and at the renderer boundary (where a per-Check renderer deserializes it). Middle layers see opaque JSON.
- Frontends that want to render payloads richly must implement per-Check renderers or fall back to a generic JSON tree view.

## Alternatives Considered

- **Single canonical struct with no payload**: rejected. Loses per-Check richness and forces all Findings into the lowest common denominator. A histogram Check can't meaningfully use a flat struct.
- **Opaque JSON blobs with no envelope**: rejected. Frontends cannot sort by severity, filter by tag, jump to location, or group by Check without parsing the blob — which defeats the point. Every frontend would need Check-specific code.
- **Protobuf-style fixed schemas per Check registered in a central registry**: rejected as over-engineered. The JSON Schema approach gets 90% of the benefit with none of the registry infrastructure.
