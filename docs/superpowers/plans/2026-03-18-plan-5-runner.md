# foundry-runner Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `foundry-runner` Docker image — the container that runs Claude Code with `foundry-mcp-gitea` configured as an MCP server for each turn.

**Architecture:** A multi-stage Dockerfile: stage 1 builds `foundry-mcp-gitea` in Rust, stage 2 is the runtime image with Node.js, Claude Code CLI, the MCP binary, and a non-root `foundry` user. An `entrypoint.sh` reads `instruction.json` from the mounted volume and passes the directive to `claude -p`.

**Tech Stack:** Docker, Node.js (Claude Code), Rust (multi-stage build), bash

---

### Task 1: Create the Dockerfile

**Files:**
- Create: `foundry-runner/Dockerfile`
- Create: `.dockerignore`

- [ ] **Step 1: Create `foundry-runner/Dockerfile`**

```dockerfile
# foundry-runner/Dockerfile

FROM node:22-slim AS runtime

# Install system dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    git \
    jq \
    curl \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Install Claude Code CLI globally
RUN npm install -g @anthropic-ai/claude-code

# Download the official gitea-mcp binary
ARG GITEA_MCP_VERSION=0.3.0
RUN curl -fsSL \
    "https://dl.gitea.com/gitea-mcp/${GITEA_MCP_VERSION}/gitea-mcp-linux-amd64" \
    -o /usr/local/bin/gitea-mcp \
    && chmod +x /usr/local/bin/gitea-mcp

# Create non-root user — Claude Code refuses --dangerously-skip-permissions as root
RUN groupadd -r -g 1000 foundry && \
    useradd -r -u 1000 -g foundry -m -d /home/foundry -s /bin/bash foundry

# Configure git defaults for the foundry user
RUN su - foundry -c 'git config --global init.defaultBranch main' && \
    su - foundry -c 'git config --global advice.detachedHead false' && \
    su - foundry -c 'git config --global core.autocrlf false'

# Create workspace directory owned by foundry user
RUN mkdir -p /workspace && chown foundry:foundry /workspace

# Copy entrypoint
COPY foundry-runner/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

# Switch to non-root user
USER foundry
WORKDIR /workspace

# Volumes mounted at runtime:
#   /foundry/     — instruction.json (rw); result.json written here by Claude
#   /etc/foundry/ — mcp-config.json (ro)

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
```

- [ ] **Step 2: Create `.dockerignore`**

```
target/
.git/
*.md
docs/
.DS_Store
.env
```

- [ ] **Step 3: Commit**

```bash
git add foundry-runner/Dockerfile .dockerignore
git commit -m "feat(runner): add multi-stage Dockerfile with non-root foundry user"
```

---

### Task 2: Create the entrypoint script

**Files:**
- Create: `foundry-runner/entrypoint.sh`

- [ ] **Step 1: Create `foundry-runner/entrypoint.sh`**

```bash
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
```

- [ ] **Step 2: Commit**

```bash
git add foundry-runner/entrypoint.sh
git commit -m "feat(runner): add entrypoint.sh with validation and context logging"
```

---

### Task 3: Create the MCP config reference file

**Files:**
- Create: `foundry-runner/mcp-config.json`

- [ ] **Step 1: Create the reference config**

This file is placed on the `foundry-shared` Docker volume by the operator. Include it in the repo as a reference template.

```json
{
  "mcpServers": {
    "gitea": {
      "command": "/usr/local/bin/gitea-mcp",
      "args": ["-t", "stdio"],
      "env": {}
    }
  }
}
```

`gitea-mcp` inherits `GITEA_URL` and `GITEA_ACCESS_TOKEN` from the container environment — no need to specify them here.

- [ ] **Step 2: Commit**

```bash
git add foundry-runner/mcp-config.json
git commit -m "feat(runner): add reference mcp-config.json for foundry-shared volume"
```

---

### Task 4: Build the image and verify contents

- [ ] **Step 1: Build the image**

Run from the workspace root (Dockerfile uses the workspace as build context):

```bash
cd /Users/matthew/claudejr/foundry
docker build -t foundry-runner:latest -f foundry-runner/Dockerfile .
```

Expected: image builds successfully. The Rust compilation stage may take several minutes on first run.

- [ ] **Step 2: Verify the image runs as the `foundry` user**

```bash
docker run --rm --entrypoint whoami foundry-runner:latest
```

Expected output: `foundry`

- [ ] **Step 3: Verify required binaries are present**

```bash
docker run --rm --entrypoint which foundry-runner:latest gitea-mcp
```

Expected: `/usr/local/bin/gitea-mcp`

```bash
docker run --rm --entrypoint ls foundry-runner:latest -lh /usr/local/bin/gitea-mcp
```

Expected: binary exists and is executable.

```bash
docker run --rm --entrypoint which foundry-runner:latest claude
```

Expected: path to the Claude Code CLI.

```bash
docker run --rm --entrypoint git foundry-runner:latest --version
```

Expected: `git version 2.x.x`

```bash
docker run --rm --entrypoint jq foundry-runner:latest --version
```

Expected: `jq-1.x`

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore(runner): verify Docker image builds with all required tools"
```

---

### Task 5: Smoke test the entrypoint

- [ ] **Step 1: Test missing instruction.json is caught**

```bash
docker run --rm foundry-runner:latest 2>&1
echo "Exit code: $?"
```

Expected: error message about missing `/foundry/instruction.json`, exit code 1.

- [ ] **Step 2: Create test fixtures**

```bash
mkdir -p /tmp/foundry-test-vol /tmp/foundry-shared-vol

cat > /tmp/foundry-test-vol/instruction.json << 'EOF'
{
  "phase": "planning",
  "repo": {"owner": "test", "repo": "test"},
  "issue_number": 1,
  "branch_name": null,
  "pr_number": null,
  "directive": "Say hello and exit immediately."
}
EOF

cp /Users/matthew/claudejr/foundry/foundry-runner/mcp-config.json \
   /tmp/foundry-shared-vol/mcp-config.json
```

- [ ] **Step 3: Run the container with test fixtures**

Note: Claude Code will fail at the API call (no valid API key) but the entrypoint should log the phase/repo/issue header correctly before failing.

```bash
docker run --rm \
    -v /tmp/foundry-test-vol:/foundry:rw \
    -v /tmp/foundry-shared-vol:/etc/foundry:ro \
    -e ANTHROPIC_API_KEY=sk-ant-invalid \
    -e GITEA_HOST=http://localhost:3000 \
    -e GITEA_ACCESS_TOKEN=fake-token \
    -e GITEA_BOT_USERNAME=foundry-bot \
    -e GIT_AUTHOR_NAME="Foundry Bot" \
    -e GIT_AUTHOR_EMAIL="foundry-bot@local" \
    -e GIT_COMMITTER_NAME="Foundry Bot" \
    -e GIT_COMMITTER_EMAIL="foundry-bot@local" \
    foundry-runner:latest 2>&1 | head -20
```

Expected: the entrypoint prints the phase/repo/issue header to stderr, then Claude Code starts and fails with an auth error. The key check is that the header appears correctly.

- [ ] **Step 4: Clean up test fixtures**

```bash
rm -rf /tmp/foundry-test-vol /tmp/foundry-shared-vol
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore(runner): smoke test entrypoint with mock instruction.json"
```

---

### Task 6: Document image size and create the Docker network

- [ ] **Step 1: Check image size**

```bash
docker images foundry-runner:latest --format "table {{.Repository}}\t{{.Tag}}\t{{.Size}}"
```

Expected: image is under 2GB.

- [ ] **Step 2: Create the Docker network used by containers**

```bash
docker network create foundry-net 2>/dev/null || echo "Network already exists"
```

- [ ] **Step 3: Verify the network exists**

```bash
docker network inspect foundry-net --format "{{.Name}} ({{.Driver}})"
```

Expected: `foundry-net (bridge)`

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore(runner): document image size and create foundry-net Docker network"
```

---

### Task 7: Final workspace verification

- [ ] **Step 1: Build the full Rust workspace in release mode**

```bash
cd /Users/matthew/claudejr/foundry
cargo build --release --workspace 2>&1
```

Expected: all crates build cleanly.

- [ ] **Step 2: Run all tests**

```bash
cargo test --workspace 2>&1
```

Expected: all tests pass.

- [ ] **Step 3: Verify release binaries exist**

```bash
ls -lh target/release/foundryd
```

Expected: binary is present.

- [ ] **Step 4: Final commit**

```bash
git add -A
git commit -m "chore: final verification — all binaries built, all tests pass"
```

---

## End-to-end smoke test (optional, requires a running Gitea instance)

Once all 5 plans are complete, verify the full system manually:

1. Run `foundry-setup` against your Gitea instance
2. Copy the generated `FOUNDRY_GITEA_TOKEN` to your environment
3. Copy `foundry-runner/mcp-config.json` to a Docker volume named `foundry-shared`
4. Start `foundryd` with `foundry.toml` configured
5. Create an issue in Gitea and assign it to `foundry-bot`
6. Verify that `foundryd` spawns a container and Claude posts a comment on the issue
