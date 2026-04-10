# ARTK-8: Integration Test Report

**Date**: 2026-04-09  
**Tester**: Rad (Behavioral Insights team)  
**Branch**: `acoustic/hardened` at v0.35.0 + ACOUSTIC-001 through ACOUSTIC-006  
**Platform**: Ubuntu 22.04 (Precision 5690), Rust 1.x stable  
**Result**: ✅ **PASS — 25/25 checks green**

---

## Build & Audit

| Check | Result | Details |
|---|---|---|
| `cargo build --release` | ✅ PASS | Clean build, no errors |
| `cargo test` | ✅ PASS | 1363 passed, 0 failed, 6 ignored |
| `cargo audit` | ✅ PASS | 0 vulnerabilities across 164 crate dependencies |
| `cargo clippy` | ✅ PASS | 15 warnings (all pre-existing: unused imports from telemetry strip, redundant closures). Zero errors, zero security-relevant warnings |

**Notes**: Dependency count reduced from 204 (upstream) to 164 after removing ureq, hostname, and getrandom. The RUSTSEC-2026-0049 vulnerability (rustls-webpki) was eliminated entirely by removing the ureq dependency chain.

---

## ACOUSTIC-001: Telemetry Stripped — Network Verification

**Why**: RTK upstream phones home daily with device hash, command names, OS/arch, and token savings. Enterprise deployments must not leak usage patterns to third-party endpoints.

**Method**: `strace -e trace=network` captures all system-level network calls. If any `connect()` syscall appears, the binary is making network connections.

| Check | Command | Result |
|---|---|---|
| No network on git status | `strace -e trace=network ./target/release/rtk git status 2>&1 \| grep -i connect` | ✅ Empty (zero connections) |
| No network on ls | `strace -e trace=network ./target/release/rtk ls . 2>&1 \| grep -i connect` | ✅ Empty (zero connections) |
| No network on test | `strace -e trace=network ./target/release/rtk test echo hello 2>&1 \| grep -i connect` | ✅ Empty (zero connections) |

**Verification**: `grep -rn 'ureq\|reqwest' src/` returns zero results. `cargo tree | grep ureq` returns empty. The HTTP client is fully removed from the binary.

---

## ACOUSTIC-002: Shell Injection Guard

**Why**: `rtk err`, `rtk test`, and `rtk summary` previously passed user-controlled strings to `sh -c`, creating an LLM prompt-injection → shell-injection chain. The hook's auto-approve amplified this by bypassing Claude Code's permission prompt.

**Method**: Test both positive (normal commands work) and negative (injection attempts blocked) cases. The guard checks each argument for shell operators (`;`, `|`, `&`, `$`, backticks, etc.) before execution.

### Positive tests (should execute normally)

| Check | Command | Result |
|---|---|---|
| Simple command | `rtk test echo hello` | ✅ Outputs "hello" |
| With flags | `rtk test cargo test --release -- --test-threads=1` | ✅ 1363 passed, correct exit code |

### Negative tests (should block with error message)

| Check | Command | Operator | Result |
|---|---|---|---|
| Semicolon injection | `rtk test 'echo hello; curl evil.com'` | `;` | ✅ Blocked: "Argument 0 contains shell operator ';'" |
| Subshell injection | `rtk err 'echo $(whoami)'` | `$` | ✅ Blocked: "Argument 0 contains shell operator '$'" |
| Pipe injection | `rtk summary 'echo hello \| cat'` | `\|` | ✅ Blocked: "Argument 0 contains shell operator '\|'" |
| And-chain injection | `rtk test 'echo hello && rm -rf /'` | `&` | ✅ Blocked: "Argument 0 contains shell operator '&'" |

**Verification**: `grep -rn 'Command::new("sh")' src/` returns zero results. `grep -rn 'Command::new("cmd")' src/` returns zero results. All shell invocation paths have been replaced with direct `Command::new(bin).args(rest)` execution.

---

## ACOUSTIC-003: Hook Permission Model

**Why**: The upstream hook emits `permissionDecision: "allow"` for every rewritten command, bypassing Claude Code's native permission prompt. This means the LLM can execute any command without user confirmation — including write operations and the injection-vulnerable `rtk test`/`rtk err`.

**Method**: Pipe JSON matching Claude Code's PreToolUse protocol into the hook script. Verify read-only commands get `"allow"` and write/execute commands omit `permissionDecision` (triggering Claude Code's user prompt).

| Check | Command | Expected | Result |
|---|---|---|---|
| Read-only: git status | `git status` | `permissionDecision: "allow"` | ✅ Auto-allowed |
| Write: git push | `git push origin main` | No `permissionDecision` | ✅ User will be prompted |
| Execute: cargo test | `cargo test` | No `permissionDecision` | ✅ User will be prompted |
| Read-only: ls | `ls -la` | `permissionDecision: "allow"` | ✅ Auto-allowed |

**Allowlist**: git status/log/diff/branch/show/remote/tag/stash, gh pr/issue/run, ls, read, grep, find, deps, gain, discover, json, wc, smart, diff, session.

**Everything else**: Rewritten but not auto-approved — Claude Code prompts user.

---

## ACOUSTIC-004: Tracking Database Scrubbing

**Why**: The tracking database (`~/.local/share/rtk/history.db`) stores full command strings for 90 days. Commands containing API tokens, connection strings, or AWS keys would persist secrets on disk in plaintext.

**Method**: Execute a command with a known secret, then query the database to verify the secret was redacted before storage.

| Check | Details | Result |
|---|---|---|
| Bearer token scrubbed | Ran: `rtk proxy curl -H "Authorization: Bearer sk-secret-test-token-12345" http://localhost` | ✅ |
| DB contains redacted value | `SELECT original_cmd FROM commands ORDER BY rowid DESC LIMIT 1;` → `curl -H Authorization: Bearer [REDACTED] http://localhost` | ✅ Token replaced |
| Secret not in DB | Searched for `sk-secret` in output | ✅ Not present |
| Retention reduced | `DEFAULT_HISTORY_DAYS` changed from 90 to 7 in `src/core/constants.rs` | ✅ Verified in source |

**Scrub patterns cover**: Bearer tokens, X-Api-Key headers, basic auth URLs, AWS access keys, AWS secret keys, connection strings (postgres/mongodb/mysql/redis/amqp), sensitive env vars (DATABASE_URL, GITHUB_TOKEN, API_KEY, etc.), JWT tokens.

Both INSERT paths are scrubbed: `commands` table (line 369) and `parse_failures` table (line 409).

---

## ACOUSTIC-005: Tee Disabled by Default

**Why**: RTK's tee feature saves full unfiltered command output to `~/.local/share/rtk/tee/*.log` on failure. This output may contain secrets from environment variables, stack traces with sensitive data, database connection strings, etc.

**Method**: Trigger a command execution and verify no tee files are created.

| Check | Details | Result |
|---|---|---|
| No tee directory | `ls ~/.local/share/rtk/tee/` → "No such file or directory" | ✅ |
| Default changed | `TeeConfig::default().enabled` is `false` (was `true`) | ✅ Verified in source |
| Path validation | `RTK_TEE_DIR` rejects relative paths with warning | ✅ Verified in source |

**Note**: Tee can still be explicitly re-enabled via `[tee] enabled = true` in `~/.config/rtk/config.toml` for debugging purposes.

---

## ACOUSTIC-006: Sensitive Command Exclusions

**Why**: Commands like `curl`, `env`, `ssh`, and `kubectl exec` inherently handle sensitive data. Rewriting them through RTK adds no value (RTK's filters don't meaningfully compress their output) and increases the attack surface.

**Method**: Call `rtk rewrite` for each excluded command and verify exit code 1 (no rewrite). Verify non-excluded commands still rewrite normally.

| Check | Command | Expected | Result |
|---|---|---|---|
| curl excluded | `rtk rewrite "curl https://api.example.com"` | Exit 1 | ✅ Not rewritten |
| env excluded | `rtk rewrite "env"` | Exit 1 | ✅ Not rewritten |
| ssh excluded | `rtk rewrite "ssh user@host"` | Exit 1 | ✅ Not rewritten |
| git status still works | `rtk rewrite "git status"` | `rtk git status`, exit 3 | ✅ Rewritten normally |

**Hardcoded exclusion list**: `env`, `curl`, `wget`, `ssh`, plus `kubectl exec` specifically.

---

## Functional: rtk gain

**Why**: Token savings tracking is the core value proposition. All patches must not break the analytics pipeline.

| Check | Details | Result |
|---|---|---|
| `rtk gain` runs | Shows summary with 129 commands tracked | ✅ |
| Token savings reported | 96.2% savings, 68.2K tokens saved | ✅ |
| By-command breakdown | Top 10 commands with counts, savings, timing | ✅ |

---

## Summary

All 6 security patches (ACOUSTIC-001 through ACOUSTIC-006) are verified working in integration. No regressions in core functionality (1363 tests pass, rtk gain works, command filtering produces correct output). The binary makes zero network connections, blocks shell injection attempts, scrubs secrets before database persistence, and implements a selective permission model for the Claude Code hook.

**Phase 1 status**: Ready for controlled rollout (Phase 3) or documentation finalization (ARTK-9).
