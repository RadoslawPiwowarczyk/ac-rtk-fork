#!/usr/bin/env bash
# rtk-hook-version: 4-acoustic
# RTK Claude Code hook — rewrites commands to use rtk for token savings.
# ACOUSTIC-003: Selective auto-allow — read-only commands only.
# Requires: rtk >= 0.23.0, jq
#
# This is a thin delegating hook: all rewrite logic lives in `rtk rewrite`,
# which is the single source of truth (src/discover/registry.rs).
#
# Exit code protocol for `rtk rewrite`:
#   0 + stdout  Rewrite found, no deny/ask rule matched
#   1           No RTK equivalent → pass through unchanged
#   2           Deny rule matched → pass through
#   3 + stdout  Ask rule matched → rewrite but prompt user

if ! command -v jq &>/dev/null; then
  echo "[rtk] WARNING: jq is not installed. Hook cannot rewrite commands. Install jq: https://jqlang.github.io/jq/download/" >&2
  exit 0
fi

if ! command -v rtk &>/dev/null; then
  echo "[rtk] WARNING: rtk is not installed or not in PATH. Hook cannot rewrite commands. Install: https://github.com/rtk-ai/rtk#installation" >&2
  exit 0
fi

# Version guard: rtk rewrite was added in 0.23.0.
RTK_VERSION=$(rtk --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)
if [ -n "$RTK_VERSION" ]; then
  MAJOR=$(echo "$RTK_VERSION" | cut -d. -f1)
  MINOR=$(echo "$RTK_VERSION" | cut -d. -f2)
  if [ "$MAJOR" -eq 0 ] && [ "$MINOR" -lt 23 ]; then
    echo "[rtk] WARNING: rtk $RTK_VERSION is too old (need >= 0.23.0). Upgrade: cargo install rtk" >&2
    exit 0
  fi
fi

INPUT=$(cat)
CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

if [ -z "$CMD" ]; then
  exit 0
fi

# Delegate rewrite + permission logic to the Rust binary.
REWRITTEN=$(rtk rewrite "$CMD" 2>/dev/null)
EXIT_CODE=$?

case $EXIT_CODE in
  0)
    [ "$CMD" = "$REWRITTEN" ] && exit 0
    ;;
  1|2)
    exit 0
    ;;
  3)
    ;; # Ask — handled below
  *)
    exit 0
    ;;
esac

# ── ACOUSTIC-003: Read-only allowlist ──────────────────────────
# Only auto-allow commands that are strictly read-only.
# Everything else gets rewritten but the user is prompted (ask).
#
# We extract the rtk subcommand to classify:
#   "rtk git status" → RTK_CMD="git", RTK_SUB="status"
#   "rtk ls ."       → RTK_CMD="ls",  RTK_SUB=""
#   "rtk test cargo" → RTK_CMD="test", RTK_SUB="cargo"

AUTO_ALLOW=false

if [ "$EXIT_CODE" -eq 0 ] || [ "$EXIT_CODE" -eq 3 ]; then
  RTK_CMD=$(echo "$REWRITTEN" | awk '{print $2}')
  RTK_SUB=$(echo "$REWRITTEN" | awk '{print $3}')

  case "$RTK_CMD" in
    git)
      case "$RTK_SUB" in
        status|log|diff|branch|show|remote|tag|stash)
          AUTO_ALLOW=true ;;
      esac
      ;;
    gh)
      case "$RTK_SUB" in
        pr|issue|run)
          # gh pr list, gh issue list, gh run list — read-only
          AUTO_ALLOW=true ;;
      esac
      ;;
    ls|read|grep|find|deps|gain|discover|json|wc|smart|diff|session)
      AUTO_ALLOW=true
      ;;
    # NEVER auto-allow: test, err, summary, proxy, env, curl, docker, kubectl,
    # cargo (build/clippy/test), lint, tsc, npm, pnpm, pip, go, etc.
  esac
fi

# ── Build output JSON ──────────────────────────────────────────

ORIGINAL_INPUT=$(echo "$INPUT" | jq -c '.tool_input')
UPDATED_INPUT=$(echo "$ORIGINAL_INPUT" | jq --arg cmd "$REWRITTEN" '.command = $cmd')

if [ "$AUTO_ALLOW" = true ]; then
  # Read-only command — auto-allow, no user prompt
  jq -n \
    --argjson updated "$UPDATED_INPUT" \
    '{
      "hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "allow",
        "permissionDecisionReason": "RTK auto-rewrite (read-only)",
        "updatedInput": $updated
      }
    }'
else
  # Write/execute/sensitive command — rewrite but prompt user
  jq -n \
    --argjson updated "$UPDATED_INPUT" \
    '{
      "hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "updatedInput": $updated
      }
    }'
fi