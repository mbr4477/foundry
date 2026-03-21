# Foundry — Design Spec

**Date:** 2026-03-18
**Status:** Approved

## Overview

Foundry is a system that lets Claude Code act as a junior developer on a self-hosted Gitea instance. When an issue is assigned to the bot user, Foundry orchestrates a workflow: Claude discusses requirements with the reporter, receives approval, implements the changes on a branch, opens a PR, and responds to review feedback. A human always merges.

---

## Components

Four distinct processes:

### `foundryd` — Orchestrator Daemon (Rust)

Receives Gitea events (via webhook and polling), manages in-memory per-issue session state, assembles structured directives, and spawns ephemeral Docker containers. Contains no AI or Gitea write logic — those live in the container.

Does NOT: call the Gitea API for issue operations, merge PRs, hold long-lived containers, or make workflow decisions beyond phase routing.

### `gitea-mcp` — Gitea MCP Server (official binary)

The official MCP server from the Gitea project, run as a stdio subprocess inside each container. Wraps the Gitea REST API into tools for Claude Code. Stateless — no persistence between container invocations.

Claude Code's allow-list permission rules restrict it to exactly the tools Foundry requires; all other gitea-mcp tools (merge, delete, admin, etc.) are blocked at the permissions layer.

Does NOT: handle events, manage containers, or make decisions.

> **Note:** Plan 2 (`2026-03-18-plan-2-mcp-gitea.md`), which described building a custom Rust MCP server, is superseded by this decision. Do not execute it.

### `scripts/gitea-init.sh` — Gitea Initialization Script (bash)

One-shot, idempotent bash script that initializes a fresh Gitea instance for Foundry: creates the Gitea admin user, creates the bot user, generates a bot API token, and registers a system-level webhook. Safe to re-run.

> **Note:** Plan 4 (`2026-03-18-plan-4-setup.md`), which described building a Rust binary for this purpose, is superseded by this decision. Do not execute it.

### Ephemeral Container — Claude Code Runtime

Runs Claude Code with `gitea-mcp` configured as an MCP server. Launched per turn, exits when Claude finishes. All persistent state lives in Gitea — the container filesystem and `foundryd`'s in-memory session store are both ephemeral.

---

## Module Interfaces

Each swappable module is defined by a Rust trait with typed errors via `thiserror`. Implementations are injected at startup; tests use in-memory fakes.

### `EventSource`

```rust
#[async_trait]
pub trait EventSource: Send + Sync + 'static {
    async fn run(
        &self,
        tx: mpsc::Sender<Event>,
        cancel: CancellationToken,
    ) -> Result<(), EventSourceError>;
}
```

Implementations: `WebhookSource` (HTTP server), `PollingSource` (Gitea API scan).

### `ContainerRuntime`

```rust
#[async_trait]
pub trait ContainerRuntime: Send + Sync + 'static {
    async fn run_container(&self, spec: &ContainerSpec) -> Result<ContainerResult, ContainerError>;
    async fn ensure_volume(&self, name: &str) -> Result<(), ContainerError>;
    async fn remove_container(&self, container_id: &str) -> Result<(), ContainerError>;
    async fn write_to_volume(&self, volume: &str, path: &str, contents: &[u8]) -> Result<(), ContainerError>;
}
```

Implementations: `DockerRuntime`, `PodmanRuntime` (future), `MockRuntime` (tests).

### `SessionStore`

```rust
#[async_trait]
pub trait SessionStore: Send + Sync + 'static {
    async fn upsert(&self, session: &IssueSession) -> Result<(), SessionStoreError>;
    async fn get(&self, key: &IssueKey) -> Result<Option<IssueSession>, SessionStoreError>;
    async fn get_by_pr(&self, owner: &str, repo: &str, pr_number: u64) -> Result<Option<IssueSession>, SessionStoreError>;
    async fn list(&self) -> Result<Vec<IssueSession>, SessionStoreError>;
    async fn delete(&self, key: &IssueKey) -> Result<(), SessionStoreError>;
}
```

Implementation: `MemorySessionStore` (in-memory `HashMap`, used in both production and tests). State is always reconstructed from Gitea on startup.

### `CodeHost`

```rust
#[async_trait]
pub trait CodeHost: Send + Sync + 'static {
    async fn list_assigned_issues(
        &self,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<ForgeIssue>, CodeHostError>;

    async fn list_issue_comments(
        &self,
        key: &IssueKey,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<ForgeComment>, CodeHostError>;

    async fn list_pr_reviews(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<Vec<ForgeReview>, CodeHostError>;
}
```

Used by `foundryd` only for polling/recovery. All writes go through the MCP server inside the container. Implementations: `GiteaCodeHost`, `GitHubCodeHost` (future).

### `CodeHost` Return Types

```rust
pub struct ForgeIssue {
    pub key: IssueKey,
    pub pr_number: Option<u64>,  // set if an open PR exists for this issue
}

pub struct ForgeComment {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

pub struct ForgeReview {
    pub id: u64,
    pub reviewer: String,
    pub state: ReviewState,
    pub submitted_at: DateTime<Utc>,
}
```

`ForgeIssue.pr_number` is populated from Gitea's issue API, which includes a linked PR reference. During startup reconstruction, this is how `foundryd` recovers `session.pr_number` for `InReview` sessions — no separate PR lookup is needed.

`list_pr_reviews` is used during startup reconstruction only: after identifying an `InReview` session, `foundryd` calls it to check whether any unaddressed reviews exist, and if so spawns a container immediately to handle them rather than waiting for the next event.

---

## Session State

```rust
pub struct IssueKey {
    pub owner: String,
    pub repo: String,
    pub issue_number: u64,
}

pub struct IssueSession {
    pub key: IssueKey,
    pub phase: IssuePhase,
    pub pr_number: Option<u64>,       // set once Claude opens the PR
    pub container_running: bool,
}

pub enum IssuePhase {
    Planning,      // assigned, Claude is asking questions
    Implementing,  // /approve received, Claude is writing code
    InReview,      // PR open, Claude is responding to reviews
    Done,          // PR merged or issue closed
}
```

`volume_name` is not stored — derived deterministically as `{issue_prefix}__{owner}__{repo}__{N}`, where `issue_prefix` comes from `[volumes].issue_prefix` in `foundry.toml` (default `foundry-issue`). Double underscores are used as separators throughout, since Gitea usernames and repo names cannot contain `__`.

### Startup Reconstruction

The session store is in-memory only. On every startup, `foundryd` scans Gitea for all issues assigned to the bot and reconstructs state:

| Gitea state | Inferred phase |
|---|---|
| Assigned, no `/approve` comment | Planning |
| `/approve` comment exists, no PR | Implementing |
| Open PR exists | InReview |
| PR merged or issue closed | Done |

`container_running` is always reset to `false` on startup. On startup, `foundryd` also lists all running containers with a `foundry.issue` label and kills any orphans before processing events — this handles the case where `foundryd` crashed while a container was still running.

---

## Event Types

```rust
pub struct RepoId {
    pub owner: String,
    pub repo: String,
}

pub enum Event {
    IssueAssigned {
        repo: RepoId,
        issue_number: u64,
        assigner: String,
        /// Gitea webhook delivery ID — used for deduplication
        delivery_id: String,
        timestamp: DateTime<Utc>,
    },
    IssueClosed {
        repo: RepoId,
        issue_number: u64,
        delivery_id: String,
        timestamp: DateTime<Utc>,
    },
    IssueCommentCreated {
        repo: RepoId,
        issue_number: u64,
        comment_id: u64,
        author: String,
        body: String,
        delivery_id: String,
        timestamp: DateTime<Utc>,
    },
    /// Note: no body field — Claude reads review content via list_pull_request_reviews MCP tool.
    /// IssueCommentCreated carries body as an optimization (single comment, cheap to include);
    /// PR reviews may have many inline comments so Claude always fetches them via MCP.
    PrReviewSubmitted {
        repo: RepoId,
        pr_number: u64,
        reviewer: String,
        state: ReviewState,
        delivery_id: String,
        timestamp: DateTime<Utc>,
    },
    PrMerged {
        repo: RepoId,
        pr_number: u64,
        delivery_id: String,
        timestamp: DateTime<Utc>,
    },
    PrClosed {
        repo: RepoId,
        pr_number: u64,
        delivery_id: String,
        timestamp: DateTime<Utc>,
    },
    /// Synthetic event from polling — activity that may have been missed.
    /// Has no delivery_id; deduplicated by (repo, issue_number, timestamp window).
    /// For InReview sessions, the dispatcher uses session.pr_number (already in memory)
    /// to spawn a container that checks for and responds to any new review activity.
    PollRecovery {
        repo: RepoId,
        issue_number: u64,
        /// Wall clock time when the polling source generated this event,
        /// truncated to the polling interval. Used as the deduplication key
        /// so back-to-back polling cycles within the same interval produce
        /// the same key and are dropped as duplicates.
        timestamp: DateTime<Utc>,
    },
}

pub enum ReviewState { Approved, ChangesRequested, Comment }
```

### Event Deduplication

Webhooks and polling can produce duplicate events. The dispatcher deduplicates using a time-windowed set (5-minute window):
- Webhook events: keyed by `delivery_id` (Gitea sets `X-Gitea-Delivery` on every webhook POST)
- `PollRecovery` events: keyed by `(repo, issue_number, timestamp)` truncated to the polling interval

The polling `since` anchor is a global high-water mark: the timestamp of the most recently processed event across all issues, held in memory and updated by the dispatcher after each event is processed. On startup, polling starts from 24 hours ago. The deduplication window handles any events re-observed after a restart.

### Known Limitations (v1)

- Standalone PR inline comments not submitted as part of a formal review (`pull_request_comment` webhook event) are not handled in real-time — they will be picked up on the next polling cycle. This is acceptable for v1.

---

## Data Flow

### A) New issue assigned

1. Gitea fires `issues` webhook (action: `assigned`) → `WebhookSource` emits `Event::IssueAssigned`
2. Dispatcher: `session_store.get()` → nothing found
3. Dispatcher: `session_store.upsert()` with phase `Planning`
4. Dispatcher: `container_runtime.ensure_volume()` for per-issue volume
5. Dispatcher: writes `instruction.json` to volume — directive: *"Read the issue, ask clarifying questions, propose a plan. Do not write code."*
6. Dispatcher: `container_runtime.run_container()` — blocks until exit
7. Container: Claude reads directive, calls `get_issue_by_index` + `get_issue_comments_by_index`, posts comment, exits
8. Dispatcher: removes container, sets `container_running = false`

### B) Reporter replies

1. `Event::IssueCommentCreated` — author is not the bot, body has no `/approve`
2. Dispatcher: loads session (phase `Planning`), `container_running` is false
3. Writes directive: *"The reporter has replied. Read the full thread, continue discussion or refine your plan."*
4. Spawns container → Claude reads thread, responds → exits

### C) Reporter comments `/approve`

1. `Event::IssueCommentCreated` — body contains `/approve`, author is not the bot
2. Dispatcher: transitions session to `Implementing`, calls `session_store.upsert()`
3. Writes directive: *"Your plan is approved. Create branch `foundry/issue-{N}`, implement the changes, write tests, open a PR, post a comment on the issue linking to it. Before exiting, write your result to `/foundry/result.json`."*
4. Spawns container → Claude clones repo, creates branch, implements, pushes, opens PR via MCP, writes `result.json`, exits
5. Dispatcher: reads `result.json` from volume, saves `session.pr_number`, transitions to `InReview`

`result.json` structure:
```json
{ "pr_number": 7 }
```

This eliminates the race condition between PR creation and incoming review events — `pr_number` is captured synchronously before the dispatcher processes any further events.

### D) Reviewer posts review comments

1. `Event::PrReviewSubmitted` → `session_store.get_by_pr()` resolves to issue session
2. Phase is `InReview`
3. Writes directive: *"A reviewer has submitted feedback. Read all review comments, fix the code or reply explaining your reasoning. Add new commits — do not force-push."*
4. Spawns container → Claude reads reviews via MCP, pushes fixes, replies inline → exits

### E) PR merged (by human)

1. `Event::PrMerged` → `session_store.get_by_pr()` resolves to issue session
2. Dispatcher: transitions session to `Done`, calls `session_store.delete()`
3. Dispatcher: removes the per-issue Docker volume (no further turns needed)
4. No container is spawned

### F) PR closed without merge

1. `Event::PrClosed` → `session_store.get_by_pr()` resolves to issue session
2. Dispatcher: transitions session to `Done`, calls `session_store.delete()`
3. Dispatcher: removes the per-issue Docker volume
4. No container is spawned — the human chose to close the PR; no automated action is taken

### G) Issue closed externally

1. `Event::IssueClosed` → `session_store.get()` resolves to issue session
2. If a PR is still open (`session.pr_number` is set), no action is taken — let flow E or F handle cleanup when the PR resolves
3. If no PR exists, dispatcher transitions to `Done` and calls `session_store.delete()`
4. No container is spawned

### Concurrency guard

If a new event arrives for an issue that already has `container_running = true`, the event is held in a bounded in-memory queue per issue key. When the container exits, the dispatcher drains the queue and spawns one new container with a consolidated directive covering all pending events (e.g., *"The reporter posted 2 new comments since your last turn — read the full thread and continue"*). The consolidated directive does not embed comment bodies; Claude reads the full thread via MCP tools.

### Container timeout and non-zero exit

If a container exceeds `timeout_secs`, or exits with a non-zero exit code, the dispatcher kills it (if still running), sets `container_running = false`, and posts a comment on the issue: *"I ran into a problem on my last turn. Please let me know if you'd like me to try again."* Any queued events for the issue are then processed normally. No automatic retry — a human decides whether to re-trigger.

---

## Container Design

### Image (`foundry-runner`)

- Node.js + Claude Code CLI (`@anthropic-ai/claude-code`)
- `gitea-mcp` binary at `/usr/local/bin/` (official binary from the Gitea project)
- `git`, `jq`, standard build tools
- Non-root user `foundry` (UID 1000) — Claude Code refuses `--dangerously-skip-permissions` when running as root. The container runs as `foundry` by default (`USER foundry` in the Dockerfile). All relevant paths (`/foundry/`, `/etc/foundry/`, `HOME`) must be readable by this user.

### Mounts

| Volume | Target | Mode | Contents |
|---|---|---|---|
| `foundry-issue__{owner}__{repo}__{N}` | `/foundry/` | rw | `instruction.json` (written by dispatcher before launch); `result.json` (written by Claude before exit) |
| `foundry-shared` | `/etc/foundry/` | ro | `mcp-config.json` |

The repo is cloned fresh each turn inside the container's ephemeral writable filesystem (`/workspace/`). Claude Code's own working files (`~/.claude/`, git clones, build artifacts) all live on the ephemeral filesystem and are discarded on exit. No session history is persisted — Gitea is the source of truth.

### Environment Variables

```
ANTHROPIC_API_KEY
GITEA_HOST            # gitea-mcp's name for the Gitea instance URL
GITEA_ACCESS_TOKEN    # gitea-mcp's name for the API token; also used by GIT_ASKPASS for HTTP git auth
GITEA_BOT_USERNAME    # used by GIT_ASKPASS for HTTP git auth and available to Claude via the directive
GIT_AUTHOR_NAME       # e.g. "Foundry Bot"
GIT_AUTHOR_EMAIL      # e.g. "foundry-bot@gitea.local"
GIT_COMMITTER_NAME    # same as GIT_AUTHOR_NAME
GIT_COMMITTER_EMAIL   # same as GIT_AUTHOR_EMAIL
```

`GIT_ASKPASS` is set internally by the entrypoint script (not an external input). It points to a temporary script that returns `GITEA_BOT_USERNAME` for username prompts and `GITEA_ACCESS_TOKEN` for password prompts, enabling `git clone` and `git push` over HTTPS without embedding credentials in remote URLs.

`GITEA_HOST` and `GITEA_ACCESS_TOKEN` use gitea-mcp's expected env var names. `foundryd` passes them directly — no remapping needed. `GIT_AUTHOR_*` and `GIT_COMMITTER_*` are standard Git environment variables ensuring every `git commit` works without `~/.gitconfig`. All values come from `foundry.toml` (under `[gitea]`).

`ANTHROPIC_API_KEY` is passed as an environment variable and is visible via `docker inspect`. For v1 this is acceptable on a trusted local network. Future improvement: mount it as a Docker secret file and have the entrypoint read it from `/run/secrets/anthropic_api_key`.

### Entrypoint

```bash
#!/bin/bash
set -euo pipefail

# Configure git HTTP authentication using GIT_ASKPASS.
# Git calls this script when it needs credentials; the token never appears in
# remote URLs, git logs, or process lists.
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

claude --dangerously-skip-permissions \
    --mcp-config /etc/foundry/mcp-config.json \
    -p "$(jq -r '.directive' /foundry/instruction.json)"
```

### `instruction.json` Structure

```json
{
  "phase": "planning",
  "repo": { "owner": "alice", "repo": "myproject" },
  "issue_number": 42,
  "pr_number": null,
  "directive": "You are a software developer assigned to issue #42 in alice/myproject...\n\n[full structured prompt]"
}
```

The directive is a fully-formed prompt assembled by the dispatcher. It includes all context (issue title, repo, phase, relevant history summary) so Claude can act without any initial orientation tool calls.

`pr_number` is injected so Claude knows the associated PR during review turns without having to search for it. It is `null` during `Planning` and `Implementing` phases.

### `result.json` Structure

Written by Claude to `/foundry/result.json` before exiting on turns that produce a PR:

```json
{ "pr_number": 7 }
```

The dispatcher reads this file after the container exits to update session state. If the file is absent (e.g., Claude did not open a PR this turn), the dispatcher treats `pr_number` as unchanged.

### MCP Config (`/etc/foundry/mcp-config.json`)

```json
{
  "mcpServers": {
    "gitea": {
      "command": "/usr/local/bin/gitea-mcp",
      "args": ["-t", "stdio"]
    }
  },
  "permissions": {
    "deny": ["mcp__gitea__*"],
    "allow": [
      "mcp__gitea__get_issue_by_index",
      "mcp__gitea__get_issue_comments_by_index",
      "mcp__gitea__create_issue_comment",
      "mcp__gitea__edit_issue",
      "mcp__gitea__create_pull_request",
      "mcp__gitea__get_pull_request_by_index",
      "mcp__gitea__get_pull_request_diff",
      "mcp__gitea__list_pull_request_reviews",
      "mcp__gitea__list_pull_request_review_comments",
      "mcp__gitea__create_pull_request_review",
      "mcp__gitea__submit_pull_request_review",
      "mcp__gitea__get_my_user_info"
    ]
  }
}
```

`gitea-mcp` inherits `GITEA_HOST` and `GITEA_ACCESS_TOKEN` from the container environment — no remapping needed since those are the names `foundryd` passes directly.

**Permissions model note:** In Claude Code, a specific `allow` entry takes precedence over a broader `deny` glob. The effect of `deny: ["mcp__gitea__*"]` plus individual `allow` entries is an allow-list: only the listed tools are callable; everything else (including `merge_pull_request`, `delete_repository`, etc.) is blocked at the permissions layer.

### Network Isolation

Containers are attached to `foundry-net` — a Docker network with access only to the Gitea instance and the Anthropic API endpoint.

---

## MCP Server Tools

These are the gitea-mcp tool names Claude Code is permitted to call (via the allow-list in `mcp-config.json`). All other gitea-mcp tools are blocked.

### Issues
- `get_issue_by_index` — full issue body and metadata
- `get_issue_comments_by_index` — chronological comments
- `create_issue_comment` — post a comment on an issue or PR (PRs are issues in Gitea)
- `edit_issue` — update issue title, body, or state

### Pull Requests
- `create_pull_request` — open a PR
- `get_pull_request_by_index` — fetch PR metadata
- `get_pull_request_diff` — view the diff
- `list_pull_request_reviews` — all submitted reviews
- `list_pull_request_review_comments` — inline comments on a review
- `create_pull_request_review` — draft a review with inline comments
- `submit_pull_request_review` — submit a drafted review

### User
- `get_my_user_info` — bot's own username

Note: the Gitea API does not support replying to individual review comment threads. Claude responds to reviewer feedback by submitting a new review or posting a general comment via `create_issue_comment`.

Claude uses the `git` CLI directly (via its built-in Bash tool) for all Git operations. The MCP server only wraps the Gitea REST API.

---

## Configuration

```toml
[server]
listen_addr = "0.0.0.0:8477"
webhook_secret = "${FOUNDRY_WEBHOOK_SECRET}"

[gitea]
url = "https://gitea.local"
api_token = "${FOUNDRY_GITEA_TOKEN}"
bot_username = "foundry-bot"
bot_display_name = "Foundry Bot"
bot_email = "foundry-bot@gitea.local"
# Optional allowlist — if omitted, process all repos the bot is assigned issues in
# repos = ["alice/myproject"]

[polling]
enabled = true
interval_secs = 120

[container]
image = "foundry-runner:latest"
runtime = "docker"       # "docker" or "podman"
network = "foundry-net"
memory_limit_mb = 4096
cpu_limit = 2.0
max_concurrent = 4          # global limit across all issues
timeout_secs = 1800

[volumes]
issue_prefix = "foundry-issue"
shared_volume = "foundry-shared"

[commands]
approve = "/approve"

[logging]
level = "info"
format = "json"          # "json" or "pretty"
```

Secrets are never written to the TOML directly — always interpolated from environment variables.

---

## Gitea Initialization Script (`scripts/gitea-init.sh`)

Idempotent. Safe to re-run. Requires `curl`, `jq`, and `docker` on the operator's machine.

```bash
./scripts/gitea-init.sh \
  --gitea-container gitea \
  --gitea-url https://gitea.local \
  --admin-username gitea-admin \
  --admin-password <password> \
  --admin-email admin@gitea.local \
  --webhook-url http://foundry:8477/webhook \
  --webhook-secret <secret> \
  [--bot-username foundry-bot] \
  [--bot-email foundry-bot@gitea.local]
```

All flags may also be supplied via environment variables (`GITEA_URL`, `GITEA_CONTAINER`, `FOUNDRY_ADMIN_USERNAME`, `FOUNDRY_ADMIN_PASSWORD`, `FOUNDRY_ADMIN_EMAIL`, `FOUNDRY_WEBHOOK_URL`, `FOUNDRY_WEBHOOK_SECRET`, `FOUNDRY_BOT_USERNAME`, `FOUNDRY_BOT_EMAIL`). Flags take precedence over env vars. Note: `GITEA_URL` here is the same Gitea instance URL passed as `GITEA_HOST` in the container runtime environment — the names differ because the script and the container runtime are separate consumers.

**Steps:**

1. **Create Gitea admin user** — `docker exec $GITEA_CONTAINER gitea admin user create --admin`; skip if already exists
2. **Create temporary admin token** — single basic auth call to `POST /api/v1/users/{admin}/tokens`, named `foundry-setup-tmp`; all subsequent API calls use this token (Bearer auth)
3. **Verify connectivity** — `GET /api/v1/version`; confirm Gitea is reachable and token is valid
4. **Create bot user** — `POST /api/v1/admin/users`; skip if already exists; `must_change_password: false`
5. **Create bot API token** — `POST /api/v1/users/{bot}/tokens`, named `foundry`, with `"scope": ["repository"]` in the request body (required for HTTP git clone/push; the bot user must also have Collaborator access to each repo — see the closing note below); skip if token named `foundry` already exists; print full token once prominently to stdout. This token is used by `foundryd` and the container for Gitea API calls and git operations over HTTPS. If the token already exists, the secret value cannot be recovered via the API — the operator must delete and recreate it manually in Gitea if the original value was lost.
6. **Register system webhook** — `POST /api/v1/admin/hooks`. Events: `issues`, `issue_comment`, `pull_request`, `pull_request_review`. If a webhook with the same URL already exists, update it; otherwise create it.
7. **Delete temporary admin token** — `DELETE /api/v1/users/{admin}/tokens/{id}`; cleanup so no long-lived admin token persists
8. **Print summary** — what was created, what was skipped, token (last 4 chars only if already existed, full value if newly created)

Docker network and volume creation are not part of this script — those are handled by `docker-compose.yml`.

Bot collaborator access on individual repos is managed manually by admins — not by `gitea-init.sh`.

---

## Graceful Shutdown

On SIGTERM:
1. Stop accepting new webhooks
2. Wait for running containers to finish (up to `container.timeout_secs` — reusing the same per-container limit as the drain window)
3. Exit
