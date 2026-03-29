#!/bin/sh
set -e

REPO="mbr4477/foundry"
BINARY="foundryd"
INSTALL_DIR="${INSTALL_DIR:-${HOME}/.local/bin}"
CONFIG_DIR="${HOME}/.config/foundry"
FOUNDRY_REF="${FOUNDRY_REF:-main}"
RAW_BASE="https://raw.githubusercontent.com/${REPO}/refs/heads/${FOUNDRY_REF}"

# Helpers

say()  { printf '%s\n' "$*"; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }
check() { command -v "$1" >/dev/null 2>&1; }

# Check prerequisites
say "Checking prerequisites..."

## Downloader
if check curl; then
    DOWNLOAD="curl -fsSL"
elif check wget; then
    DOWNLOAD="wget -qO-"
else
    die "curl or wget is required but neither was found"
fi

## Docker
if ! check docker; then
    die "Docker is required but was not found. Install Docker: https://docs.docker.com/get-docker/"
fi
if ! docker info >/dev/null 2>&1; then
    die "Docker is installed but the daemon is not running. Start Docker and retry."
fi

# Detect platform
OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}-${ARCH}" in
    Linux-x86_64)   ASSET="foundryd-linux-amd64" ;;
    Linux-arm64)    ASSET="foundryd-linux-arm64" ;;
    Linux-aarch64)  ASSET="foundryd-linux-arm64" ;;
    Darwin-arm64)   ASSET="foundryd-macos-arm64" ;;
    Darwin-aarch64) ASSET="foundryd-macos-arm64" ;;
    *) die "Unsupported platform: ${OS} ${ARCH}. Supported: Linux x86_64, Linux arm64, macOS arm64" ;;
esac

# Resolve version
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

# Download and install foundryd binary
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

mkdir -p "${INSTALL_DIR}"
cp "${TMP_FILE}" "${INSTALL_DIR}/${BINARY}"
say "Installed ${BINARY} to ${INSTALL_DIR}/${BINARY}"

# Warn if INSTALL_DIR is not on PATH
case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *) say "Note: add ${INSTALL_DIR} to your PATH if it isn't already:"
       say "  export PATH=\"\$PATH:${INSTALL_DIR}\"" ;;
esac

# Configure app.ini for Gitea
mkdir -p "${CONFIG_DIR}/gitea"
if [ ! -f "${CONFIG_DIR}/gitea/app.ini" ]; then
    cat > "${CONFIG_DIR}/gitea/app.ini" <<'EOF'
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
    say "Wrote ${CONFIG_DIR}/gitea/app.ini"
fi

# Configure docker-compose.yml for Gitea
if [ ! -f "${CONFIG_DIR}/gitea/docker-compose.yml" ]; then
    cat > "${CONFIG_DIR}/gitea/docker-compose.yml" <<EOF
# Foundry — managed by install.sh. Safe to edit.
services:
  gitea:
    image: gitea/gitea:latest
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
    say "Wrote ${CONFIG_DIR}/gitea/docker-compose.yml"
fi

# Start Gitea
say "Starting Gitea..."
docker compose -f "${CONFIG_DIR}/gitea/docker-compose.yml" up -d

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

# Configure Gitea
## TODO

# Configure foundry.toml
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

# Pull foundry-runner image
RUNNER_IMAGE="ghcr.io/mbr4477/foundry-runner:${VERSION#v}"
say "Pulling container image ${RUNNER_IMAGE}..."
docker pull "${RUNNER_IMAGE}"

# Done
echo "Foundry ${VERSION} installed successfully!"