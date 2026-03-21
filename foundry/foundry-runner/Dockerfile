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
