# Acoustic RTK Setup Guide

## Why RTK Needs Two Hooks

RTK compresses CLI output before it reaches Claude's context window. But Claude Code has built-in tools (Read, Grep, Glob) that bypass the shell entirely — RTK never sees them:

```
┌──────────────────────────────────────────────────────────────┐
│ Claude Code has TWO ways to read files:                      │
│                                                              │
│ 1. Built-in Read tool → bypasses shell → NO RTK → 0% savings │
│ 2. Bash cat/head/tail → shell → RTK hook intercepts → savings │
│                                                              │
│ Same for search (Grep tool vs grep/rg)                       │
│ Same for file listing (Glob tool vs find/ls)                 │
└──────────────────────────────────────────────────────────────┘
```

The solution is two hooks working together:

1. **RTK rewrite hook** (on Bash) — intercepts shell commands, rewrites `cat` → `rtk read`, `grep` → `rtk grep`, etc.
2. **Force-bash hook** (on Read/Grep/Glob) — rejects built-in tool calls with `exit 2`, telling Claude to use Bash equivalents instead. Claude retries via Bash, which the RTK hook intercepts.

```
Claude wants to read file.java
  → tries Read tool
    → force-bash hook rejects: "Use cat via Bash instead"
  → retries: cat file.java
    → RTK hook rewrites: rtk read file.java
      → minimal filter strips comments/blanks → 15-22% savings
```

**CLAUDE.md instructions alone are unreliable** — Claude sometimes ignores them and uses built-in tools anyway. The hook approach is deterministic.

## Step 1: Install the Acoustic RTK Binary

```bash
cd /path/to/ac-rtk-fork
cargo build --release
cp ./target/release/rtk ~/.local/bin/rtk

# Verify
rtk --version    # Should show 0.35.0 (or your fork version)
rtk gain         # Should show token savings stats
```

Ensure `~/.local/bin` is in your PATH:
```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

## Step 2: Install the RTK Rewrite Hook

```bash
rtk init -g
```

This creates:
- `~/.claude/hooks/rtk-rewrite.sh` — the PreToolUse hook that rewrites Bash commands
- `~/.claude/RTK.md` — awareness file for meta commands (rtk gain, rtk discover)
- Updates `~/.claude/settings.json` with the Bash hook registration

## Step 3: Create the Force-Bash Hook

Create `~/.claude/hooks/force-bash-for-rtk.sh`:

```bash
cat > ~/.claude/hooks/force-bash-for-rtk.sh << 'EOF'
#!/usr/bin/env bash
INPUT=$(cat)
TOOL=$(echo "$INPUT" | jq -r '.tool_name // empty')
case "$TOOL" in
  Read)
    FILE=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty')
    echo "Use cat $FILE via Bash instead of Read — RTK compresses the output." >&2
    exit 2
    ;;
  Grep)
    echo "Use grep -rn via Bash instead of Grep — RTK compresses the output." >&2
    exit 2
    ;;
  Glob)
    echo "Use find or ls via Bash instead of Glob — RTK compresses the output." >&2
    exit 2
    ;;
  *)
    exit 0
    ;;
esac
EOF
chmod +x ~/.claude/hooks/force-bash-for-rtk.sh
```

How it works:
- `exit 2` tells Claude Code the tool call was rejected
- The stderr message tells Claude what to use instead
- Claude retries with the Bash equivalent, which the RTK hook intercepts

## Step 4: Register Both Hooks in settings.json

Edit `~/.claude/settings.json` to register hooks on all four tool types:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "~/.claude/hooks/rtk-rewrite.sh"
          }
        ]
      },
      {
        "matcher": "Read",
        "hooks": [
          {
            "type": "command",
            "command": "~/.claude/hooks/force-bash-for-rtk.sh"
          }
        ]
      },
      {
        "matcher": "Grep",
        "hooks": [
          {
            "type": "command",
            "command": "~/.claude/hooks/force-bash-for-rtk.sh"
          }
        ]
      },
      {
        "matcher": "Glob",
        "hooks": [
          {
            "type": "command",
            "command": "~/.claude/hooks/force-bash-for-rtk.sh"
          }
        ]
      }
    ]
  }
}
```

**Note**: `rtk init -g` only registers the Bash hook. You must manually add the Read/Grep/Glob entries. If you re-run `rtk init -g`, verify it doesn't overwrite your additional hooks.

## Step 5: Add CLAUDE.md Instructions

The hooks handle routing, but CLAUDE.md instructions help Claude make better first-attempt choices (fewer rejected-then-retried calls):

```markdown
## RTK Output Compression

RTK is installed globally and compresses Bash command output by 60-90%.
For this to work, Claude MUST use Bash commands instead of built-in tools:

- Use `cat`, `head -N`, `tail -N` to read files instead of the Read tool
- Use `grep -rn` or `rg` to search code instead of the Grep tool
- Use `find` or `ls` to list files instead of the Glob tool

The RTK hook transparently rewrites these to compressed equivalents.
Built-in tools (Read, Grep, Glob) bypass the hook and get no compression.

**Test execution** — CRITICAL for token savings:
- NEVER run `pnpm test` or `npx jest` directly
- ALWAYS use `rtk test pnpm test` or `rtk test npx jest`
- This filters output to show failures only — 90% token reduction on passing suites
- For specific tests: `rtk test npx jest <pattern>`
```

With both hooks in place, CLAUDE.md is a performance optimization (avoids the reject-retry round trip), not a correctness requirement. The hooks guarantee RTK is always in the path.

## Step 6: Restart Claude Code and Verify

**Restart Claude Code** — hooks are loaded at startup.

Start a session and ask Claude to review a file (don't specify `cat` — let it try the natural way):

```
Review the file src/main/SomeFile.java — summarize the main responsibilities.
```

Claude should:
1. Try the Read tool → force-bash hook rejects it
2. Retry with `cat src/main/SomeFile.java`
3. RTK rewrite hook rewrites to `rtk read src/main/SomeFile.java`
4. Compressed output enters context

Then check savings:
```bash
rtk gain
```

### Expected `rtk gain` Output (Healthy Session)

```
By Command
 #  Command              Count  Saved    Avg%    Time
 1. rtk read               27  161.0K   15.1%    0ms   ████████
 2. rtk grep                5    4.4K   57.7%    1ms   ██
 3. rtk run-test            2    1.4K   97.1%   2.5s   █
 4. rtk git diff HEAD       1     250   10.3%    8ms
```

If `rtk read` shows **0.0%**, the binary's filter default is wrong — rebuild from the fork with ACOUSTIC-010 applied.

## How RTK Compresses Different Commands

| Command | Hook rewrites to | Filter strategy | Typical savings |
|---|---|---|---|
| `cat file.java` | `rtk read file.java` | Strip comments, blank lines | 15-22% (Java), 5% (clean TS) |
| `grep -rn pattern .` | `rtk grep pattern .` | Group by file, truncate lines | 50-58% |
| `ls -la` | `rtk ls -la` | Tree grouping, noise dirs collapsed | 68-80% |
| `find . -name "*.ts"` | `rtk find "*.ts" .` | Compact tree, .gitignore respected | 78% |
| `git status` | `rtk git status` | Compact format | 80% |
| `git diff` | `rtk git diff` | Condensed hunks | 60-75% |
| `git log -n 10` | `rtk git log -n 10` | One-line commits | 80% |
| `git push` | `rtk git push` | → "ok main" | 92% |
| `pnpm test` (via rtk test) | `rtk test pnpm test` | Failures only | 90-97% |

## Troubleshooting

### `rtk read` shows 0% savings
The binary's `--level` default is `none` instead of `minimal`. This is an upstream bug (v0.35.0). Our fork fixes this with ACOUSTIC-010. Rebuild from the fork.

### Claude still uses built-in Read/Grep/Glob without retrying
The hooks aren't registered or Claude Code wasn't restarted:
```bash
cat ~/.claude/settings.json | jq '.hooks.PreToolUse[].matcher'
# Should show: "Bash", "Read", "Grep", "Glob"
```
If any are missing, add them and restart Claude Code.

### Hook outdated warning in `rtk gain`
```bash
rtk init -g
# Then verify your Read/Grep/Glob hooks are still in settings.json
# rtk init -g may overwrite — re-add if needed
# Restart Claude Code
```

### `jq` not found
The force-bash hook requires `jq`:
```bash
sudo apt install jq    # Ubuntu/Debian
brew install jq        # macOS
```

### Claude loops — keeps trying Read, getting rejected, trying Read again
This shouldn't happen — `exit 2` with a stderr message tells Claude to use a different approach. If it does loop, check that the hook is working:
```bash
echo '{"tool_name":"Read","tool_input":{"file_path":"test.txt"}}' | ~/.claude/hooks/force-bash-for-rtk.sh
# Should print message to stderr and exit 2
echo $?  # Should be 2
```

## Security Notes

This guide uses the Acoustic hardened fork (`ac-rtk-fork`). Key differences from upstream:
- **No telemetry** (ACOUSTIC-001)
- **No shell injection** (ACOUSTIC-002)
- **Read-only auto-allow, write operations prompt** (ACOUSTIC-003)
- **Secrets scrubbed from tracking DB** (ACOUSTIC-004)
- **Tee disabled by default** (ACOUSTIC-005)
- **Minimal filter default** (ACOUSTIC-010)

See `acoustic-changes.md` in the fork repo for full patch details.
