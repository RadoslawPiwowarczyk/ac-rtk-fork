# acoustic-rtk: Developer Onboarding

## What is RTK?

RTK (Rust Token Killer) is a CLI proxy that sits between your AI coding agent (Claude Code, Cursor, Copilot) and your shell. When Claude Code runs a command like `git status`, RTK intercepts it, runs the real command, compresses the output by 60–90%, and returns the compact version. The result: longer sessions, better reasoning (less noise in context), and lower token costs.

`acoustic-rtk` is our security-hardened fork of [rtk-ai/rtk](https://github.com/rtk-ai/rtk). It removes telemetry, fixes a shell injection vulnerability, and adds guardrails appropriate for working with sensitive customer data via MCP connectors.

## What We Changed (and Why)

The upstream RTK binary is designed for individual developers on personal projects. For Acoustic — where Claude Code connects to MongoDB, Planhat, Zendesk, and Slack via MCP — we needed stronger security guarantees. Six patches were applied:

1. **No phone-home**: The binary makes zero network connections. Upstream sends daily usage metrics to an external endpoint — we stripped that entirely.

2. **No shell injection**: Commands like `rtk test` and `rtk err` used to pass arguments through `sh -c`, which meant a prompt injection in a file Claude reads could chain into shell execution. Our fork executes binaries directly — no shell interprets your arguments.

3. **Permission prompts for writes**: The upstream hook auto-approves every command Claude Code runs. Our fork only auto-approves read-only operations (git status, ls, grep, etc.). Write and execute operations (git push, cargo test, npm install) trigger Claude Code's normal permission prompt so you see what's about to happen.

4. **Secrets scrubbed from tracking**: RTK tracks command history for the `rtk gain` analytics dashboard. Our fork redacts bearer tokens, AWS keys, connection strings, and other secrets before storing them.

5. **No raw output files**: Upstream saves full unfiltered command output to disk on failure. We disabled this by default since those files can contain secrets.

6. **Sensitive commands excluded**: `curl`, `env`, `ssh`, and `wget` are never intercepted by RTK — they pass through to Claude Code's native handling.

Full details in [ACOUSTIC-CHANGES.md](../ACOUSTIC-CHANGES.md).

## Installation

Run the install script from the repo root:

```bash
cd ~/Documents/Project/ac-rtk-fork
./scripts/install-acoustic-rtk.sh
```

Or manually:

```bash
cargo build --release
cp target/release/rtk ~/.local/bin/rtk
export RTK_TELEMETRY_DISABLED=1  # belt + suspenders
rtk init -g                      # installs Claude Code hook
```

Then **restart Claude Code**.

## Verify It Works

```bash
./scripts/verify-installation.sh
```

Or manually:

```bash
rtk --version          # Should show 0.35.0
rtk init --show        # Should show hook installed
rtk gain               # Should show token savings (after a few commands)
```

## What You'll Experience

### No change (seamless)

Read-only commands execute silently with compressed output — exactly like vanilla RTK:
- `git status`, `git log`, `git diff` → compressed automatically
- `ls`, `grep`, `find` → compressed automatically
- You won't notice RTK is running. Output is just shorter.

### New: permission prompts

Write and execute commands will trigger Claude Code's permission prompt:
- `git push`, `git commit` → Claude Code asks "Allow?"
- `cargo test`, `npm run build` → Claude Code asks "Allow?"
- This is intentional. Approve normally — the command still gets RTK compression.

### New: compound commands don't work with rtk test/err

This no longer works:
```bash
rtk test "cargo build && cargo test"  # ← blocked
```

Split into separate calls instead:
```bash
rtk test cargo build
rtk test cargo test
```

This is a deliberate security trade-off — the `&&` operator was the injection vector.

### Unchanged: these commands bypass RTK

`curl`, `env`, `ssh`, `wget`, `kubectl exec` — these are never intercepted. Claude Code runs them directly with its normal permission handling.

## Useful Commands

```bash
rtk gain              # Token savings dashboard
rtk gain --graph      # ASCII graph (last 7 days)
rtk gain --history    # Recent command history
rtk discover          # Find commands that could benefit from RTK
rtk init --show       # Verify hook installation
```

## Troubleshooting

**RTK doesn't seem active (verbose output from Claude Code)**:
```bash
rtk init --show       # Check if hook is installed
rtk --version         # Verify correct binary (should be 0.35.0)
which rtk             # Should point to ~/.local/bin/rtk
```

**Want full output for a specific command** (bypass RTK filtering):
```bash
rtk proxy <command>   # Passthrough — raw output, still tracked
```

Or use Claude Code's built-in tools (Read, Grep, Glob) — these bypass the hook entirely.

**Want to re-enable tee files for debugging**:
```bash
# In ~/.config/rtk/config.toml
[tee]
enabled = true
mode = "failures"
```

## Updating

We pin to a specific upstream commit. Updates are manual and audited:

```bash
git fetch upstream
git log upstream/master --oneline -20  # Review changes
# Cherry-pick specific commits after checking ACOUSTIC-CHANGES.md audit guide
```

Never run `git merge upstream/master` — always cherry-pick and review.
