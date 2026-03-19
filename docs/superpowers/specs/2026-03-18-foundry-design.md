# Foundry — Design Spec

**Date:** 2026-03-18
**Status:** Approved

## Overview

Foundry is a system that lets Claude Code act as a junior developer on a self-hosted Gitea instance. When an issue is assigned to the bot user, Foundry orchestrates a workflow: Claude discusses requirements with the reporter, receives approval, implements the changes on a branch, opens a PR, and responds to review feedback. A human always merges.

---

## Components

Four distinct processes:

### `foundryd` — Orchestrator Daemon (Rust)

Receives Gitea events (via webhook and polling), manages per-issue session state, assembles structured directives, and spawns ephemeral Docker containers. Contains no AI or Gitea write logic — those live in the container.

Does NOT: call the Gitea API for issue operations, merge PRs, hold long-lived containers, or make workflow decisions beyond phase routing.

### `foundry-mcp-gitea` — Gitea MCP Server (Rust)

A standalone MCP server (stdio JSON-RPC) that runs inside each container. Wraps the Gitea REST API into typed tools for Claude Code. Stateless — no persistence between container invocations.

Does NOT: handle events, manage containers, or make decisions.

### `foundry-setup` — Initialization CLI (Rust)

One-shot, idempotent CLI that initializes Gitea: creates the bot user, generates an API token, registers a system-level webhook, and creates a Docker network. Safe to re-run.

### Ephemeral Container — Claude Code Runtime

Runs Claude Code with `foundry-mcp-gitea` configured as an MCP server. Launched per turn, exits when Claude finishes. All state lives in Gitea and the `foundryd` session store — the container filesystem is discarded on exit.

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

Implementations: `SqliteSessionStore` (WAL mode), `MemorySessionStore` (tests).

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
        key: &IssueKey,
        pr_number: u64,
    ) -> Result<Vec<ForgeReview>, CodeHostError>;
}
```

Used by `foundryd` only for polling/recovery. All writes go through the MCP server inside the container. Implementations: `GiteaCodeHost`, `GitHubCodeHost` (future).

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
    pub branch_name: Option<String>,  // set once Claude creates the branch
    pub pr_number: Option<u64>,       // set once Claude opens the PR
    pub last_event_at: DateTime<Utc>,
    pub container_running: bool,
}

pub enum IssuePhase {
    Planning,      // assigned, Claude is asking questions
    Implementing,  // /approve received, Claude is writing code
    InReview,      // PR open, Claude is responding to reviews
    Done,          // PR merged or issue closed
}
```

`volume_name` is not stored — derived deterministically as `foundry-issue-{owner}-{repo}-{N}`.

### Crash Recovery

The session store is a cache, not the source of truth. On startup (or if the DB is cleared), `foundryd` scans Gitea for all issues assigned to the bot and reconstructs state:

| Gitea state | Inferred phase |
|---|---|
| Assigned, no `/approve` comment, no branch | Planning |
| `/approve` comment exists, no PR | Implementing |
| Open PR exists | InReview |
| PR merged or issue closed | Done |

`container_running` is always reset to `false` on startup.

---

## Event Types

```rust
pub enum Event {
    IssueAssigned {
        repo: RepoId,
        issue_number: u64,
        assigner: String,
        timestamp: DateTime<Utc>,
    },
    IssueClosed {
        repo: RepoId,
        issue_number: u64,
        timestamp: DateTime<Utc>,
    },
    IssueCommentCreated {
        repo: RepoId,
        issue_number: u64,
        comment_id: u64,
        author: String,
        body: String,
        timestamp: DateTime<Utc>,
    },
    PrReviewSubmitted {
        repo: RepoId,
        pr_number: u64,
        reviewer: String,
        state: ReviewState,
        timestamp: DateTime<Utc>,
    },
    PrMerged {
        repo: RepoId,
        pr_number: u64,
        timestamp: DateTime<Utc>,
    },
    PrClosed {
        repo: RepoId,
        pr_number: u64,
        timestamp: DateTime<Utc>,
    },
    /// Synthetic event from polling — activity that may have been missed
    PollRecovery {
        repo: RepoId,
        issue_number: u64,
        timestamp: DateTime<Utc>,
    },
}

pub enum ReviewState { Approved, ChangesRequested, Comment }
```

---

## Data Flow

### A) New issue assigned

1. Gitea fires `issues` webhook (action: `assigned`) → `WebhookSource` emits `Event::IssueAssigned`
2. Dispatcher: `session_store.get()` → nothing found
3. Dispatcher: `session_store.upsert()` with phase `Planning`
4. Dispatcher: `container_runtime.ensure_volume()` for per-issue volume
5. Dispatcher: writes `instruction.json` to volume — directive: *"Read the issue, ask clarifying questions, propose a plan. Do not write code."*
6. Dispatcher: `container_runtime.run_container()` — blocks until exit
7. Container: Claude reads directive, calls `get_issue` + `list_issue_comments`, posts comment, exits
8. Dispatcher: removes container, sets `container_running = false`

### B) Reporter replies

1. `Event::IssueCommentCreated` — author is not the bot, body has no `/approve`
2. Dispatcher: loads session (phase `Planning`), `container_running` is false
3. Writes directive: *"The reporter has replied. Read the full thread, continue discussion or refine your plan."*
4. Spawns container → Claude reads thread, responds → exits

### C) Reporter comments `/approve`

1. `Event::IssueCommentCreated` — body contains `/approve`, author is not the bot
2. Dispatcher: transitions session to `Implementing`, calls `session_store.upsert()`
3. Writes directive: *"Your plan is approved. Create branch `foundry/issue-{N}`, implement the changes, write tests, open a PR, post a comment on the issue linking to it."*
4. Spawns container → Claude clones repo, creates branch, implements, pushes, opens PR via MCP, exits
5. Dispatcher: polls for new PR on branch, saves `session.pr_number`, transitions to `InReview`

### D) Reviewer posts review comments

1. `Event::PrReviewSubmitted` → `session_store.get_by_pr()` resolves to issue session
2. Phase is `InReview`
3. Writes directive: *"A reviewer has submitted feedback. Read all review comments, fix the code or reply explaining your reasoning. Add new commits — do not force-push."*
4. Spawns container → Claude reads reviews via MCP, pushes fixes, replies inline → exits

### Concurrency guard

If a new event arrives for an issue that already has `container_running = true`, the event is held in a bounded in-memory queue per issue key. When the container exits, the dispatcher drains the queue and spawns one new container with a consolidated directive.

### Event deduplication

Webhooks and polling can produce duplicate events. The dispatcher deduplicates by `(repo, issue_number, event_id, timestamp)` using a time-windowed set (5-minute window).

---

## Container Design

### Image (`foundry-runner`)

- Node.js + Claude Code CLI (`@anthropic-ai/claude-code`)
- `foundry-mcp-gitea` binary at `/usr/local/bin/`
- `git`, `jq`, standard build tools

### Mounts

| Volume | Target | Mode | Contents |
|---|---|---|---|
| `foundry-issue-{owner}-{repo}-{N}` | `/foundry/` | ro | `instruction.json` |
| `foundry-shared` | `/etc/foundry/` | ro | `mcp-config.json` |

The repo is cloned fresh each turn inside the container's ephemeral filesystem. No session history is persisted — Gitea is the source of truth.

### Environment Variables

```
ANTHROPIC_API_KEY
GITEA_URL
GITEA_TOKEN
GITEA_BOT_USERNAME
```

### Entrypoint

```bash
#!/bin/bash
set -euo pipefail
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
  "branch_name": null,
  "pr_number": null,
  "directive": "You are a software developer assigned to issue #42 in alice/myproject...\n\n[full structured prompt]"
}
```

The directive is a fully-formed prompt assembled by the dispatcher. It includes all context (issue title, repo, phase, relevant history summary) so Claude can act without any initial orientation tool calls.

### MCP Config (`/etc/foundry/mcp-config.json`)

```json
{
  "mcpServers": {
    "gitea": {
      "command": "/usr/local/bin/foundry-mcp-gitea",
      "args": [],
      "env": {}
    }
  }
}
```

`foundry-mcp-gitea` inherits the container's environment for `GITEA_URL` and `GITEA_TOKEN`.

### Network Isolation

Containers are attached to `foundry-net` — a Docker network with access only to the Gitea instance and the Anthropic API endpoint.

---

## MCP Server Tools

### Issues
- `get_issue(owner, repo, issue_number)` — full issue body and metadata
- `list_issue_comments(owner, repo, issue_number, since?)` — chronological
- `create_issue_comment(owner, repo, issue_number, body)` — post a comment
- `edit_issue(owner, repo, issue_number, title?, body?, state?)` — update issue

### Pull Requests
- `create_pull_request(owner, repo, title, body, head, base)` — open a PR
- `update_pull_request(owner, repo, pr_number, title?, body?)` — edit PR
- `get_pull_request(owner, repo, pr_number)` — fetch PR metadata
- `list_pr_reviews(owner, repo, pr_number)` — all reviews
- `list_pr_review_comments(owner, repo, pr_number, review_id)` — inline comments
- `create_pr_comment(owner, repo, pr_number, body)` — general PR comment
- `reply_to_pr_comment(owner, repo, pr_number, comment_id, body)` — reply inline

### Repository
- `get_repo(owner, repo)` — default branch, description
- `get_authenticated_user()` — bot's own username

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
max_concurrent = 4
timeout_secs = 1800

[volumes]
issue_prefix = "foundry-issue"
shared_volume = "foundry-shared"

[session]
backend = "sqlite"       # "sqlite" or "memory"
db_path = "/var/lib/foundry/state.db"

[commands]
approve = "/approve"
pause = "/pause"
resume = "/resume"

[logging]
level = "info"
format = "json"          # "json" or "pretty"
```

Secrets are never written to the TOML directly — always interpolated from environment variables.

---

## Setup Script (`foundry-setup`)

Idempotent. Safe to re-run.

```bash
foundry-setup \
  --config foundry.toml \
  --admin-token gta_admin_...
```

**Steps:**

1. **Verify connectivity** — confirm Gitea is reachable and admin token is valid
2. **Create bot user** — skip if already exists; `must_change_password: false`
3. **Generate bot API token** — scopes: `read:issue`, `write:issue`, `read:repository`, `write:repository`, `read:user`. Print once. Skip if token named `foundry` already exists.
4. **Create system-level webhook** — `POST /api/v1/admin/hooks`. Single hook covers all repos. Events: `issues`, `issue_comment`, `pull_request`, `pull_request_review`. Update if already registered.
5. **Print summary** — what was created, what was skipped, token (last 4 chars only)

Bot collaborator access on individual repos is managed manually by admins — not by `foundry-setup`.

---

## Graceful Shutdown

On SIGTERM:
1. Stop accepting new webhooks
2. Wait for running containers to finish (up to `timeout_secs`)
3. Persist all session state
4. Exit
