#!/bin/sh
set -e

REPO="mbr4477/foundry"
BINARY="foundryd"
INSTALL_DIR="${INSTALL_DIR:-${HOME}/.local/bin}"
CONFIG_DIR="${HOME}/.config/foundry"
FOUNDRY_REF="${FOUNDRY_REF:-main}"
RAW_BASE="https://raw.githubusercontent.com/${REPO}/refs/heads/${FOUNDRY_REF}"

# ── Helpers ────────────────────────────────────────────────────────────────────

say()  { printf '%s\n' "$*"; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }
check() { command -v "$1" >/dev/null 2>&1; }

# ── 1. Prerequisites ───────────────────────────────────────────────────────────

say "Checking prerequisites..."

# Downloader
if check curl; then
    DOWNLOAD="curl -fsSL"
elif check wget; then
    DOWNLOAD="wget -qO-"
else
    die "curl or wget is required but neither was found"
fi

# Docker
if ! check docker; then
    die "Docker is required but was not found. Install Docker: https://docs.docker.com/get-docker/"
fi
if ! docker info >/dev/null 2>&1; then
    die "Docker is installed but the daemon is not running. Start Docker and retry."
fi

# bash (required for gitea-init.sh)
if ! check bash; then
    die "bash is required for Gitea initialization but was not found"
fi

# ── 2. Detect platform ─────────────────────────────────────────────────────────

OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}-${ARCH}" in
    Linux-x86_64)   ASSET="foundryd-linux-amd64" ;;
    Darwin-arm64)   ASSET="foundryd-macos-arm64" ;;
    Darwin-aarch64) ASSET="foundryd-macos-arm64" ;;
    *) die "Unsupported platform: ${OS} ${ARCH}. Supported: Linux x86_64, macOS arm64" ;;
esac

# ── 3. Resolve version ─────────────────────────────────────────────────────────

if [ -n "${FOUNDRYD_VERSION:-}" ]; then
    VERSION="${FOUNDRYD_VERSION}"
else
    say "Fetching latest release..."
    VERSION=$(${DOWNLOAD} "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep '"tag_name"' \
        | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')
    [ -n "${VERSION}" ] || die "Could not determine latest release version"
fi

say "Installing ${BINARY} ${VERSION} (${ASSET})..."

# ── 4. Download binary ─────────────────────────────────────────────────────────

TMP_FILE="$(mktemp)"
TMP_DIR="$(mktemp -d)"
trap 'rm -f "${TMP_FILE}"; rm -rf "${TMP_DIR}"' EXIT

DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${VERSION}/${ASSET}"

if check curl; then
    curl -fsSL "${DOWNLOAD_URL}" -o "${TMP_FILE}"
else
    wget -qO "${TMP_FILE}" "${DOWNLOAD_URL}"
fi

chmod +x "${TMP_FILE}"

# ── 5. Install binary ──────────────────────────────────────────────────────────

mkdir -p "${INSTALL_DIR}"
cp "${TMP_FILE}" "${INSTALL_DIR}/${BINARY}"
say "Installed ${BINARY} to ${INSTALL_DIR}/${BINARY}"

# Warn if INSTALL_DIR is not on PATH
case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *) say "Note: add ${INSTALL_DIR} to your PATH if it isn't already:"
       say "  export PATH=\"\$PATH:${INSTALL_DIR}\"" ;;
esac

# ── 6. Load / create config ────────────────────────────────────────────────────

say ""
say "Configuring Foundry..."

mkdir -p "${CONFIG_DIR}"

if [ ! -f "${CONFIG_DIR}/config.env" ]; then
    cat > "${CONFIG_DIR}/config.env" <<'EOF'
GITEA_CONTAINER=gitea
GITEA_URL=http://localhost:3000
FOUNDRY_ADMIN_USERNAME=gitea-admin
FOUNDRY_ADMIN_PASSWORD=changeme
FOUNDRY_ADMIN_EMAIL=gitea.admin@gitea.local
FOUNDRY_WEBHOOK_URL=http://host.docker.internal:8477/webhook
FOUNDRY_WEBHOOK_SECRET=
FOUNDRY_BOT_USERNAME=foundry-bot
FOUNDRY_BOT_EMAIL=foundry-bot@gitea.local
FOUNDRY_GITEA_TOKEN=
EOF
    say ""
    say "Created ${CONFIG_DIR}/config.env with default values."
    say "Edit it (especially FOUNDRY_ADMIN_PASSWORD), then re-run the installer."
    say ""
    say "  \$EDITOR ${CONFIG_DIR}/config.env"
    exit 0
fi

# shellcheck source=/dev/null
. "${CONFIG_DIR}/config.env"

# Auto-generate webhook secret if not set
if [ -z "${FOUNDRY_WEBHOOK_SECRET:-}" ]; then
    if check openssl; then
        FOUNDRY_WEBHOOK_SECRET="$(openssl rand -hex 32)"
    else
        FOUNDRY_WEBHOOK_SECRET="$(tr -dc 'a-f0-9' < /dev/urandom | head -c 64)"
    fi
    # Write it back to config.env
    if check sed; then
        sed -i.bak "s/^FOUNDRY_WEBHOOK_SECRET=.*/FOUNDRY_WEBHOOK_SECRET=${FOUNDRY_WEBHOOK_SECRET}/" \
            "${CONFIG_DIR}/config.env" && rm -f "${CONFIG_DIR}/config.env.bak"
    fi
    say "Generated webhook secret and saved to config.env"
fi

# ── 7. Write app.ini ──────────────────────────────────────────────────────────

if [ ! -f "${CONFIG_DIR}/app.ini" ]; then
    cat > "${CONFIG_DIR}/app.ini" <<'EOF'
APP_NAME = Gitea
WORK_PATH = /data/gitea

[database]
DB_TYPE = sqlite3
PATH = /data/gitea/gitea.db

[server]
DOMAIN = localhost
HTTP_PORT = 3000
ROOT_URL = http://localhost:3000/

[webhook]
ALLOWED_HOST_LIST = *
SKIP_TLS_VERIFY = true

[security]
INSTALL_LOCK = true

[log]
MODE = console
LEVEL = info
ROOT_PATH = /data/gitea/log
EOF
    say "Wrote ${CONFIG_DIR}/app.ini"
fi

# ── 8. Write docker-compose.yml ───────────────────────────────────────────────

if [ ! -f "${CONFIG_DIR}/docker-compose.yml" ]; then
    cat > "${CONFIG_DIR}/docker-compose.yml" <<EOF
# Foundry — managed by install.sh. Safe to edit.
services:
  gitea:
    image: gitea/gitea:latest
    container_name: ${GITEA_CONTAINER:-gitea}
    restart: unless-stopped
    environment:
      - USER_UID=1000
      - USER_GID=1000
    volumes:
      - gitea-data:/data
      - ./app.ini:/data/gitea/conf/app.ini
      - /etc/timezone:/etc/timezone:ro
      - /etc/localtime:/etc/localtime:ro
    extra_hosts:
      - "host.docker.internal:host-gateway"
    ports:
      - "3000:3000"
      - "2222:22"
    networks:
      - foundry-net
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:3000/"]
      interval: 10s
      timeout: 5s
      retries: 10
      start_period: 30s

networks:
  foundry-net:
    driver: bridge
    name: foundry-net

volumes:
  gitea-data:
    driver: local
  foundry-shared:
    driver: local
EOF
    say "Wrote ${CONFIG_DIR}/docker-compose.yml"
fi

# ── 9. Start Gitea ────────────────────────────────────────────────────────────

say "Starting Gitea..."
docker compose -f "${CONFIG_DIR}/docker-compose.yml" up -d gitea

# Wait for Gitea to respond on the host
say "Waiting for Gitea to be ready..."
GITEA_URL="${GITEA_URL:-http://localhost:3000}"
i=0
while [ "$i" -lt 12 ]; do
    if ${DOWNLOAD} "${GITEA_URL}/" > /dev/null 2>&1; then
        say "Gitea is ready."
        break
    fi
    i=$((i + 1))
    [ "$i" -lt 12 ] || die "Gitea did not become ready after 60s"
    sleep 5
done

# ── 10. Run gitea-init.sh ─────────────────────────────────────────────────────

say "Initialising Gitea..."

# Download gitea-init.sh and mcp-config.json into temp dir
${DOWNLOAD} "${RAW_BASE}/scripts/gitea-init.sh" > "${TMP_DIR}/gitea-init.sh"
chmod +x "${TMP_DIR}/gitea-init.sh"
${DOWNLOAD} "${RAW_BASE}/foundry/foundry-runner/mcp-config.json" > "${TMP_DIR}/mcp-config.json"

# Run init; capture output to extract token if newly created
INIT_OUTPUT="$(
    GITEA_CONTAINER="${GITEA_CONTAINER:-gitea}" \
    GITEA_URL="${GITEA_URL}" \
    FOUNDRY_ADMIN_USERNAME="${FOUNDRY_ADMIN_USERNAME}" \
    FOUNDRY_ADMIN_PASSWORD="${FOUNDRY_ADMIN_PASSWORD}" \
    FOUNDRY_ADMIN_EMAIL="${FOUNDRY_ADMIN_EMAIL}" \
    FOUNDRY_WEBHOOK_URL="${FOUNDRY_WEBHOOK_URL}" \
    FOUNDRY_WEBHOOK_SECRET="${FOUNDRY_WEBHOOK_SECRET}" \
    FOUNDRY_BOT_USERNAME="${FOUNDRY_BOT_USERNAME:-foundry-bot}" \
    FOUNDRY_BOT_EMAIL="${FOUNDRY_BOT_EMAIL:-foundry-bot@gitea.local}" \
    MCP_CONFIG_PATH="${TMP_DIR}/mcp-config.json" \
    bash "${TMP_DIR}/gitea-init.sh" 2>&1
)"
say "${INIT_OUTPUT}"

# Extract token if one was newly generated and save to config.env
# gitea-init.sh prints: │  FOUNDRY_GITEA_TOKEN=<token padded>│
NEW_TOKEN="$(printf '%s\n' "${INIT_OUTPUT}" \
    | grep 'FOUNDRY_GITEA_TOKEN=' \
    | sed 's/.*FOUNDRY_GITEA_TOKEN=\([A-Za-z0-9_-]*\).*/\1/')"
if [ -n "${NEW_TOKEN}" ]; then
    if check sed; then
        sed -i.bak "s/^FOUNDRY_GITEA_TOKEN=.*/FOUNDRY_GITEA_TOKEN=${NEW_TOKEN}/" \
            "${CONFIG_DIR}/config.env" && rm -f "${CONFIG_DIR}/config.env.bak"
    fi
    say "Bot token saved to ${CONFIG_DIR}/config.env"
fi

# ── 11. Generate foundry.toml ─────────────────────────────────────────────────

if [ ! -f "${CONFIG_DIR}/foundry.toml" ]; then
    RUNNER_VERSION="${VERSION#v}"
    cat > "${CONFIG_DIR}/foundry.toml" <<EOF
[server]
listen_addr = "0.0.0.0:8477"
webhook_secret = "\${FOUNDRY_WEBHOOK_SECRET}"

[gitea]
url = "${GITEA_URL:-http://localhost:3000}"
url_from_runner = "http://172.17.0.1:3000"
api_token = "\${FOUNDRY_GITEA_TOKEN}"
bot_username = "${FOUNDRY_BOT_USERNAME:-foundry-bot}"
bot_display_name = "Foundry Bot"
bot_email = "${FOUNDRY_BOT_EMAIL:-foundry-bot@gitea.local}"

[polling]
enabled = true
interval_secs = 120

[container]
image = "ghcr.io/mbr4477/foundry-runner:${RUNNER_VERSION}"
runtime = "docker"
network = "foundry-net"
memory_limit_mb = 2048
cpu_limit = 2.0
max_concurrent = 3
timeout_secs = 600

[volumes]
issue_prefix = "foundry-issue"
shared_volume = "foundry-shared"
home_volume = "foundry-home"

[commands]
approve = "/approve"

[logging]
level = "info"
format = "json"
EOF
    say "Generated ${CONFIG_DIR}/foundry.toml"
fi

# ── 12. Pull foundry-runner container ─────────────────────────────────────────

RUNNER_IMAGE="ghcr.io/mbr4477/foundry-runner:${VERSION#v}"
say "Pulling container image ${RUNNER_IMAGE}..."
docker pull "${RUNNER_IMAGE}"

# ── 13. Install service ───────────────────────────────────────────────────────

say "Installing foundryd as a background service..."

mkdir -p "${CONFIG_DIR}/logs"

FOUNDRYD_BIN="${INSTALL_DIR}/${BINARY}"
FOUNDRYD_TOML="${CONFIG_DIR}/foundry.toml"

if [ "${OS}" = "Darwin" ]; then
    # ── macOS: LaunchAgent ────────────────────────────────────────────────────

    PLIST_DIR="${HOME}/Library/LaunchAgents"
    PLIST_FILE="${PLIST_DIR}/dev.mruss.foundryd.plist"
    mkdir -p "${PLIST_DIR}"

    # Re-source config.env to get current secret values
    . "${CONFIG_DIR}/config.env"

    cat > "${PLIST_FILE}" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>dev.mruss.foundryd</string>

    <key>ProgramArguments</key>
    <array>
        <string>${FOUNDRYD_BIN}</string>
        <string>--config</string>
        <string>${FOUNDRYD_TOML}</string>
    </array>

    <key>EnvironmentVariables</key>
    <dict>
        <key>FOUNDRY_WEBHOOK_SECRET</key>
        <string>${FOUNDRY_WEBHOOK_SECRET}</string>
        <key>FOUNDRY_GITEA_TOKEN</key>
        <string>${FOUNDRY_GITEA_TOKEN}</string>
    </dict>

    <key>RunAtLoad</key>
    <true/>

    <key>KeepAlive</key>
    <true/>

    <key>StandardOutPath</key>
    <string>${CONFIG_DIR}/logs/foundryd.log</string>

    <key>StandardErrorPath</key>
    <string>${CONFIG_DIR}/logs/foundryd.err.log</string>
</dict>
</plist>
EOF

    # Unload any existing instance, then load the (new) plist
    launchctl unload "${PLIST_FILE}" 2>/dev/null || true
    launchctl load -w "${PLIST_FILE}"
    say "Service installed: ${PLIST_FILE}"
    say "Logs: ${CONFIG_DIR}/logs/"

else
    # ── Linux: systemd user service ───────────────────────────────────────────

    SYSTEMD_DIR="${HOME}/.config/systemd/user"
    UNIT_FILE="${SYSTEMD_DIR}/foundryd.service"
    mkdir -p "${SYSTEMD_DIR}"

    cat > "${UNIT_FILE}" <<EOF
[Unit]
Description=Foundry Daemon
After=network.target

[Service]
Type=simple
ExecStart=${FOUNDRYD_BIN} --config ${FOUNDRYD_TOML}
EnvironmentFile=${CONFIG_DIR}/config.env
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
EOF

    systemctl --user daemon-reload
    systemctl --user enable foundryd
    systemctl --user restart foundryd
    say "Service installed: ${UNIT_FILE}"

    # Advisory: linger lets the service start at boot without a login session
    if check loginctl; then
        LINGER="$(loginctl show-user "$(id -un)" --property=Linger --value 2>/dev/null || true)"
        if [ "${LINGER}" != "yes" ]; then
            say ""
            say "Note: to start foundryd at boot without logging in, run:"
            say "  loginctl enable-linger $(id -un)"
        fi
    fi
fi

# ── 14. Done ──────────────────────────────────────────────────────────────────

say ""
say "Foundry ${VERSION} installed successfully."
say "  Binary:    ${INSTALL_DIR}/${BINARY}"
say "  Image:     ${RUNNER_IMAGE}"
say "  Config:    ${CONFIG_DIR}/"
say ""
if [ "${OS}" = "Darwin" ]; then
    say "Service status:  launchctl list dev.mruss.foundryd"
    say "Logs:            tail -f ${CONFIG_DIR}/logs/foundryd.log"
else
    say "Service status:  systemctl --user status foundryd"
    say "Logs:            journalctl --user -u foundryd -f"
fi
say ""
say "Get started: https://github.com/${REPO}#getting-started"
