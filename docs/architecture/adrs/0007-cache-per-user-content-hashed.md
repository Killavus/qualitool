# ADR-0007: Per-user content-hashed cache location

**Status:** Accepted
**Date:** 2026-04-14

## Context

Probe outputs are expensive — parsing a 500 kLOC repo's AST, walking full git history, building a dependency graph — but they are **deterministic** over their inputs. The cache is what makes iterative Check development and repeat runs against the same repo fast. Three requirements constrain the cache location:

1. **Client repo must stay pristine.** Consultants cannot leave `.qualitool/cache/` directories in client repos; it pollutes `.gitignore` files, risks accidental commits, and makes fresh clones re-compute everything.
2. **Cache must survive fresh clones.** When a consultant re-clones the client repo (branch switch, fresh worktree, different machine with restored state), previously computed Probe outputs should still be valid.
3. **Isolation between engagements.** State from client A must never apply to client B, even if the consultant is running qualitool against both in the same session.

## Decision

### Location

Cache lives at `$XDG_CACHE_HOME/qualitool/<repo-hash>/` on Unix (`~/.cache/qualitool/<repo-hash>/` if `XDG_CACHE_HOME` is unset). On macOS, the same `$XDG_CACHE_HOME` convention is followed; qualitool does **not** use `~/Library/Caches/` to keep the behavior uniform with Linux and to simplify CI paths.

### Repo hash

The `<repo-hash>` is a short (first 16 hex chars of SHA-256) hash derived from:

- Absolute project root path (disambiguates multiple checkouts of the same repo).
- Git HEAD commit hash, or for non-Git projects, a stable hash of the file tree.
- Qualitool schema version (invalidates on breaking changes).

This is the same hash used for the user overlay scoping in ADR-0006.

### Directory layout

```
$XDG_CACHE_HOME/qualitool/
└── <repo-hash>/
    ├── manifest.json                 # cache metadata: created, last-accessed, qualitool version
    └── probes/
        └── <probe-name>/
            └── <probe-input-hash>.json
```

Each Probe declares its input hash over (probe version, probe config, relevant file set). A mismatch in any input invalidates the entry. The per-probe directory layout makes `qualitool cache clean --probe <name>` trivial.

### CI integration

On CI, consultants point `XDG_CACHE_HOME` at a CI-cached directory (`actions/cache` on GitHub Actions, `cache:` on GitLab, etc.) and key the cache on the repo hash computed by `qualitool cache key`. A one-line snippet in the docs shows how.

### Eviction

No automatic eviction in v1. `qualitool cache clean` is a manual command, with flags:
- `--all` — wipe the entire qualitool cache directory
- `--repo <hash>` — wipe cache for one repo
- `--older-than <duration>` — wipe entries last accessed before a cutoff
- `--probe <name>` — wipe entries for a specific probe across all repos

Automatic LRU eviction is a plausible v2 addition if disk usage becomes a real complaint.

## Consequences

**Positive:**
- Client repos stay pristine. No `.qualitool` artifacts appear in the consultant's working tree.
- Cache survives clones, branch switches, and fresh worktrees, because it is keyed on repo content, not filesystem location alone.
- Engagement isolation is automatic — different repos, different hashes, different directories.
- CI caching works with a single well-known key (`qualitool cache key` → the repo hash).

**Negative:**
- Disk usage grows unbounded without manual cleanup. A consultant running against many large repos will eventually need to run `qualitool cache clean`. Mitigated by shipping `qualitool cache clean --older-than 30d` as a suggested cron target in the docs.
- CI caching requires consultants to understand the hash convention. Mitigated by the `qualitool cache key` helper command.
- The cache directory is separate from the project, so `rm -rf` of the project does not clean its cache entries. This is a deliberate trade for the benefits listed above.

## Alternatives Considered

- **In-repo `.qualitool/cache/`**: rejected. Pollutes client repos, creates gitignore hygiene burden, risks accidental commits of cache data, lost on every fresh clone.
- **Single flat per-user cache keyed by (repo, probe, file)**: rejected. Harder to clear per-engagement state; `qualitool cache clean --repo X` becomes a global scan rather than a directory delete.
- **System temp directory (`/tmp/qualitool-<hash>`)**: rejected. Non-persistent across reboots on many systems; consultants lose work between sessions.
- **Platform-specific locations (`~/Library/Caches/` on macOS, `%LOCALAPPDATA%` on Windows)**: rejected for uniformity. Same `XDG_CACHE_HOME` convention everywhere makes docs and CI scripts simpler.
