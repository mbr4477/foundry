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
# Releases are tarballs on gitea.com; asset names use x86_64/arm64 conventions.
ARG GITEA_MCP_VERSION=1.0.1
RUN set -eux; \
    ARCH="$(uname -m)"; \
    case "$ARCH" in \
        x86_64)  GITEA_ARCH="x86_64" ;; \
        aarch64) GITEA_ARCH="arm64"  ;; \
        *) echo "Unsupported arch: $ARCH" && exit 1 ;; \
    esac; \
    curl -fsSL \
        "https://gitea.com/gitea/gitea-mcp/releases/download/v${GITEA_MCP_VERSION}/gitea-mcp_Linux_${GITEA_ARCH}.tar.gz" \
        -o /tmp/gitea-mcp.tar.gz \
    && tar -xzf /tmp/gitea-mcp.tar.gz -C /usr/local/bin/ gitea-mcp \
    && chmod +x /usr/local/bin/gitea-mcp \
    && rm /tmp/gitea-mcp.tar.gz

# Create non-root user — Claude Code refuses --dangerously-skip-permissions as root
# node:22-slim already has a 'node' user at UID/GID 1000; rename it to 'foundry'.
RUN groupmod -n foundry node && \
    usermod -l foundry -d /home/foundry -m node && \
    usermod -s /bin/bash foundry

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
