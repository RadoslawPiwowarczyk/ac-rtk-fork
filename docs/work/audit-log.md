# AUDIT-LOG.md — Security Surface Audit

**Date**: 2026-04-09  
**Commit**: `8a7106c` (v0.35.0)  
**Auditor**: Rad (Behavioral Insights team)  
**Build status**: ✅ 1350 passed, 0 failed, 6 ignored  
**Cargo audit**: ⚠️ 1 vulnerability (RUSTSEC-2026-0049 in rustls-webpki → rustls → ureq → rtk) — resolved by ARTK-2 (removing ureq)

---

## 1. Shell Injection Surface (`sh -c` / `cmd /C`)

**Finding**: Exactly 3 files, 6 call sites. All user-controlled. All require patching.

| File | Line | Pattern | Context | Task |
|---|---|---|---|---|
| `src/cmds/rust/runner.rs` | 17 | `Command::new("cmd")` | Windows path for `rtk err` | ARTK-3 |
| `src/cmds/rust/runner.rs` | 23 | `Command::new("sh")` | Unix path for `rtk err` | ARTK-3 |
| `src/cmds/rust/runner.rs` | 73 | `Command::new("cmd")` | Windows path for `rtk test` | ARTK-3 |
| `src/cmds/rust/runner.rs` | 79 | `Command::new("sh")` | Unix path for `rtk test` | ARTK-3 |
| `src/cmds/system/summary.rs` | 18 | `Command::new("cmd")` | Windows path for `rtk summary` | ARTK-3 |
| `src/cmds/system/summary.rs` | 24 | `Command::new("sh")` | Unix path for `rtk summary` | ARTK-3 |

**Note**: `grep -rn '\.arg("-c")' src/` returned zero results. The `-c` arg is passed via `.args(["-c", command])` (plural), so grep for the string literal alone misses it. The `Command::new("sh")` grep is sufficient to locate all instances.

**Confirmed**: No other files use `sh -c`. All other command modules (git, cargo, ls, grep, find, docker, kubectl, etc.) use safe `resolved_command("binary").arg()` pattern via `run_filtered()`.

---

## 2. Network Calls

**Finding**: Single network call site. Telemetry only. One dependency (`ureq`).

| File | Line | Code | Purpose | Task |
|---|---|---|---|---|
| `src/core/telemetry.rs` | 81 | `ureq::post(url).set("Content-Type", "application/json")` | Daily telemetry phone-home | ARTK-2 |

**Dependency chain**:
```
ureq v2.12.1
└── rtk v0.35.0
```

`ureq` is used ONLY by telemetry. Removing it eliminates:
- The single network call
- The `ureq` dependency and its entire transitive tree (rustls, rustls-webpki, etc.)
- The RUSTSEC-2026-0049 vulnerability

**Additional telemetry file-write locations** (salt file, marker file):
| File | Line | Code | Purpose |
|---|---|---|---|
| `src/core/telemetry.rs` | 128 | `File::create(&salt_path)` | Creates local salt file for device hash |
| `src/core/telemetry.rs` | 225 | `fs::write(path, b"")` | Writes daily telemetry marker (prevents re-send) |

---

## 3. Hook Permission Decisions

**Finding**: Auto-allow lives in 2 locations — the bash hook script and the Rust hook command.

### Claude Code hook (bash script — primary target for ARTK-4)

| File | Line | Code | Notes |
|---|---|---|---|
| `hooks/claude/rtk-rewrite.sh` | 77 | Comment: "Ask: rewrite command, omit permissionDecision" | Existing ask path (for unmatched commands) |
| `hooks/claude/rtk-rewrite.sh` | 93 | `"permissionDecision": "allow"` | **THE TARGET** — auto-allows all rewritten commands |
| `hooks/claude/rtk-rewrite.sh` | 94 | `"permissionDecisionReason": "RTK auto-rewrite"` | Reason string |

### Rust hook command (for Copilot/Gemini — secondary target)

| File | Line | Code | Notes |
|---|---|---|---|
| `src/hooks/hook_cmd.rs` | 132 | `"permissionDecision": decision` | Variable — need to trace what `decision` is set to |
| `src/hooks/hook_cmd.rs` | 133 | `"permissionDecisionReason": "RTK auto-rewrite"` | Same pattern as bash hook |
| `src/hooks/hook_cmd.rs` | 148 | `"permissionDecision": "deny"` | Copilot CLI deny-with-suggestion path (safe) |
| `src/hooks/hook_cmd.rs` | 149 | `"permissionDecisionReason": format!(...)` | Deny reason (safe) |

### Hook template in init.rs

The bash hook script is generated from a template in `src/hooks/init.rs`. The exact template location needs to be identified:

```bash
grep -n 'REWRITE_HOOK\|rtk-rewrite' src/hooks/init.rs | head -20
```

Both the installed `hooks/claude/rtk-rewrite.sh` AND the template in `init.rs` must be patched together.

---

## 4. Tracking Database (Secrets Persistence)

**Finding**: Two INSERT statements, both in `src/core/tracking.rs`.

| File | Line | Code | What gets stored | Task |
|---|---|---|---|---|
| `src/core/tracking.rs` | 369 | `INSERT INTO commands (timestamp, original_cmd, rtk_cmd, project_path, input_tokens, output_tokens, saved_tokens, savings_pct, exec_time_ms)` | Full command strings — potential secrets in `original_cmd` and `rtk_cmd` | ARTK-5 |
| `src/core/tracking.rs` | 409 | `INSERT INTO parse_failures (timestamp, raw_command, error_message, fallback_succeeded)` | Raw commands on parse failure — potential secrets in `raw_command` | ARTK-5 |

**Note**: Both `original_cmd` and `raw_command` fields need scrubbing. The `parse_failures` table was not mentioned in Issue #640 — this is a new finding specific to our audit.

---

## 5. Tee File Output

**Finding**: Tee system contained in single file `src/core/tee.rs`.

| File | Line | Code | Risk | Task |
|---|---|---|---|---|
| `src/core/tee.rs` | 40 | `std::env::var("RTK_TEE_DIR")` | Path traversal — no validation on env var | ARTK-6 |
| `src/core/tee.rs` | 135 | `std::fs::write(&filepath, content)` | Writes raw unfiltered output to disk | ARTK-6 |

`get_tee_dir()` at line 38 accepts `RTK_TEE_DIR` without canonicalization or absolute-path check.

---

## 6. Other File Writes (Informational — No Patching Required)

Most `fs::write` / `File::create` results are in:
- **Test code** (lines in `#[cfg(test)]` modules) — not production risk
- **Hook installation** (`src/hooks/init.rs`) — writes hook scripts, CLAUDE.md, settings.json during `rtk init`
- **Hook integrity** (`src/hooks/integrity.rs`) — writes SHA-256 hash files for hook verification
- **Trust store** (`src/hooks/trust.rs`) — writes filter trust decisions
- **Config** (`src/core/config.rs` line 159) — writes config.toml
- **Learn/report** (`src/learn/report.rs` lines 68, 108) — writes analytics reports
- **Dotnet** (`src/cmds/dotnet/`) — all in test code

None of these are security-relevant for our hardening scope.

---

## 7. Summary — Patch Target Map

| Task | Files to patch | Call sites | Complexity |
|---|---|---|---|
| ARTK-2: Strip telemetry | `src/core/telemetry.rs`, `Cargo.toml` | 3 (post, salt, marker) | Low — no-op the send function, remove `ureq` dep |
| ARTK-3: Shell injection | `src/cmds/rust/runner.rs`, `src/cmds/system/summary.rs` | 6 (3 sh + 3 cmd) | Medium — replace with direct exec, add sanitize module |
| ARTK-4: Hook permissions | `hooks/claude/rtk-rewrite.sh`, `src/hooks/hook_cmd.rs`, `src/hooks/init.rs` (template) | 3 active (line 93, 132, template) | Medium — allowlist logic in bash + Rust |
| ARTK-5: DB scrubbing | `src/core/tracking.rs` | 2 INSERTs (line 369, 409) | Medium — add scrub module, wrap both inserts |
| ARTK-6: Tee + path | `src/core/tee.rs` | 2 (line 40, 135) | Low — default change + path validation |
| ARTK-7: Exclude commands | `src/hooks/rewrite.rs` (or registry) | 1 entry point | Low — early return for excluded list |

**Total production files requiring patches**: 7 files  
**New files to create**: 2 (`src/core/sanitize.rs`, `src/core/scrub.rs`)

---

## 8. Confirmed: Issue #640 File Path Mapping

The v0.34.3 refactoring moved files. Here's the mapping from the Issue #640 audit to current v0.35.0:

| Issue #640 reference | Current v0.35.0 location | Confirmed |
|---|---|---|
| `src/runner.rs` (lines 14-27, 73-86) | `src/cmds/rust/runner.rs` (lines 17-23, 73-79) | ✅ Pattern identical, lines shifted |
| `src/summary.rs` (lines 15-28) | `src/cmds/system/summary.rs` (lines 18-24) | ✅ Pattern identical, lines shifted |
| `src/telemetry.rs` (line 44) | `src/core/telemetry.rs` (line 81) | ✅ Now uses `ureq` instead of `reqwest` |
| `src/tracking.rs` | `src/core/tracking.rs` | ✅ Same module, new path |
| `src/tee.rs` | `src/core/tee.rs` | ✅ Same module, new path |
| `src/trust.rs` | `src/hooks/trust.rs` | ✅ Moved to hooks subdirectory |
| `src/toml_filter.rs` | Needs verification (`grep -rn 'filters.toml' src/`) | ⬜ Check for ARTK-7 |
