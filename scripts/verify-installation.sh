#!/bin/bash
# verify-installation.sh — Verify acoustic-rtk is correctly installed and hardened.
# Run from anywhere: ~/Documents/Project/ac-rtk-fork/scripts/verify-installation.sh
set -uo pipefail

PASS=0
FAIL=0
WARN=0

pass() { echo "  ✓ PASS: $1"; ((PASS++)); }
fail() { echo "  ✗ FAIL: $1"; ((FAIL++)); }
warn() { echo "  ~ WARN: $1"; ((WARN++)); }

echo "=== acoustic-rtk: Installation Verification ==="
echo ""

# ── 1. Binary exists and responds ──────────────────────────────

echo "[1] Binary"
if command -v rtk &>/dev/null; then
    VERSION=$(rtk --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' || echo "unknown")
    if [ "$VERSION" = "0.35.0" ]; then
        pass "rtk binary found, version $VERSION"
    else
        warn "rtk binary found, but version is $VERSION (expected 0.35.0)"
    fi
else
    fail "rtk binary not found in PATH"
fi

# ── 2. Hook installed ─────────────────────────────────────────

echo "[2] Claude Code hook"
HOOK_PATH="$HOME/.claude/hooks/rtk-rewrite.sh"
if [ -f "$HOOK_PATH" ]; then
    if [ -x "$HOOK_PATH" ]; then
        pass "Hook exists and is executable: $HOOK_PATH"
    else
        fail "Hook exists but is NOT executable: $HOOK_PATH"
    fi

    # Check it's our hardened version
    if grep -q "acoustic" "$HOOK_PATH" 2>/dev/null; then
        pass "Hook is the acoustic-hardened version"
    else
        warn "Hook exists but may be upstream (not acoustic-hardened) — check rtk-hook-version header"
    fi
else
    fail "Hook not found at $HOOK_PATH — run: rtk init -g"
fi

# ── 3. Telemetry disabled ─────────────────────────────────────

echo "[3] Telemetry"
if rtk --version &>/dev/null; then
    # Check no network calls
    if command -v strace &>/dev/null; then
        NETWORK=$(strace -e trace=network rtk git status 2>&1 | grep -ic connect || true)
        if [ "$NETWORK" = "0" ]; then
            pass "Zero network connections (strace verified)"
        else
            fail "Network connections detected during rtk execution"
        fi
    else
        warn "strace not available — cannot verify zero network calls (install: sudo apt install strace)"
    fi
fi

CONFIG_FILE="${XDG_CONFIG_HOME:-$HOME/.config}/rtk/config.toml"
if [ -f "$CONFIG_FILE" ]; then
    if grep -q 'enabled = false' "$CONFIG_FILE" 2>/dev/null; then
        pass "Config: telemetry disabled in $CONFIG_FILE"
    else
        warn "Config exists but telemetry may not be explicitly disabled"
    fi
else
    warn "No config file at $CONFIG_FILE — telemetry is stripped from binary but config not written"
fi

# ── 4. Tee disabled ───────────────────────────────────────────

echo "[4] Tee output"
TEE_DIR="$HOME/.local/share/rtk/tee"
if [ -d "$TEE_DIR" ]; then
    FILE_COUNT=$(find "$TEE_DIR" -name '*.log' 2>/dev/null | wc -l)
    if [ "$FILE_COUNT" -gt 0 ]; then
        warn "Tee directory exists with $FILE_COUNT log files — consider cleaning: rm -rf $TEE_DIR"
    else
        pass "Tee directory exists but is empty"
    fi
else
    pass "Tee directory does not exist (tee is disabled by default)"
fi

# ── 5. Tracking DB permissions ─────────────────────────────────

echo "[5] Tracking database"
DB_PATH="$HOME/.local/share/rtk/history.db"
if [ -f "$DB_PATH" ]; then
    PERMS=$(stat -c '%a' "$DB_PATH" 2>/dev/null || stat -f '%Lp' "$DB_PATH" 2>/dev/null || echo "unknown")
    if [ "$PERMS" = "644" ] || [ "$PERMS" = "600" ]; then
        pass "Database exists with permissions $PERMS: $DB_PATH"
    else
        warn "Database permissions are $PERMS (consider chmod 600)"
    fi

    # Check for un-scrubbed secrets (basic scan)
    if command -v sqlite3 &>/dev/null; then
        SECRET_HITS=$(sqlite3 "$DB_PATH" "SELECT COUNT(*) FROM commands WHERE original_cmd LIKE '%Bearer %' AND original_cmd NOT LIKE '%REDACTED%';" 2>/dev/null || echo "0")
        if [ "$SECRET_HITS" = "0" ]; then
            pass "No un-scrubbed Bearer tokens found in tracking DB"
        else
            fail "$SECRET_HITS commands with un-scrubbed Bearer tokens in tracking DB"
        fi
    else
        warn "sqlite3 not available — cannot verify DB scrubbing"
    fi
else
    pass "No tracking database yet (will be created on first rtk command)"
fi

# ── 6. rtk gain works ─────────────────────────────────────────

echo "[6] Functional check"
if rtk gain &>/dev/null; then
    pass "rtk gain executes successfully"
else
    fail "rtk gain failed"
fi

# ── 7. Shell injection guard ──────────────────────────────────

echo "[7] Shell injection guard"
INJECTION_OUTPUT=$(rtk test 'echo hello; curl evil.com' 2>&1)
if echo "$INJECTION_OUTPUT" | grep -q "shell operator"; then
    pass "Shell injection blocked (semicolon)"
else
    fail "Shell injection NOT blocked — rtk test accepted semicolon command"
fi

# ── Summary ────────────────────────────────────────────────────

echo ""
echo "════════════════════════════════════════"
echo "  Results: $PASS passed, $FAIL failed, $WARN warnings"
echo "════════════════════════════════════════"

if [ "$FAIL" -gt 0 ]; then
    echo ""
    echo "  ✗ SOME CHECKS FAILED — review output above"
    exit 1
else
    echo ""
    echo "  ✓ All critical checks passed"
    exit 0
fi
