#!/bin/bash
set -euo pipefail

INSTRUCTION_FILE="/foundry/instruction.json"
MCP_CONFIG="/etc/foundry/mcp-config.json"

# ── Validate inputs ────────────────────────────────────────────────────────────

if [ ! -f "$INSTRUCTION_FILE" ]; then
    echo "ERROR: $INSTRUCTION_FILE not found" >&2
    echo "The dispatcher must write instruction.json to the volume before launching this container." >&2
    exit 1
fi

if ! jq -e '.directive' "$INSTRUCTION_FILE" > /dev/null 2>&1; then
    echo "ERROR: $INSTRUCTION_FILE is missing the 'directive' field or is invalid JSON" >&2
    exit 1
fi

if [ ! -f "$MCP_CONFIG" ]; then
    echo "ERROR: $MCP_CONFIG not found" >&2
    echo "Mount the foundry-shared volume at /etc/foundry/" >&2
    exit 1
fi

# ── Log context ────────────────────────────────────────────────────────────────

PHASE=$(jq -r '.phase' "$INSTRUCTION_FILE")
OWNER=$(jq -r '.repo.owner' "$INSTRUCTION_FILE")
REPO=$(jq -r '.repo.repo' "$INSTRUCTION_FILE")
ISSUE=$(jq -r '.issue_number' "$INSTRUCTION_FILE")

echo "=== Foundry Runner ===" >&2
echo "Phase:  $PHASE" >&2
echo "Repo:   $OWNER/$REPO" >&2
echo "Issue:  #$ISSUE" >&2
echo "MCP:    $MCP_CONFIG" >&2
echo "======================" >&2

# ── Extract directive ──────────────────────────────────────────────────────────

DIRECTIVE=$(jq -r '.directive' "$INSTRUCTION_FILE")

# ── Configure git HTTP authentication ─────────────────────────────────────────

# Write a GIT_ASKPASS helper so git clone/push authenticate over HTTPS
# without embedding credentials in remote URLs.
ASKPASS_FILE="$(mktemp /tmp/git-askpass-XXXXXX.sh)"
cat > "$ASKPASS_FILE" << 'EOF'
#!/bin/bash
case "$1" in
  Username*) echo "${GITEA_BOT_USERNAME}" ;;
  Password*) echo "${GITEA_ACCESS_TOKEN}" ;;
esac
EOF
chmod +x "$ASKPASS_FILE"
export GIT_ASKPASS="$ASKPASS_FILE"

# ── Run Claude Code ────────────────────────────────────────────────────────────

exec claude \
    --dangerously-skip-permissions \
    --mcp-config "$MCP_CONFIG" \
    -p "$DIRECTIVE"
