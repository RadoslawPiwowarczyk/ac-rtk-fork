#!/bin/bash
# install-acoustic-rtk.sh — Build and install the security-hardened RTK fork.
# Run from the repo root: ./scripts/install-acoustic-rtk.sh
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
INSTALL_DIR="$HOME/.local/bin"
BINARY="$INSTALL_DIR/rtk"

echo "=== acoustic-rtk: Security-Hardened RTK Installation ==="
echo ""

# ── Prerequisites ──────────────────────────────────────────────

if ! command -v cargo &>/dev/null; then
    echo "[ERROR] Rust toolchain not found."
    echo "Install with: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    exit 1
fi

if ! command -v jq &>/dev/null; then
    echo "[ERROR] jq not found (required by the Claude Code hook)."
    echo "Install with: sudo apt install jq  (or brew install jq)"
    exit 1
fi

# ── Build ──────────────────────────────────────────────────────

echo "[1/5] Building from source..."
cd "$REPO_DIR"
cargo build --release --quiet

# ── Install binary ─────────────────────────────────────────────

echo "[2/5] Installing binary to $INSTALL_DIR..."
mkdir -p "$INSTALL_DIR"
cp target/release/rtk "$BINARY"
chmod +x "$BINARY"

# Ensure ~/.local/bin is in PATH
if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    echo ""
    echo "[WARN] $INSTALL_DIR is not in your PATH."
    echo "Add this to your ~/.bashrc or ~/.zshrc:"
    echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
    echo ""
    export PATH="$INSTALL_DIR:$PATH"
fi

# ── Configure ──────────────────────────────────────────────────

echo "[3/5] Writing hardened config..."

# Belt + suspenders: env var disable even though telemetry code is stripped
export RTK_TELEMETRY_DISABLED=1

# Write config
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/rtk"
mkdir -p "$CONFIG_DIR"
cat > "$CONFIG_DIR/config.toml" << 'TOML'
# acoustic-rtk hardened configuration
# See ACOUSTIC-CHANGES.md for details on each setting.

[telemetry]
enabled = false

[tee]
enabled = false
# Re-enable for debugging:
# enabled = true
# mode = "failures"

[tracking]
# Retention is enforced in code at 7 days (ACOUSTIC-004).
# database_path can be overridden here if needed.
TOML

# ── Install hook ───────────────────────────────────────────────

echo "[4/5] Installing Claude Code hook..."
rtk init -g --auto-patch

# ── Verify ─────────────────────────────────────────────────────

echo "[5/5] Verifying installation..."
echo ""

RTK_VERSION=$(rtk --version 2>/dev/null || echo "NOT FOUND")
echo "  Binary version:  $RTK_VERSION"

HOOK_STATUS=$(rtk init --show 2>&1 | grep -i 'hook\|installed' | head -1 || echo "unknown")
echo "  Hook status:     $HOOK_STATUS"

echo ""
echo "=== Installation complete ==="
echo ""
echo "Next steps:"
echo "  1. Restart Claude Code"
echo "  2. Run a few commands, then check: rtk gain"
echo "  3. Verify with: ./scripts/verify-installation.sh"
echo ""
