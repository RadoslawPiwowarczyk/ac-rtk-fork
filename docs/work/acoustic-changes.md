# Acoustic RTK Patches

Based on upstream [rtk-ai/rtk](https://github.com/rtk-ai/rtk) v0.35.0 (commit `8a7106c`).

## Security Patches Applied

| ID | Patch | Files Modified | Files Created |
|---|---|---|---|
| ACOUSTIC-001 | Strip telemetry at compile time | `src/core/telemetry.rs`, `Cargo.toml` | — |
| ACOUSTIC-002 | Replace `sh -c` with direct execution + metacharacter guard | `src/cmds/rust/runner.rs`, `src/cmds/system/summary.rs`, `src/main.rs` | `src/core/sanitize.rs` |
| ACOUSTIC-003 | Selective hook permissions — read-only auto-allow | `hooks/claude/rtk-rewrite.sh` | — |
| ACOUSTIC-004 | Argument scrubbing in tracking DB, retention reduced to 7d | `src/core/tracking.rs`, `src/core/constants.rs` | `src/core/scrub.rs` |
| ACOUSTIC-005 | Disable tee by default, validate RTK_TEE_DIR path | `src/core/tee.rs` | — |
| ACOUSTIC-006 | Exclude sensitive commands from hook rewriting | `src/discover/registry.rs` | — |

## Patch Details

### ACOUSTIC-001: Telemetry Stripped

Upstream RTK sends daily anonymous usage metrics (device hash, top command names, token savings, OS/arch) to a compiled-in endpoint via the `ureq` HTTP client. The endpoint URL is injected at build time from GitHub Actions secrets and cannot be inspected without disassembly.

**What we changed**: Replaced `src/core/telemetry.rs` with a no-op function. Removed `ureq`, `hostname`, and `getrandom` from `Cargo.toml`. This also eliminated RUSTSEC-2026-0049 (rustls-webpki vulnerability) and reduced dependencies from 204 to 164.

**Verification**: `strace -e trace=network` confirms zero outbound connections.

### ACOUSTIC-002: Shell Injection Fixed

`rtk err`, `rtk test`, and `rtk summary` accepted arbitrary command strings and passed them to `sh -c` / `cmd /C`. The hook's `permissionDecision: "allow"` bypassed Claude Code's permission prompt, creating an LLM prompt-injection → shell-injection chain.

**What we changed**: Functions now accept `&[String]` (Clap's parsed args) instead of `&str`. Execution uses `Command::new(bin).args(rest)` — no shell involved. A metacharacter guard (`src/core/sanitize.rs`) rejects arguments containing `;`, `|`, `&`, `$`, backticks, parentheses, braces, redirects, or newlines.

**Trade-off**: Compound commands like `rtk test "cargo build && cargo test"` no longer work. Users must split them into separate calls. This is intentional — the security benefit outweighs the convenience loss.

### ACOUSTIC-003: Hook Permission Model

The upstream hook emits `permissionDecision: "allow"` for every rewritten command. Our patch introduces a read-only allowlist: only safe read-only operations auto-allow, everything else defers to Claude Code's native permission prompt.

**Auto-allowed (read-only)**: git status/log/diff/branch/show/remote/tag/stash, gh pr/issue/run, ls, read, grep, find, deps, gain, discover, json, wc, smart, diff, session.

**User prompted (write/execute)**: git push/commit/add/merge/rebase, cargo test/build/clippy, test, err, summary, proxy, env, curl, docker, kubectl, all others.

### ACOUSTIC-004: Database Scrubbing

The tracking database stored full command strings including arguments for 90 days. Commands with bearer tokens, connection strings, or AWS keys had secrets persisted in plaintext.

**What we changed**: Both INSERT paths (commands table + parse_failures table) are wrapped with `scrub::scrub_command()` which redacts known secret patterns via 9 regex rules. Retention reduced from 90 to 7 days.

### ACOUSTIC-005: Tee Disabled

RTK's tee feature saves full unfiltered output on failure. Disabled by default (was enabled). Added path validation for `RTK_TEE_DIR` to prevent path traversal. Can be re-enabled via config if needed.

### ACOUSTIC-006: Command Exclusions

Commands that inherently handle sensitive data are excluded from rewriting: `env`, `curl`, `wget`, `ssh`, and `kubectl exec`. These pass through to Claude Code's native handling (uncompressed, with normal permission prompts).

## Updating from Upstream

```bash
git fetch upstream
git log upstream/master --oneline -20  # Review what changed
```

**Before merging any upstream changes, audit these files for regressions:**

| File | Risk | What to check |
|---|---|---|
| `src/cmds/rust/runner.rs` | HIGH | New `sh -c` or `Command::new("sh")` patterns reintroduced? |
| `src/cmds/system/summary.rs` | HIGH | Same check as runner.rs |
| `hooks/claude/rtk-rewrite.sh` | HIGH | `permissionDecision: "allow"` without allowlist check? |
| `src/hooks/init.rs` | HIGH | Hook template changed? (regenerates from include_str) |
| `src/core/tracking.rs` | MEDIUM | New INSERT without scrub wrapper? |
| `src/core/telemetry.rs` | SKIP | We replaced the entire file — skip upstream changes |
| `src/core/tee.rs` | LOW | Default changed back to enabled? |
| `src/discover/registry.rs` | LOW | Our ACOUSTIC_EXCLUDE block removed? |
| `Cargo.toml` | MEDIUM | `ureq` or new HTTP client added? New suspicious deps? |

**Merge strategy**: Cherry-pick specific commits rather than merging master. Review each commit's diff against the files listed above.

## Decision Log

| Decision | Choice | Rationale |
|---|---|---|
| Telemetry removal | Compile-time strip (not runtime config) | Runtime config can be re-enabled; compile-time removal eliminates the HTTP client entirely |
| Shell injection fix | Pass `&[String]` directly, no shell | Using Clap's native parsed args avoids the join → split round-trip and all its edge cases |
| Hook permissions | Allowlist in bash script (Option B) | Self-contained, no Claude Code config changes needed, testable via JSON pipe |
| DB scrubbing | Regex-based, 9 patterns | Covers the most common secret formats; false-positive tested against normal commands |
| Retention | 7 days (was 90) | Enough for `rtk gain` analytics while minimizing exposure window |
| Tee default | Disabled (was enabled) | Raw output may contain secrets; can be re-enabled per-project if needed |
| Command exclusions | Hardcoded in registry.rs | Simple, no config to misconfigure; `env`/`curl`/`wget`/`ssh` have no meaningful RTK compression benefit |
