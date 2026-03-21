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
echo "Gitea:    $GITEA_URL"
echo "Container: $GITEA_CONTAINER"
echo "Bot user:  $BOT_USERNAME"
echo ""

# ── Helpers ────────────────────────────────────────────────────────────────────

# Perform an authenticated API call. Usage: api_call METHOD PATH [BODY]
# Returns the response body. Exits on HTTP error.
api_call() {
    local method="$1"
    local path="$2"
    local body="${3:-}"

    local args=(-s -f -X "$method" -H "Authorization: Bearer $ADMIN_TOKEN" \
                   -H "Content-Type: application/json" \
                   -H "Accept: application/json")
    [[ -n "$body" ]] && args+=(-d "$body")
    curl "${args[@]}" "${GITEA_URL}/api/v1${path}"
}

# ── Step 1: Create Gitea admin user ───────────────────────────────────────────

echo "[1/8] Creating Gitea admin user @${ADMIN_USERNAME}..."

# Check if user already exists via docker exec
if docker exec --user git "$GITEA_CONTAINER" \
       gitea admin user list --admin 2>/dev/null \
       | grep -q "^[0-9]\+[[:space:]]\+${ADMIN_USERNAME}[[:space:]]"; then
    echo "       Already exists — skipped."
else
    docker exec --user git "$GITEA_CONTAINER" gitea admin user create \
        --username "$ADMIN_USERNAME" \
        --password "$ADMIN_PASSWORD" \
        --email    "$ADMIN_EMAIL" \
        --admin \
        --must-change-password=false
    echo "       Created."
fi

# ── Step 2: Create temporary admin token ──────────────────────────────────────

echo "[2/8] Creating temporary admin token..."

# Delete any leftover token from a previous failed run
curl -s -X DELETE \
    -u "${ADMIN_USERNAME}:${ADMIN_PASSWORD}" \
    -H "Accept: application/json" \
    "${GITEA_URL}/api/v1/users/${ADMIN_USERNAME}/tokens/foundry-setup-tmp" \
    > /dev/null 2>&1 || true

ADMIN_TOKEN_RESPONSE=$(curl -s -f -X POST \
    -u "${ADMIN_USERNAME}:${ADMIN_PASSWORD}" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json" \
    -d '{"name":"foundry-setup-tmp"}' \
    "${GITEA_URL}/api/v1/users/${ADMIN_USERNAME}/tokens")

ADMIN_TOKEN=$(echo "$ADMIN_TOKEN_RESPONSE" | jq -r '.sha1')
ADMIN_TOKEN_ID=$(echo "$ADMIN_TOKEN_RESPONSE" | jq -r '.id')

if [[ -z "$ADMIN_TOKEN" || "$ADMIN_TOKEN" == "null" ]]; then
    echo "ERROR: failed to create admin token." >&2
    echo "$ADMIN_TOKEN_RESPONSE" >&2
    exit 1
fi

echo "       Token created (id=${ADMIN_TOKEN_ID})."

# Clean up the temporary token on exit (step 7), even if the script errors.
CLEANUP_DONE=false
cleanup() {
    if [[ "$CLEANUP_DONE" == "false" && -n "${ADMIN_TOKEN_ID:-}" ]]; then
        curl -s -X DELETE \
            -H "Authorization: Bearer $ADMIN_TOKEN" \
            -H "Accept: application/json" \
            "${GITEA_URL}/api/v1/users/${ADMIN_USERNAME}/tokens/${ADMIN_TOKEN_ID}" \
            > /dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

# ── Step 3: Verify connectivity ───────────────────────────────────────────────

echo "[3/8] Verifying Gitea connectivity..."

VERSION=$(api_call GET /version | jq -r '.version')
if [[ -z "$VERSION" || "$VERSION" == "null" ]]; then
    echo "ERROR: could not read Gitea version — check URL and credentials." >&2
    exit 1
fi

echo "       OK (Gitea ${VERSION})."

# ── Step 4: Create bot user ───────────────────────────────────────────────────

echo "[4/8] Creating bot user @${BOT_USERNAME}..."

BOT_EXISTS=$(api_call GET "/users/${BOT_USERNAME}" 2>/dev/null \
    | jq -r '.login // empty')

if [[ -n "$BOT_EXISTS" ]]; then
    echo "       Already exists — skipped."
else
    # Generate a random password — the bot never logs in interactively
    BOT_PASSWORD=$(LC_ALL=C tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 32)
    api_call POST /admin/users \
        "{\"username\":\"${BOT_USERNAME}\",\"email\":\"${BOT_EMAIL}\",\"password\":\"${BOT_PASSWORD}\",\"must_change_password\":false,\"send_notify\":false}" \
        > /dev/null
    echo "       Created."
fi

# ── Step 5: Create bot API token ──────────────────────────────────────────────

echo "[5/8] Creating bot API token 'foundry'..."

EXISTING_TOKEN_INFO=$(api_call GET "/users/${BOT_USERNAME}/tokens" \
    | jq -r '.[] | select(.name=="foundry") | "\(.name):\(.token_last_eight // "")"')

BOT_TOKEN=""
BOT_TOKEN_LAST4=""
BOT_TOKEN_STATUS=""

if [[ -n "$EXISTING_TOKEN_INFO" ]]; then
    BOT_TOKEN_STATUS="already_exists"
    LAST_EIGHT=$(echo "$EXISTING_TOKEN_INFO" | cut -d: -f2)
    BOT_TOKEN_LAST4="${LAST_EIGHT: -4}"
    echo "       Token 'foundry' already exists — skipped."
    echo "       NOTE: The token value cannot be recovered via the API."
    echo "       If you need it, delete and recreate it manually in Gitea."
else
    TOKEN_RESPONSE=$(api_call POST "/users/${BOT_USERNAME}/tokens" \
        '{"name":"foundry","scope":["repository"]}')
    BOT_TOKEN=$(echo "$TOKEN_RESPONSE" | jq -r '.sha1')
    if [[ -z "$BOT_TOKEN" || "$BOT_TOKEN" == "null" ]]; then
        echo "ERROR: failed to create bot token." >&2
        echo "$TOKEN_RESPONSE" >&2
        exit 1
    fi
    BOT_TOKEN_STATUS="created"
    echo "       Created."
fi

# ── Step 6: Register system webhook ───────────────────────────────────────────

echo "[6/8] Registering system webhook..."

EXISTING_HOOK_ID=$(api_call GET /admin/hooks \
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

# ── Step 7: Delete temporary admin token ──────────────────────────────────────

echo "[7/8] Deleting temporary admin token..."

api_call DELETE "/users/${ADMIN_USERNAME}/tokens/${ADMIN_TOKEN_ID}" > /dev/null
CLEANUP_DONE=true   # prevent the EXIT trap from trying again

echo "       Deleted."

# ── Step 8: Print summary ─────────────────────────────────────────────────────

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
    echo "  Bot token 'foundry' already existed (ends in ...${BOT_TOKEN_LAST4})."
    echo "  To get the full value, delete the token in Gitea and re-run this script."
    echo ""
fi

echo "Next steps:"
echo "  1. Set FOUNDRY_GITEA_TOKEN in your foundryd environment."
echo "  2. Add @${BOT_USERNAME} as a Collaborator on each repo you want Foundry to manage."
echo "  3. Docker network and volume setup is handled by docker-compose.yml."
