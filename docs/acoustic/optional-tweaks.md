# Optional Tweaks — Further RTK Optimization

These are improvements beyond the core security hardening (ACOUSTIC-001 through ACOUSTIC-008). None are required — apply them per-project or per-developer as needed.

---

## 1. CLAUDE.md Shell Preferences (High Impact, Easy)

**Problem**: Claude Code v2.1+ prefers built-in tools (Read, Grep, Glob) over Bash commands. These bypass the RTK hook entirely — typically 70-80% of operations never touch RTK.

**Fix**: Add to your project's `CLAUDE.md` (or `~/.claude/CLAUDE.md` for global):

```markdown
## Shell Preferences (RTK Token Compression)

Claude Code should prefer Bash commands over built-in tools to enable RTK output compression:

- Use `cat`, `head -N`, `tail -N` to read files instead of the Read tool
- Use `grep -rn` or `rg` to search code instead of the Grep tool
- Use `find` or `ls` to list files instead of the Glob tool
- Use `wc -l` to count lines instead of reading entire files

This routes all operations through RTK's PreToolUse hook, reducing token consumption by 60-90% on command output. RTK is installed globally — no project setup needed.
```

**Expected impact**: Shifts RTK interception from ~20% to ~80% of all commands. Overall savings should increase from ~35% to 60-80%.

**Trade-off**: Built-in tools are slightly faster (no shell overhead). For most workflows the token savings outweigh the ~10ms per command.

---

## 2. Explicit `rtk test` for Test Runners (High Impact, Easy)

**Problem**: Even with rewrite rules, Claude sometimes runs test commands in ways the registry doesn't match (e.g., `cd project && npx jest` as a compound, or custom test scripts).

**Fix**: Add to your project's `CLAUDE.md`:

```markdown
## Test execution
Always run tests through RTK's test filter for compressed output:
- `rtk test pnpm test` instead of `pnpm test`
- `rtk test npx jest` instead of `npx jest`  
- `rtk test npx jest --testPathPattern=<pattern>` for specific tests
The rtk test wrapper shows failures only — 90%+ token savings on passing suites.
```

**Expected impact**: Test runs with 2000+ lines of output compress to ~20 lines (failures only). This is the single biggest token saver for active development sessions.

---

## 3. Project-Local TOML Filters (Medium Impact, Medium Effort)

**Problem**: Some commands pass through RTK as transparent proxies (tracking only, no compression) because they lack dedicated Rust filter modules. Examples: `rtk gradle`, `rtk mvn`, `rtk npm run build`.

**Fix**: Create `.rtk/filters.toml` in your project root:

```toml
schema_version = 1

# NestJS build — strip progress lines, keep errors
[filters.nest-build]
description = "Compact NestJS build output"
match_command = "^rtk npm.*build"
strip_ansi = true
strip_lines_matching = [
    "^\\s*$",
    "^Compiling",
    "^\\[Nest\\].*LOG",
    "^\\[Nest\\].*Dependencies initialized",
    "^webpack .* compiled",
]
max_lines = 30
on_empty = "build: ok"

# Gradle build — strip download progress, keep errors/warnings
[filters.gradle-build]
description = "Compact Gradle build output"
match_command = "^rtk gradle"
strip_ansi = true
strip_lines_matching = [
    "^Downloading",
    "^Download ",
    "^> Task :\\w+:compile",
    "^BUILD SUCCESSFUL",
]
max_lines = 40
on_empty = "gradle: ok"

# Maven build — strip download progress
[filters.mvn-build]
description = "Compact Maven build output"
match_command = "^rtk mvn"
strip_ansi = true
strip_lines_matching = [
    "^Downloading from",
    "^Downloaded from",
    "^\\[INFO\\] --- ",
    "^\\[INFO\\] BUILD SUCCESS",
    "^\\[INFO\\] ------",
]
max_lines = 40
on_empty = "mvn: ok"

# pnpm install — strip resolution progress
[filters.pnpm-install]
description = "Compact pnpm install output"
match_command = "^rtk pnpm install"
strip_ansi = true
strip_lines_matching = [
    "^Packages: ",
    "^Progress:",
    "^\\s*$",
    "Already up to date",
]
max_lines = 20
on_empty = "pnpm install: ok (no changes)"
```

**Note**: Project-local filters require trust approval on first load (`rtk` will prompt). The SHA-256 pinning mechanism verifies the file hasn't been tampered with.

**Expected impact**: 40-70% compression on build/install commands that currently pass through unfiltered.

---

## 4. Global TOML Filters (Medium Impact, Easy)

**Problem**: You want filters to apply across all projects without committing `.rtk/filters.toml` to each repo.

**Fix**: Create `~/.config/rtk/filters.toml`:

```toml
schema_version = 1

# Suppress npm install noise globally
[filters.npm-install-global]
description = "Compact npm/pnpm install"
match_command = "^rtk (npm|pnpm) install"
strip_ansi = true
strip_lines_matching = ["^added \\d+", "^npm warn", "^\\s*$"]
max_lines = 10
on_empty = "install: ok"

# Compact docker compose output
[filters.docker-compose]
description = "Compact docker compose logs"
match_command = "^rtk docker compose"
strip_lines_matching = ["^\\s*$", "^Attaching to"]
max_lines = 50
```

**Note**: Our ACOUSTIC-007 patch added SHA-256 integrity checking to global filters (same protection as project-local filters).

---

## 5. Command-Specific CLAUDE.md Hints (Low Impact, Easy)

For commands that RTK doesn't cover well, teach Claude to use RTK's generic wrappers:

```markdown
## Command compression hints
- For any command with verbose output: `rtk summary <command>` gives a heuristic summary
- For build errors only: `rtk err <command>` shows stderr/errors only
- For long log files: `rtk log <logfile>` deduplicates repeated lines
- For JSON output: `rtk json <file>` shows structure without values
```

---

## 6. Hook Allowlist Tuning (Low Impact, Easy)

The ACOUSTIC-003 hook allowlist can be extended per-team. If your team trusts certain write commands (e.g., `git add` is low-risk), edit `hooks/claude/rtk-rewrite.sh` and add them to the auto-allow case:

```bash
    git)
      case "$RTK_SUB" in
        status|log|diff|branch|show|remote|tag|stash|add)  # added: add
          AUTO_ALLOW=true ;;
      esac
      ;;
```

**Trade-off**: Every command added to auto-allow is one less permission prompt. Balance convenience vs. security for your team's risk tolerance.

---

## 7. Future: Custom Rust Filter Modules (High Impact, High Effort)

For maximum compression on frequently-used commands, you can write dedicated Rust filter modules. The pattern is:

1. Create `src/cmds/<ecosystem>/mycommand_cmd.rs`
2. Implement `pub fn run(args: &[String], verbose: u8) -> Result<i32>` using `runner::run_filtered()`
3. Write a `filter_mycommand(output: &str) -> String` function
4. Add a `Commands::MyCommand` variant to `main.rs`
5. The rewrite rule in `rules.rs` already points to `rtk mycommand`

**Candidates for Acoustic**:

| Command | Current state | Potential savings | Effort |
|---|---|---|---|
| `rtk gradle` | Passthrough (tracking only) | 70-80% (strip download progress, show errors) | 1-2 days |
| `rtk mvn` | Passthrough (tracking only) | 70-80% (strip downloads, show errors/warnings) | 1-2 days |
| Jest via `rtk vitest` | Uses vitest filter | Works but jest output format differs slightly | 0.5 days |

This is only worth doing if `rtk gain` shows these commands dominating your token usage after applying tweaks 1-3.

---

## 8. Monitoring: What to Watch

After applying any of these tweaks, use these commands to measure impact:

```bash
# Overall savings dashboard
rtk gain

# See which commands save the most (focus effort here)
rtk gain --history

# Find commands that bypass RTK (opportunities)
rtk discover

# Daily trend (are savings improving over time?)
rtk gain --daily

# Per-project breakdown
rtk gain --project .
```

**Target metrics**:
- Overall savings: 60-80% (with shell preferences in CLAUDE.md)
- Test runs: 90%+ (with `rtk test` wrapper)
- Build commands: 40-70% (with TOML filters)
- File operations: 60-85% (with shell preferences routing through Bash)

---

## Priority Order

If you're picking which tweaks to apply first:

1. **CLAUDE.md shell preferences** — biggest bang, 2 minutes to apply
2. **`rtk test` instruction** — huge savings on test-heavy sessions
3. **Project TOML filters** — for your specific noisy commands
4. **Global TOML filters** — for cross-project patterns
5. Everything else — only if `rtk gain` shows remaining gaps
