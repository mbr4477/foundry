# Foundry

Interact with Claude Code via a local or self-hosted Gitea instance.

## Installation

Requires Docker, curl, and sh.

### Linux / macOS (recommended)

```shell
curl -fsSL https://raw.githubusercontent.com/mbr4477/foundry/main/scripts/install.sh | sh
```

**Options:**

```shell
# Pin to a specific release version
FOUNDRYD_VERSION=v0.2.0 curl -fsSL https://raw.githubusercontent.com/mbr4477/foundry/refs/heads/main/scripts/install.sh | sh

# Install to a custom directory (no sudo required)
INSTALL_DIR=~/.local/bin curl -fsSL https://raw.githubusercontent.com/mbr4477/foundry/refs/heads/main/scripts/install.sh | sh

# Pull install scripts from a specific branch
FOUNDRY_REF=my-branch curl -fsSL https://raw.githubusercontent.com/mbr4477/foundry/refs/heads/my-branch/scripts/install.sh | sh
```

### From Source


```shell
git clone https://github.com/mbr4477/foundry.git
cd foundry/foundry
cargo build --release -p foundryd
```

## Getting Started

1. Start the Docker Compose configuration:
   ```shell
   docker compose up -d
   ```
2. Run the setup script to create users, create webhooks, and setup volumes:
   ```shell
   ./scripts/gitea-init.sh \
       --gitea-container gitea \
       --gitea-url http://localhost:3000 \
       --admin-username gitea-admin \
       --admin-password password \
       --admin-email gitea.admin@gitea.local \
       --webhook-url http://host.docker.internal:8477/webhook \
       --webhook-secret 123
   ```
3. Configure environment variables:
   ```shell
   export FOUNDRY_GITEA_TOKEN=<your agent token>
   export FOUNDRY_WEBHOOK_SECRET=<your webhook secret>
   # If using a Claude API account
   export ANTHROPIC_AUTH_TOKEN=<your auth token>
   # If using a custom Claude endpoint
   export ANTHROPIC_BASE_URL=<your base url>
   ```
4. Start the foundryd daemon:
   ```shell
   ./foundryd foundry.toml
   ```
5. **IMPORTANT:** If using a Claude Pro/Max subscription, run the foundry-runner container to log in (will be persisted):
   ```shell
   docker run --rm -it --mount type=volume,src=foundry-home,dst=/home/foundry --entrypoint bash foundry-runner:latest
   ```
   > The `src` volume name MUST match the `home_volume` entry in the foundry.toml file.
   
   Inside the container prompt:
   ```shell
   claude
   # Follow the login prompts
   exit
   ```

## TODO

- How to use a customized Claude settings.json file
- How to install plugins
