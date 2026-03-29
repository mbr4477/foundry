#!/usr/bin/env bash
set -euo pipefail

# ── Usage ──────────────────────────────────────────────────────────────────────

usage() {
    cat >&2 <<EOF
Usage: $0 [options]

Initialize a Gitea instance for Foundry.

Options (flags take precedence over env vars):
  --gitea-container NAME    Docker container name running Gitea
                            [env: GITEA_CONTAINER, default: gitea]
  --gitea-url URL           Gitea base URL (e.g. http://localhost:3000)
                            [env: GITEA_URL]
  --admin-username USER     Gitea admin username
                            [env: FOUNDRY_ADMIN_USERNAME]
  --admin-password PASS     Gitea admin password
                            [env: FOUNDRY_ADMIN_PASSWORD]
  --admin-email EMAIL       Gitea admin email
                            [env: FOUNDRY_ADMIN_EMAIL]
  --webhook-url URL         Foundry webhook URL (e.g. http://foundry:8477/webhook)
                            [env: FOUNDRY_WEBHOOK_URL]
  --webhook-secret SECRET   HMAC secret for webhook signatures
                            [env: FOUNDRY_WEBHOOK_SECRET]
  --bot-username USER       Bot account username [env: FOUNDRY_BOT_USERNAME, default: foundry-bot]
  --bot-email EMAIL         Bot account email [env: FOUNDRY_BOT_EMAIL, default: foundry-bot@gitea.local]
  --mcp-config PATH         Path to mcp-config.json to copy into the foundry-shared volume
                            [env: MCP_CONFIG_PATH, default: relative to script location]
  -h, --help                Show this help
EOF
    exit 1
}

# ── Defaults ───────────────────────────────────────────────────────────────────

GITEA_CONTAINER="${GITEA_CONTAINER:-gitea}"
GITEA_URL="${GITEA_URL:-}"
ADMIN_USERNAME="${FOUNDRY_ADMIN_USERNAME:-}"
ADMIN_PASSWORD="${FOUNDRY_ADMIN_PASSWORD:-}"
ADMIN_EMAIL="${FOUNDRY_ADMIN_EMAIL:-}"
WEBHOOK_URL="${FOUNDRY_WEBHOOK_URL:-}"
WEBHOOK_SECRET="${FOUNDRY_WEBHOOK_SECRET:-}"
BOT_USERNAME="${FOUNDRY_BOT_USERNAME:-foundry-bot}"
BOT_EMAIL="${FOUNDRY_BOT_EMAIL:-foundry-bot@gitea.local}"
MCP_CONFIG_PATH="${MCP_CONFIG_PATH:-}"

# ── Argument parsing ───────────────────────────────────────────────────────────

while [[ $# -gt 0 ]]; do
    case "$1" in
        --gitea-container) GITEA_CONTAINER="$2"; shift 2 ;;
        --gitea-url)       GITEA_URL="$2";       shift 2 ;;
        --admin-username)  ADMIN_USERNAME="$2";  shift 2 ;;
        --admin-password)  ADMIN_PASSWORD="$2";  shift 2 ;;
        --admin-email)     ADMIN_EMAIL="$2";     shift 2 ;;
        --webhook-url)     WEBHOOK_URL="$2";     shift 2 ;;
        --webhook-secret)  WEBHOOK_SECRET="$2";  shift 2 ;;
        --bot-username)    BOT_USERNAME="$2";    shift 2 ;;
        --bot-email)       BOT_EMAIL="$2";       shift 2 ;;
        --mcp-config)      MCP_CONFIG_PATH="$2"; shift 2 ;;
        -h|--help)         usage ;;
        *) echo "Unknown option: $1" >&2; usage ;;
    esac
done

# ── Validate required args ─────────────────────────────────────────────────────

missing=()
[[ -z "$GITEA_URL" ]]       && missing+=("--gitea-url / GITEA_URL")
[[ -z "$ADMIN_USERNAME" ]]  && missing+=("--admin-username / FOUNDRY_ADMIN_USERNAME")
[[ -z "$ADMIN_PASSWORD" ]]  && missing+=("--admin-password / FOUNDRY_ADMIN_PASSWORD")
[[ -z "$ADMIN_EMAIL" ]]     && missing+=("--admin-email / FOUNDRY_ADMIN_EMAIL")
[[ -z "$WEBHOOK_URL" ]]     && missing+=("--webhook-url / FOUNDRY_WEBHOOK_URL")
[[ -z "$WEBHOOK_SECRET" ]]  && missing+=("--webhook-secret / FOUNDRY_WEBHOOK_SECRET")

if [[ ${#missing[@]} -gt 0 ]]; then
    echo "ERROR: missing required arguments:" >&2
    for m in "${missing[@]}"; do echo "  $m" >&2; done
    exit 1
fi

GITEA_URL="${GITEA_URL%/}"  # strip trailing slash

echo "=== gitea-init.sh ==="
echo "Gitea:     $GITEA_URL"
echo "Container: $GITEA_CONTAINER"
echo "Bot user:  $BOT_USERNAME"
echo ""

# ── Helpers ────────────────────────────────────────────────────────────────────

# Run a gitea CLI command inside the container as the git user.
gitea_cli() {
	echo "docker exec --user git \"$GITEA_CONTAINER\" gitea \"$@\""
    docker exec --user git "$GITEA_CONTAINER" gitea "$@"
}

# Perform an admin API call using basic auth. Usage: api_call METHOD PATH [BODY]
# Returns the response body. Prints error and exits on curl or HTTP error.
api_call() {
    local method="$1"
    local path="$2"
    local body="${3:-}"

    local _tmp
    _tmp=$(mktemp)
    local args=(-s -o "$_tmp" -w "%{http_code}" -X "$method" \
                   -u "${ADMIN_USERNAME}:${ADMIN_PASSWORD}" \
                   -H "Content-Type: application/json" \
                   -H "Accept: application/json")
    [[ -n "$body" ]] && args+=(-d "$body")

    local _status
    _status=$(curl "${args[@]}" "${GITEA_URL}/api/v1${path}") || {
        echo "ERROR: curl network error for $method /api/v1${path}" >&2
        rm -f "$_tmp"
        return 1
    }

    if [[ "$_status" -lt 200 || "$_status" -ge 300 ]]; then
        echo "ERROR: API $method /api/v1${path} returned HTTP $_status:" >&2
        cat "$_tmp" >&2
        printf '\n' >&2
        rm -f "$_tmp"
        return 1
    fi

    cat "$_tmp"
    rm -f "$_tmp"
}

# ── Step 1: Create Gitea admin user ───────────────────────────────────────────

echo "[1/6] Creating Gitea admin user @${ADMIN_USERNAME}..."

if gitea_cli admin user list --admin 2>/dev/null \
       | grep -q "^[0-9]\+[[:space:]]\+${ADMIN_USERNAME}[[:space:]]"; then
    echo "       Already exists — skipped."
else
    gitea_cli admin user create \
        --username "$ADMIN_USERNAME" \
        --password "$ADMIN_PASSWORD" \
        --email    "$ADMIN_EMAIL" \
        --admin \
        --must-change-password=false
    echo "       Created."
fi

# ── Step 2: Create bot user ───────────────────────────────────────────────────

echo "[2/6] Creating bot user @${BOT_USERNAME}..."

if gitea_cli admin user list 2>/dev/null \
       | grep -q "^[0-9]\+[[:space:]]\+${BOT_USERNAME}[[:space:]]"; then
    echo "       Already exists — skipped."
else
	BOT_PASSWORD=$(LC_ALL=C tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 32 || true)
    gitea_cli admin user create \
        --username "$BOT_USERNAME" \
        --password "$BOT_PASSWORD" \
        --email    "$BOT_EMAIL" \
        --must-change-password=false
    echo "       Created."
fi

# ── Step 3: Create bot API token ──────────────────────────────────────────────

echo "[3/6] Creating bot API token 'foundry'..."

BOT_TOKEN=""
BOT_TOKEN_STATUS=""

EXISTING=$(api_call GET "/users/${BOT_USERNAME}/tokens" \
    | jq -r '.[] | select(.name=="foundry") | .name')

if [[ -n "$EXISTING" ]]; then
    BOT_TOKEN_STATUS="already_exists"
    echo "       Token 'foundry' already exists — skipped."
    echo "       NOTE: The token value cannot be recovered. Delete it in Gitea and re-run to get a new one."
else
    BOT_TOKEN=$(gitea_cli admin user generate-access-token \
        --username  "$BOT_USERNAME" \
        --token-name foundry \
        --raw 2>&1) \
        || { echo "ERROR: failed to generate bot token." >&2; exit 1; }
    if [[ -z "$BOT_TOKEN" || "$BOT_TOKEN" == "null" ]]; then
        echo "ERROR: generate-access-token returned an empty token." >&2
        exit 1
    fi
    BOT_TOKEN_STATUS="created"
    echo "       Created."
fi

# ── Step 4: Register system webhook ───────────────────────────────────────────

echo "[4/6] Registering system webhook..."

EXISTING_HOOK_ID=$(api_call GET "/admin/hooks?type=default" \
    | jq -r --arg url "$WEBHOOK_URL" '.[] | select(.config.url==$url) | .id')

WEBHOOK_EVENTS='["issues","issue_comment","pull_request","pull_request_review"]'
WEBHOOK_CONFIG="{\"url\":\"${WEBHOOK_URL}\",\"content_type\":\"json\",\"secret\":\"${WEBHOOK_SECRET}\"}"

if [[ -n "$EXISTING_HOOK_ID" ]]; then
    api_call PATCH "/admin/hooks/${EXISTING_HOOK_ID}" \
        "{\"config\":${WEBHOOK_CONFIG},\"events\":${WEBHOOK_EVENTS},\"active\":true}" \
        > /dev/null
    echo "       Updated existing webhook (id=${EXISTING_HOOK_ID})."
else
    NEW_HOOK_ID=$(api_call POST /admin/hooks \
        "{\"type\":\"gitea\",\"config\":${WEBHOOK_CONFIG},\"events\":${WEBHOOK_EVENTS},\"active\":true}" \
        | jq -r '.id')
    echo "       Created webhook (id=${NEW_HOOK_ID})."
fi

# ── Step 5: Populate foundry-shared volume ────────────────────────────────────

echo "[5/6] Populating foundry-shared volume with mcp-config.json..."

if [[ -z "$MCP_CONFIG_PATH" ]]; then
    SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
    MCP_CONFIG_PATH="${SCRIPT_DIR}/../foundry/foundry-runner/mcp-config.json"
fi
MCP_CONFIG_SRC="$MCP_CONFIG_PATH"

if [[ ! -f "$MCP_CONFIG_SRC" ]]; then
    echo "ERROR: mcp-config.json not found at ${MCP_CONFIG_SRC}" >&2
    exit 1
fi

docker run --rm \
    -v foundry-shared:/etc/foundry \
    -v "$(realpath "$MCP_CONFIG_SRC"):/tmp/mcp-config.json:ro" \
    alpine:latest \
    cp /tmp/mcp-config.json /etc/foundry/mcp-config.json

echo "       Done."

# ── Step 6: Print summary ─────────────────────────────────────────────────────

echo ""
echo "=== Setup complete ==="
echo ""

if [[ "$BOT_TOKEN_STATUS" == "created" ]]; then
    echo "  ┌─────────────────────────────────────────────────────────────┐"
    echo "  │  SAVE THIS TOKEN — it will not be shown again               │"
    echo "  │                                                             │"
    printf "  │  FOUNDRY_GITEA_TOKEN=%-40s│\n" "$BOT_TOKEN"
    echo "  └─────────────────────────────────────────────────────────────┘"
    echo ""
elif [[ "$BOT_TOKEN_STATUS" == "already_exists" ]]; then
    echo "  Bot token 'foundry' already existed."
    echo "  To get the full value, delete the token in Gitea and re-run this script."
    echo ""
fi

echo "Next steps:"
echo "  1. Set FOUNDRY_GITEA_TOKEN in your foundryd environment."
echo "  2. Add @${BOT_USERNAME} as a Collaborator on each repo you want Foundry to manage."
echo "  3. Start foundryd — the foundry-shared volume is ready with mcp-config.json."
