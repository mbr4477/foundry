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
        owner: &str,
        repo: &str,
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

`volume_name` is not stored — derived deterministically as `foundry-issue-{owner}__{repo}__{N}` (double underscore separator, since Gitea usernames and repo names cannot contain `__`).

### Crash Recovery

The session store is a cache, not the source of truth. On startup (or if the DB is cleared), `foundryd` scans Gitea for all issues assigned to the bot and reconstructs state:

| Gitea state | Inferred phase |
|---|---|
| Assigned, no `/approve` comment, no branch | Planning |
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
    /// Note: no body field — Claude reads review content via list_pr_reviews MCP tool.
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
    PollRecovery {
        repo: RepoId,
        issue_number: u64,
        timestamp: DateTime<Utc>,
    },
}

pub enum ReviewState { Approved, ChangesRequested, Comment }
```

### Event Deduplication

Webhooks and polling can produce duplicate events. The dispatcher deduplicates using a time-windowed set (5-minute window):
- Webhook events: keyed by `delivery_id` (Gitea sets `X-Gitea-Delivery` on every webhook POST)
- `PollRecovery` events: keyed by `(repo, issue_number, timestamp)` truncated to the polling interval

The polling `since` anchor is a global high-water mark: the timestamp of the most recently processed event across all issues, persisted in the session store. On first run (or after DB loss), polling starts from 24 hours ago.

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
3. Writes directive: *"Your plan is approved. Create branch `foundry/issue-{N}`, implement the changes, write tests, open a PR, post a comment on the issue linking to it. Before exiting, write your result to `/foundry/result.json`."*
4. Spawns container → Claude clones repo, creates branch, implements, pushes, opens PR via MCP, writes `result.json`, exits
5. Dispatcher: reads `result.json` from volume, saves `session.pr_number` and `session.branch_name`, transitions to `InReview`

`result.json` structure:
```json
{ "pr_number": 7, "branch_name": "foundry/issue-42" }
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
- `foundry-mcp-gitea` binary at `/usr/local/bin/`
- `git`, `jq`, standard build tools
- Non-root user `foundry` (UID 1000) — Claude Code refuses `--dangerously-skip-permissions` when running as root. The container runs as `foundry` by default (`USER foundry` in the Dockerfile). All relevant paths (`/foundry/`, `/etc/foundry/`, `HOME`) must be readable by this user.

### Mounts

| Volume | Target | Mode | Contents |
|---|---|---|---|
| `foundry-issue-{owner}-{repo}-{N}` | `/foundry/` | rw | `instruction.json` (written by dispatcher before launch); `result.json` (written by Claude before exit) |
| `foundry-shared` | `/etc/foundry/` | ro | `mcp-config.json` |

The repo is cloned fresh each turn inside the container's ephemeral writable filesystem (`/workspace/`). Claude Code's own working files (`~/.claude/`, git clones, build artifacts) all live on the ephemeral filesystem and are discarded on exit. No session history is persisted — Gitea is the source of truth.

### Environment Variables

```
ANTHROPIC_API_KEY
GITEA_URL
GITEA_TOKEN
GITEA_BOT_USERNAME
GIT_AUTHOR_NAME       # e.g. "Foundry Bot"
GIT_AUTHOR_EMAIL      # e.g. "foundry-bot@gitea.local"
GIT_COMMITTER_NAME    # same as GIT_AUTHOR_NAME
GIT_COMMITTER_EMAIL   # same as GIT_AUTHOR_EMAIL
```

`GIT_AUTHOR_*` and `GIT_COMMITTER_*` are standard Git environment variables. Setting them ensures every `git commit` inside the container works without requiring a `~/.gitconfig`. The values are driven by `foundry.toml` (under `[gitea]`) and passed by the dispatcher at container launch.

`ANTHROPIC_API_KEY` is passed as an environment variable and is visible via `docker inspect`. For v1 this is acceptable on a trusted local network. Future improvement: mount it as a Docker secret file and have the entrypoint read it from `/run/secrets/anthropic_api_key`.

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

### `result.json` Structure

Written by Claude to `/foundry/result.json` before exiting on turns that produce a PR:

```json
{ "pr_number": 7, "branch_name": "foundry/issue-42" }
```

The dispatcher reads this file after the container exits to update session state. If the file is absent (e.g., Claude did not open a PR this turn), the dispatcher treats `pr_number` and `branch_name` as unchanged.

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

[session]
backend = "sqlite"       # "sqlite" or "memory"
db_path = "/var/lib/foundry/state.db"

[commands]
approve = "/approve"

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
3. **Generate bot API token** — scopes: `read:issue`, `write:issue`, `read:repository`, `write:repository`, `read:user`. Print once. Skip if token named `foundry` already exists. This token is used by the bot for API calls and git push over HTTP — it does not require admin scope. The `--admin-token` flag is a separate credential used only during setup.
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
