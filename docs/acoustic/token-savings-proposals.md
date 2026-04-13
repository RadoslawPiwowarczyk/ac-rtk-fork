# Builder-Critic: Token Savings Proposals

**Author:** Radoslaw Piwowarczyk
**Date:** 2026-04-11
**Context:** Analysis of token consumption hotspots in builder-critic, informed by RTK (Rust Token Killer) compression techniques and code-review-graph's structural analysis approach.

---

## Current Token Spend Profile

Based on code analysis of the debate loop and agent orchestration:

| Component | % of Total Spend | Notes |
|---|---|---|
| PR diff re-sent every round | ~40% | 32KB max, sent to Builder + Critic each round |
| Debate history accumulation | ~20% | Full objections from all prior rounds carried forward |
| Dependency context | ~15% | Up to 50KB, fetched even for trivial PRs |
| System prompts (repeated) | ~10% | Builder (~1.5K tokens), Critic (~2.5K tokens) every round |
| File content for objections | ~10% | Up to 5 files x 20KB each |
| CLAUDE.md injection | ~5% | Same conventions sent to every agent call |

Prompt caching (already enabled) mitigates some of this, but the cached blocks themselves are large and the debate history is not cacheable (changes every round).

---

## Proposal 1: Truncate Debate History in Round 2+

**Estimated savings: 40-60% on Critic calls in rounds 2+**
**Difficulty: Easy**
**File: `agents/critic.ts`**

### Problem
Critic receives the full text of all prior round objections. By round 4-5, the prompt contains rounds 1-3's complete objection bodies, Builder rebuttals, and assessment prose — most of which Critic has already processed.

### Proposed Change
Replace full objection history with a compact summary format:

```
## Prior Rounds (summarized)
R1-OBJ-1 [CRITICAL] auth.ts:42 — Missing null check on token refresh (RESOLVED by Builder)
R1-OBJ-2 [MAJOR] db.ts:118 — SQL injection via string interpolation (UNRESOLVED)
R2-OBJ-1 [MINOR] utils.ts:5 — Unused import (RESOLVED by Builder)

## Current Round: Full Objections
[... only the latest round's full text ...]
```

This gives Critic enough context to avoid re-raising resolved issues without carrying forward kilobytes of prose.

---

## Proposal 2: Compress Diff in Shared Context Block

**Estimated savings: 30-45% on diff context**
**Difficulty: Medium**
**File: `github/context.ts`**

### Problem
The full unified diff (up to `MAX_DIFF_CHARS = 32,000`) is sent as shared context. Unified diffs include unchanged context lines, file headers, and mode lines that provide little review value.

### Proposed Change
Pre-process the diff before injection:
- Strip unchanged context lines beyond +/- 3 lines around changes
- Collapse long unchanged hunks into `... (47 unchanged lines) ...`
- Remove binary file entries, mode-only changes, and rename-only entries
- For files with >200 changed lines, include only the first 100 + last 20 with a gap marker

Expected reduction: 32KB typical diff compresses to 12-18KB with no loss of review-relevant information.

---

## Proposal 3: Triage-Gate Dependency Context

**Estimated savings: 20-30% on Round 1 for light/standard reviews**
**Difficulty: Easy**
**File: `github/dependency-context.ts`**

### Problem
Dependency context (up to `MAX_TOTAL_DEPENDENCY_CHARS = 50,000`) is fetched for all PRs regardless of triage intensity. Light reviews of config/docs changes don't need cross-file dependency analysis.

### Proposed Change
Skip dependency fetch when:
- Triage intensity is `skip` or `light`
- No changed files match `criticalPaths` patterns
- All changed files are non-code (`.md`, `.yml`, `.json`, `.toml`, config files)

For `standard` intensity, cap at 25KB instead of 50KB.

---

## Proposal 4: Strip Verbose Assessment from Builder Rebuttal Input

**Estimated savings: 10-15% on Builder Round 2+**
**Difficulty: Easy**
**File: `agents/builder.ts`**

### Problem
Builder receives the Critic's full response including detailed assessment prose, reasoning chains, and severity justifications. Builder only needs the objection list and verdict to construct rebuttals.

### Proposed Change
Before passing Critic output to Builder, extract only:
- Objection ID, severity, file, line
- One-sentence description
- Verdict (APPROVED / CHANGES_REQUESTED)

Strip: assessment narrative, confidence scores, dimension-by-dimension analysis.

---

## Proposal 5: Conditional System Prompts in Round 2+

**Estimated savings: 5-10% on agent calls after Round 1**
**Difficulty: Easy**
**File: `prompts.ts`**

### Problem
Full system prompts (including all 11 review dimensions, output format instructions, and severity definitions) are sent every round. By Round 2, both agents have already internalized these instructions.

### Proposed Change
Use abbreviated system prompts for Round 2+:

```typescript
const criticSystemPrompt = round === 1
  ? FULL_CRITIC_SYSTEM_PROMPT       // ~2,500 tokens
  : ABBREVIATED_CRITIC_PROMPT;       // ~800 tokens — just role + output format + "same rules as R1"
```

Note: This interacts with prompt caching — if the full prompt is cache-hit anyway, the savings are smaller. Test both approaches and measure actual API costs.

---

## Proposal 6: Integrate Structural Context from code-review-graph (Future)

**Estimated savings: 60-80% on input tokens per agent call**
**Difficulty: High (new dependency + workflow changes)**

### Concept
Instead of sending the raw 32KB diff to agents, pre-process through code-review-graph's blast-radius analysis:

```
Current flow:
  PR diff (32KB) → Builder → Critic → repeat

Proposed flow:
  PR diff → code-review-graph detect_changes → 450 tokens of structured context
  → Builder (with risk scores, affected flows, test gaps) → Critic → repeat
```

code-review-graph (https://github.com/aipoweredmarketer/code-review-graph) achieves 8.2x average token reduction across real repositories by replacing naive full-file reads with structural analysis (Tree-sitter AST parsing, SQLite knowledge graph, BFS impact radius).

### What This Would Look Like

Add a workflow step before the debate loop:

```yaml
- name: Build code graph
  run: |
    pip install code-review-graph
    code-review-graph build --incremental
    code-review-graph detect-changes --detail minimal --output context.json
```

Then feed `context.json` (risk-scored functions, blast radius, test coverage gaps) to Builder/Critic instead of raw diff. Raw diff still available as fallback for objections that need line-level precision.

### Trade-offs
- Adds ~10-15s to workflow (graph build + analysis)
- New Python dependency in the Actions runner
- Requires code-review-graph to support the repo's languages
- Best suited for large PRs where 32KB diff cap is a bottleneck

---

## Summary: Prioritized Roadmap

| Priority | Proposal | Savings | Effort |
|---|---|---|---|
| 1 | Truncate debate history R2+ | 40-60% on Critic | Easy |
| 2 | Compress diff in shared context | 30-45% on diff | Medium |
| 3 | Triage-gate dependency context | 20-30% on R1 | Easy |
| 4 | Strip Critic prose from Builder input | 10-15% on Builder R2+ | Easy |
| 5 | Conditional system prompts R2+ | 5-10% on all calls | Easy |
| 6 | code-review-graph integration | 60-80% on input | High |

**Proposals 1, 3, 4, 5 are quick wins** — pure code changes, no new dependencies, no architectural impact. Combined, they should reduce per-PR token spend by **40-55%** (accounting for overlap and caching effects).

**Proposal 2** requires careful testing to ensure compressed diffs don't lose review-relevant context.

**Proposal 6** is a longer-term investment that would fundamentally change how context is provided to agents, but offers the largest single improvement.

---

## Measurement Approach

To validate savings, I suggest adding a `--dry-run` mode that:
1. Runs the full debate pipeline
2. Logs token counts at each step (already partially done via `usage-stats.ts`)
3. Compares before/after for each proposal in isolation

The existing cost tracking comment posted on PRs is a good baseline — extend it with per-round breakdowns to identify which rounds are most expensive.
