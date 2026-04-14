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
| ACOUSTIC-007 | Extend rewrite registry for Node.js/pnpm stack | `src/discover/rules.rs` | — |
| ACOUSTIC-008 | Add yarn, NestJS, Gradle wrappers, extend Maven | `src/discover/rules.rs` | — |
| ACOUSTIC-010 | Fix `rtk read` default filter: `none` → `minimal` | `src/main.rs` | — |
| ACOUSTIC-011 | Route test commands to generic test filter | `src/discover/rules.rs` | — |
| ACOUSTIC-012 | Capture jest failure details in test filter | `src/cmds/rust/runner.rs` | — |
| ACOUSTIC-013 | Bypass test filter for diagnostic flags | `src/cmds/rust/runner.rs` | — |
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

### ACOUSTIC-007: Node.js/pnpm Stack Coverage

The upstream rewrite registry lacked coverage for common Node.js workflows used at Acoustic. Jest commands, `pnpm run` scripts, and `pnpm run lint` were not intercepted by the hook, meaning Claude Code's heaviest output (test runs, builds, lint reports) bypassed RTK entirely.

**What we changed** in `src/discover/rules.rs`:
- Vitest rule: added `npx jest`, `pnpm jest`, `pnpm test` to rewrite prefixes (routes through `rtk vitest` filter — failures-only output)
- npm rule: extended pattern from `^npm\s+(run|exec)` to `^(pnpm|npm)\s+(run|exec)`, added `pnpm` to prefixes
- Lint rule: added `pnpm run lint` to rewrite prefixes

**Impact**: `npx jest --no-cache` (2658 tests, ~25K tokens raw) now compresses to failures-only output (~500 tokens). `pnpm run build` and `pnpm run lint` output is compressed through existing npm/lint filters.

### ACOUSTIC-008: Yarn, NestJS, Gradle/Maven Wrappers

Acoustic projects span multiple ecosystems: React (yarn), NestJS (nest CLI), Java/Flink (Gradle/Maven wrappers). These commands were not intercepted by RTK.

**What we changed** in `src/discover/rules.rs`:
- npm rule: merged yarn and nest into the existing rule (shared `rtk_cmd: "rtk npm"` requires single-rule approach to avoid lookup collision). Pattern now covers `yarn run/exec/test/build/lint/install/add/remove` and `nest build/start/test` with npx/pnpm prefixes.
- mvn rule: extended pattern to include `test`, `verify`, `generate-sources`, `versions:*` subcommands. Added `./mvnw` and `mvnw` wrapper prefixes.
- gradle rule (new): added `./gradlew`, `gradlew`, `gradle` with subcommands `build/test/clean/check/assemble/spotlessCheck/spotlessApply/jacocoTestReport`.

**Note**: `rtk gradle` and `rtk mvn` are transparent proxies (no dedicated filter module). Commands execute correctly and token usage is tracked, but output is not yet compressed. Future filter modules can add compression without changing rules.

### ACOUSTIC-010: Fix `rtk read` Default Filter Level

Upstream RTK v0.35.0 ships with the `--level` Clap argument defaulting to `"none"` instead of `"minimal"` as documented. This means every `rtk read` invocation — including all hook-rewritten `cat` calls — applies zero filtering, producing byte-identical output to raw `cat`.

**What we changed**: In `src/main.rs`, the `Commands::Read` Clap definition's `default_value` for the `--level` arg changed from `"none"` to `"minimal"`.

**Impact measured on Acoustic codebase**:
- 181KB legacy Java file (TLEventProcessor.java): 0% → **22% reduction** (181K → 141K bytes)
- Clean TypeScript files (valkey-consumer.service.ts): 0% → **5.2% reduction** (comments/blanks stripped)
- Session-level `rtk gain`: **5.5% → 41.8% efficiency** — `rtk read` became the #1 token saver (161K tokens across 27 reads)

**What minimal filtering strips**: single-line comments (`//`, `#`), block comments (`/* */`), and blank lines. All code logic, imports, type signatures, and string literals are preserved. Claude retains full ability to read and edit files.

**Known upstream issues** (not fixed here, noted for reference):
- `minimal` does not strip Javadoc (`/** */`) blocks — explains why Java savings are 22% not 40-60%
- `aggressive` filter produces empty output on small files, triggering fallback to raw passthrough

**Verification**: `rtk read <file> -v` now shows `(filter: minimal)` instead of `(filter: none)`.

### ACOUSTIC-011: Route Test Commands to Generic Test Filter

The general npm/pnpm/yarn rule (`rtk npm`) catches `pnpm test` and `npm test` as a passthrough with ~0% compression. The generic test filter (`rtk test`) shows failures only with 90-97% compression. Team member benchmarking showed 0.3% savings on `rtk npm test` vs 97.1% on `rtk test`.

**What we changed** in `src/discover/rules.rs`:
- Added five rules AFTER the general npm and npx rules (RTK's RegexSet uses last-match-wins — specific overrides must have higher array index than the catch-all)
- `pnpm test` → `rtk test pnpm test` (failures-only filter)
- `npm test` → `rtk test npm test` (failures-only filter)
- `yarn test` → `rtk test yarn test` (failures-only filter)
- `npx jest` → `rtk test npx jest` (failures-only filter)
- `pnpm jest` → `rtk test pnpm jest` (failures-only filter)
- All other npm/pnpm commands (`pnpm run build`, `npm install`, etc.) still route to `rtk npm` unchanged

**Key discoveries during implementation**:
1. RegexSet `matches().last()` means highest-index rule wins — the opposite of typical route matching conventions
2. `rtk test` is a wrapper expecting the full command as arguments — `rtk_cmd` must be `"rtk test pnpm"` (not `"rtk test"`), with `rewrite_prefixes: &["pnpm"]`, so `pnpm test` strips `pnpm` and produces `rtk test pnpm test`
3. Each package manager needs its own rule because `rtk_cmd` must embed the runner name

### ACOUSTIC-012: Capture Jest Failure Details in Test Filter

The `extract_test_summary` function in `runner.rs` only captured `FAIL` file headers and summary lines for jest output. Assertion diffs, test names, code context, and stack traces were stripped. When a test failed, Claude received only `FAIL src/test/file.test.ts` with no diagnostic information, forcing 6+ extra tool calls (re-read test file, re-read source, re-run with verbose, etc.) to diagnose the failure.

**What we changed** in `src/cmds/rust/runner.rs`:
- Added `pnpm test` to the jest command detection (was missing, only matched `jest`, `npm test`, `yarn test`)
- Jest failure blocks starting with `●` (test name) are now captured along with their indented body (assertion diff, source lines, stack trace)
- Failure detail lines capped at 50 to prevent output explosion when many tests fail
- Passing test output is unchanged — still summary-only

**Before (old filter output on failure):**

### ACOUSTIC-013: Bypass Test Filter for Diagnostic Flags

ACOUSTIC-011 routes `npx jest` and `pnpm jest` to the test filter. But diagnostic invocations like `npx jest --listTests` or `pnpm jest --showConfig` are not test executions — their output is structured data (file lists, configuration JSON) that the test filter corrupts. In a real session, Claude received only 6 of 27 test files from `--listTests` because the filter stripped most of the output, causing several wasted tool calls to debug a Jest cache issue.

**What we changed** in `src/cmds/rust/runner.rs`:
- Added `DIAGNOSTIC_FLAGS` constant: `--listTests`, `--showConfig`, `--help`, `--version`, `-h`, `-V`
- At the top of `run_test()`, if any arg matches a diagnostic flag, the command runs via `run_passthrough()` — full unfiltered output, still tracked in `rtk gain`
- Normal test runs are unaffected

**Impact**: `rtk test npx jest --listTests` now returns all 27 test files instead of 6. Zero extra tool calls needed to resolve test discovery issues.

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
| `src/discover/rules.rs` | LOW | Our added prefixes removed? New rules conflict with our additions? |
| `src/main.rs` | MEDIUM | `Commands::Read` level default changed back to `"none"`? |
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
| Registry extensions | Prefix additions in rules.rs | No new patterns — just added missing prefixes for commands RTK already has filters for (jest→vitest, pnpm→npm) |
| Yarn/Nest merge | Single npm rule with combined pattern | Multiple rules sharing same `rtk_cmd` causes lookup collision — must be one rule |
| Gradle/Maven wrappers | Prefix-only (no filter module) | Transparent proxy with tracking is still valuable; filter can be added later |
| Read filter default | `minimal` (was `none`) | Upstream bug: docs say minimal, binary ships none. 41.8% efficiency gain with zero functional impact on Claude's ability to read/edit code |
| Test command routing | Separate rules per runner, rtk_cmd embeds runner name (last-match-wins) | RegexSet picks highest index; `rtk test` wrapper needs full command as args, so rtk_cmd must be `"rtk test pnpm"` not `"rtk test"` |
| Jest failure detail | Capture ● blocks with 50-line cap | 97% compression on failures is counterproductive — Claude spends more tokens re-reading files than it saves. 90% with diagnostic info is net cheaper. |
| Diagnostic flag bypass | Passthrough in runner.rs, not rules.rs | Rules only match command prefixes — can't inspect deep flags. The filter function is the right place to decide whether to compress. |