#!/bin/bash
# Install Claude Code hook for kessel-cli.
# Adds PostToolUse and Stop hooks that forward events to kessel-cli's Unix socket.
#
# Usage: bash scripts/install-claude-hook.sh
#
# What it does:
#   1. Copies kessel-cli-hook.sh to ~/.claude/hooks/
#   2. Merges hook config into ~/.claude/settings.json

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
HOOK_SCRIPT="$SCRIPT_DIR/kessel-cli-hook.sh"
DEST_DIR="$HOME/.claude/hooks"
DEST_HOOK="$DEST_DIR/kessel-cli-hook.sh"
SETTINGS="$HOME/.claude/settings.json"

# Check hook script exists
if [ ! -f "$HOOK_SCRIPT" ]; then
    echo "Error: $HOOK_SCRIPT not found"
    exit 1
fi

# Copy hook script
mkdir -p "$DEST_DIR"
cp "$HOOK_SCRIPT" "$DEST_HOOK"
chmod +x "$DEST_HOOK"
echo "Installed hook script: $DEST_HOOK"

# Merge into settings.json
if ! command -v jq &>/dev/null; then
    echo "Error: jq is required. Install with: brew install jq"
    exit 1
fi

# Create settings.json if missing
if [ ! -f "$SETTINGS" ]; then
    echo '{}' > "$SETTINGS"
fi

# Build the hook entry
HOOK_ENTRY=$(jq -n --arg cmd "$DEST_HOOK" '{
    matcher: "",
    hooks: [{
        type: "command",
        command: $cmd,
        timeout: 10,
        async: true
    }]
}')

# Check if kessel-cli hook already exists
if jq -e ".hooks.PostToolUse[]? | select(.hooks[]?.command == \"$DEST_HOOK\")" "$SETTINGS" &>/dev/null; then
    echo "Hook already installed in $SETTINGS"
    exit 0
fi

# Add hooks for PostToolUse and Stop
UPDATED=$(jq --argjson entry "$HOOK_ENTRY" '
    .hooks //= {} |
    .hooks.PostToolUse //= [] |
    .hooks.Stop //= [] |
    .hooks.PostToolUse += [$entry] |
    .hooks.Stop += [$entry]
' "$SETTINGS")

echo "$UPDATED" > "$SETTINGS"
echo "Updated $SETTINGS with PostToolUse and Stop hooks"
echo ""
echo "Done! Kessel will receive Claude Code events when running."
echo "To uninstall: bash $(dirname "$0")/uninstall-claude-hook.sh"
