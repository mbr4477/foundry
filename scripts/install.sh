#!/bin/sh
set -e

REPO="mbr4477/foundry"
BINARY="foundryd"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"

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
trap 'rm -f "${TMP_FILE}"' EXIT

DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${VERSION}/${ASSET}"

if check curl; then
    curl -fsSL "${DOWNLOAD_URL}" -o "${TMP_FILE}"
else
    wget -qO "${TMP_FILE}" "${DOWNLOAD_URL}"
fi

chmod +x "${TMP_FILE}"

# ── 5. Install binary ──────────────────────────────────────────────────────────

if [ -w "${INSTALL_DIR}" ]; then
    cp "${TMP_FILE}" "${INSTALL_DIR}/${BINARY}"
elif check sudo; then
    say "Installing to ${INSTALL_DIR} (requires sudo)..."
    sudo cp "${TMP_FILE}" "${INSTALL_DIR}/${BINARY}"
else
    # Fall back to ~/.local/bin
    INSTALL_DIR="${HOME}/.local/bin"
    mkdir -p "${INSTALL_DIR}"
    cp "${TMP_FILE}" "${INSTALL_DIR}/${BINARY}"
    say "Installed to ${INSTALL_DIR}/${BINARY}"
    say "Add ${INSTALL_DIR} to your PATH if it isn't already:"
    say "  export PATH=\"\$PATH:${INSTALL_DIR}\""
fi

say "Installed ${BINARY} to ${INSTALL_DIR}/${BINARY}"

# ── 6. Pull foundry-runner container ──────────────────────────────────────────

RUNNER_IMAGE="ghcr.io/mbr4477/foundry-runner:${VERSION}"
say "Pulling container image ${RUNNER_IMAGE}..."
docker pull "${RUNNER_IMAGE}"

# ── 7. Done ────────────────────────────────────────────────────────────────────

say ""
say "Foundry ${VERSION} installed successfully."
say "  Binary:  ${INSTALL_DIR}/${BINARY}"
say "  Image:   ${RUNNER_IMAGE}"
say ""
say "Get started: https://github.com/${REPO}#getting-started"
