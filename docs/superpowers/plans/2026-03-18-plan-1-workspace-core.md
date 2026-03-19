# Workspace + foundry-core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Initialize the Cargo workspace and `foundry-core` library crate containing all shared types, traits, and error types used by every other crate.

**Architecture:** A single `foundry-core` library crate defines the domain types (`RepoId`, `IssueKey`, `IssueSession`, `IssuePhase`, `ReviewState`, `Event`) and the four swappable traits (`EventSource`, `ContainerRuntime`, `SessionStore`, `CodeHost`) with their associated error types. No implementations live here — only interfaces. All other crates depend on this crate.

**Tech Stack:** Rust stable, `async-trait`, `thiserror`, `serde`, `chrono`, `tokio` (for channel types in traits)

---

### Task 1: Initialize the Cargo workspace

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `.gitignore`

- [ ] **Step 1: Create the workspace `Cargo.toml`**

```toml
# Cargo.toml
[workspace]
members = [
    "foundry-core",
    "foundry-mcp-gitea",
    "foundryd",
    "foundry-setup",
]
resolver = "2"

[workspace.dependencies]
# Async runtime
tokio = { version = "1", features = ["full"] }
tokio-util = { version = "0.7", features = ["rt"] }
async-trait = "0.1"

# Error handling
thiserror = "2"
anyhow = "1"

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Time
chrono = { version = "0.4", features = ["serde"] }

# HTTP
reqwest = { version = "0.12", features = ["json"] }

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }

# CLI
clap = { version = "4", features = ["derive"] }

# Config
toml = "0.8"
serde-env = "0.1"

# Database
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "chrono", "migrate"] }

# Web server (webhook listener)
axum = { version = "0.7", features = ["json"] }
tower = "0.4"
hmac = "0.12"
sha2 = "0.10"
hex = "0.4"

# Docker
bollard = "0.17"

# MCP
rmcp = { version = "0.1", features = ["server", "transport-io"] }

# Testing
mockito = { version = "1", features = [] }

[workspace.package]
version = "0.1.0"
edition = "2021"
authors = ["Foundry Bot <foundry@local>"]
```

- [ ] **Step 2: Create `.gitignore`**

```
/target/
**/*.rs.bk
Cargo.lock
.env
*.db
*.db-shm
*.db-wal
```

- [ ] **Step 3: Verify workspace parses (no member crates yet — expected error)**

```bash
cd /Users/matthew/claudejr/foundry
cargo metadata --no-deps 2>&1 | head -5
```

Expected: error about missing member directories (that's fine, we haven't created them yet).

- [ ] **Step 4: Commit**

```bash
cd /Users/matthew/claudejr/foundry
git add Cargo.toml .gitignore
git commit -m "chore: initialize Cargo workspace"
```

---

### Task 2: Create the `foundry-core` crate skeleton

**Files:**
- Create: `foundry-core/Cargo.toml`
- Create: `foundry-core/src/lib.rs`

- [ ] **Step 1: Create `foundry-core/Cargo.toml`**

```toml
[package]
name = "foundry-core"
version.workspace = true
edition.workspace = true

[dependencies]
async-trait.workspace = true
thiserror.workspace = true
serde = { workspace = true }
serde_json.workspace = true
chrono.workspace = true
tokio = { workspace = true }
tokio-util.workspace = true
tracing.workspace = true
```

- [ ] **Step 2: Create `foundry-core/src/lib.rs`**

```rust
pub mod errors;
pub mod events;
pub mod traits;
pub mod types;
```

- [ ] **Step 3: Create stub files so the crate compiles**

Create `foundry-core/src/types.rs`, `foundry-core/src/events.rs`, `foundry-core/src/errors.rs` each containing just a `// TODO` comment, and `foundry-core/src/traits/mod.rs` with the same.

```bash
mkdir -p /Users/matthew/claudejr/foundry/foundry-core/src/traits
echo "// TODO" > /Users/matthew/claudejr/foundry/foundry-core/src/types.rs
echo "// TODO" > /Users/matthew/claudejr/foundry/foundry-core/src/events.rs
echo "// TODO" > /Users/matthew/claudejr/foundry/foundry-core/src/errors.rs
echo "// TODO" > /Users/matthew/claudejr/foundry/foundry-core/src/traits/mod.rs
```

- [ ] **Step 4: Verify crate compiles**

```bash
cd /Users/matthew/claudejr/foundry
cargo build -p foundry-core
```

Expected: `Compiling foundry-core v0.1.0` with no errors.

- [ ] **Step 5: Commit**

```bash
git add foundry-core/
git commit -m "chore(core): add foundry-core crate skeleton"
```

---

### Task 3: Implement domain types

**Files:**
- Modify: `foundry-core/src/types.rs`

- [ ] **Step 1: Write the test first**

```rust
// foundry-core/src/types.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_key_equality() {
        let a = IssueKey { owner: "alice".into(), repo: "myproject".into(), issue_number: 42 };
        let b = IssueKey { owner: "alice".into(), repo: "myproject".into(), issue_number: 42 };
        assert_eq!(a, b);
    }

    #[test]
    fn volume_name_uses_double_underscore_separator() {
        let key = IssueKey { owner: "alice".into(), repo: "my-project".into(), issue_number: 1 };
        let name = key.volume_name("foundry-issue");
        assert_eq!(name, "foundry-issue__alice__my-project__1");
        // Verify no ambiguity: owner and repo separated from each other and number
        assert!(!name.contains("alice-my")); // would be ambiguous with single hyphen
    }

    #[test]
    fn issue_phase_display() {
        assert_eq!(IssuePhase::Planning.to_string(), "planning");
        assert_eq!(IssuePhase::Implementing.to_string(), "implementing");
        assert_eq!(IssuePhase::InReview.to_string(), "in-review");
        assert_eq!(IssuePhase::Done.to_string(), "done");
    }

    #[test]
    fn issue_session_volume_name_delegates_to_key() {
        let session = IssueSession {
            key: IssueKey { owner: "bob".into(), repo: "repo".into(), issue_number: 7 },
            phase: IssuePhase::Planning,
            branch_name: None,
            pr_number: None,
            last_event_at: chrono::Utc::now(),
            container_running: false,
        };
        assert_eq!(session.volume_name("foundry-issue"), "foundry-issue__bob__repo__7");
    }
}
```

- [ ] **Step 2: Run the tests to confirm they fail**

```bash
cd /Users/matthew/claudejr/foundry
cargo test -p foundry-core 2>&1 | tail -10
```

Expected: compile errors (types not defined yet).

- [ ] **Step 3: Implement the types**

```rust
// foundry-core/src/types.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoId {
    pub owner: String,
    pub repo: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IssueKey {
    pub owner: String,
    pub repo: String,
    pub issue_number: u64,
}

impl IssueKey {
    /// Derives the Docker volume name for this issue.
    /// Uses `__` as separator since Gitea names cannot contain `__`.
    pub fn volume_name(&self, prefix: &str) -> String {
        format!("{}__{}__{}__{}", prefix, self.owner, self.repo, self.issue_number)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueSession {
    pub key: IssueKey,
    pub phase: IssuePhase,
    /// Set once Claude creates the branch (after /approve).
    pub branch_name: Option<String>,
    /// Set once Claude opens the PR (read from result.json).
    pub pr_number: Option<u64>,
    /// Timestamp of the most recently processed event for this issue.
    pub last_event_at: DateTime<Utc>,
    /// True while a container is actively running for this issue.
    pub container_running: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IssuePhase {
    /// Assigned; Claude is reading the issue and asking clarifying questions.
    Planning,
    /// /approve received; Claude is implementing on a branch.
    Implementing,
    /// PR is open; Claude is responding to review feedback.
    InReview,
    /// PR merged or issue closed; no further action.
    Done,
}

impl fmt::Display for IssuePhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IssuePhase::Planning => write!(f, "planning"),
            IssuePhase::Implementing => write!(f, "implementing"),
            IssuePhase::InReview => write!(f, "in-review"),
            IssuePhase::Done => write!(f, "done"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    Approved,
    ChangesRequested,
    Comment,
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p foundry-core types 2>&1
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add foundry-core/src/types.rs
git commit -m "feat(core): add domain types (IssueKey, IssueSession, IssuePhase, ReviewState)"
```

---

### Task 4: Implement the Event enum

**Files:**
- Modify: `foundry-core/src/events.rs`

- [ ] **Step 1: Write the test first**

```rust
// foundry-core/src/events.rs
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn event_serializes_to_json() {
        let event = Event::IssueAssigned {
            repo: RepoId { owner: "alice".into(), repo: "proj".into() },
            issue_number: 1,
            assigner: "bob".into(),
            delivery_id: "abc123".into(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("IssueAssigned"));
        assert!(json.contains("abc123"));
    }

    #[test]
    fn event_roundtrips_through_json() {
        let original = Event::IssueCommentCreated {
            repo: RepoId { owner: "alice".into(), repo: "proj".into() },
            issue_number: 5,
            comment_id: 99,
            author: "charlie".into(),
            body: "/approve".into(),
            delivery_id: "xyz".into(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let roundtripped: Event = serde_json::from_str(&json).unwrap();
        // Compare via json since DateTime precision may differ
        assert_eq!(
            serde_json::to_value(&original).unwrap(),
            serde_json::to_value(&roundtripped).unwrap()
        );
    }
}
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test -p foundry-core events 2>&1 | tail -5
```

Expected: compile error.

- [ ] **Step 3: Implement the Event enum**

```rust
// foundry-core/src/events.rs
use crate::types::{RepoId, ReviewState};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Event {
    IssueAssigned {
        repo: RepoId,
        issue_number: u64,
        assigner: String,
        /// Gitea webhook delivery ID (X-Gitea-Delivery header) for deduplication.
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
        /// Body included here as an optimization; Claude may also fetch via MCP.
        body: String,
        delivery_id: String,
        timestamp: DateTime<Utc>,
    },
    /// PR reviews may have many inline comments — Claude fetches them via MCP.
    /// No body field here by design.
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
    /// Synthetic event from the polling fallback — no delivery_id.
    /// Deduplicated by (repo, issue_number, timestamp window).
    PollRecovery {
        repo: RepoId,
        issue_number: u64,
        timestamp: DateTime<Utc>,
    },
}
```

- [ ] **Step 4: Add `use` imports to `events.rs` and re-export from `lib.rs`**

Add to `foundry-core/src/lib.rs`:
```rust
pub use events::Event;
pub use types::{IssueKey, IssuePhase, IssueSession, RepoId, ReviewState};
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p foundry-core events 2>&1
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add foundry-core/src/events.rs foundry-core/src/lib.rs
git commit -m "feat(core): add Event enum with serde support"
```

---

### Task 5: Implement error types

**Files:**
- Modify: `foundry-core/src/errors.rs`

- [ ] **Step 1: Write tests first**

```rust
// foundry-core/src/errors.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_source_error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<EventSourceError>();
    }

    #[test]
    fn container_error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ContainerError>();
    }

    #[test]
    fn session_store_error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SessionStoreError>();
    }

    #[test]
    fn code_host_error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CodeHostError>();
    }

    #[test]
    fn container_error_timeout_display() {
        let e = ContainerError::Timeout { container_id: "abc".into() };
        assert!(e.to_string().contains("abc"));
    }
}
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test -p foundry-core errors 2>&1 | tail -5
```

- [ ] **Step 3: Implement error types**

```rust
// foundry-core/src/errors.rs
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EventSourceError {
    #[error("Webhook signature verification failed")]
    InvalidSignature,
    #[error("Failed to parse webhook payload: {0}")]
    ParseError(String),
    #[error("HTTP server error: {0}")]
    Http(String),
    #[error("Channel closed — dispatcher shut down")]
    ChannelClosed,
}

#[derive(Debug, Error)]
pub enum ContainerError {
    #[error("Docker API error: {0}")]
    Api(String),
    #[error("Container {container_id} timed out")]
    Timeout { container_id: String },
    #[error("Failed to create volume '{name}': {reason}")]
    VolumeCreate { name: String, reason: String },
    #[error("Failed to write to volume '{volume}' at '{path}': {reason}")]
    VolumeWrite { volume: String, path: String, reason: String },
    #[error("Container exited with non-zero code {exit_code}")]
    NonZeroExit { exit_code: i64 },
}

#[derive(Debug, Error)]
pub enum SessionStoreError {
    #[error("Database error: {0}")]
    Database(String),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Migration error: {0}")]
    Migration(String),
}

#[derive(Debug, Error)]
pub enum CodeHostError {
    #[error("HTTP request failed: {0}")]
    Http(String),
    #[error("API rate limited; retry after {retry_after_secs:?} seconds")]
    RateLimited { retry_after_secs: Option<u64> },
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Unauthorized — check GITEA_TOKEN")]
    Unauthorized,
    #[error("Unexpected response: {0}")]
    UnexpectedResponse(String),
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p foundry-core errors 2>&1
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add foundry-core/src/errors.rs
git commit -m "feat(core): add typed error enums for all module boundaries"
```

---

### Task 6: Implement traits

**Files:**
- Create: `foundry-core/src/traits/event_source.rs`
- Create: `foundry-core/src/traits/container_runtime.rs`
- Create: `foundry-core/src/traits/session_store.rs`
- Create: `foundry-core/src/traits/code_host.rs`
- Modify: `foundry-core/src/traits/mod.rs`

- [ ] **Step 1: Write trait tests (object-safety and Send+Sync bounds)**

```rust
// foundry-core/src/traits/mod.rs
pub mod code_host;
pub mod container_runtime;
pub mod event_source;
pub mod session_store;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::*;

    // Verify traits are object-safe (can be used as Box<dyn Trait>)
    #[test]
    fn event_source_is_object_safe() {
        fn _assert(_: Box<dyn event_source::EventSource>) {}
    }

    #[test]
    fn container_runtime_is_object_safe() {
        fn _assert(_: Box<dyn container_runtime::ContainerRuntime>) {}
    }

    #[test]
    fn session_store_is_object_safe() {
        fn _assert(_: Box<dyn session_store::SessionStore>) {}
    }

    #[test]
    fn code_host_is_object_safe() {
        fn _assert(_: Box<dyn code_host::CodeHost>) {}
    }
}
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test -p foundry-core traits 2>&1 | tail -5
```

- [ ] **Step 3: Implement `event_source.rs`**

```rust
// foundry-core/src/traits/event_source.rs
use crate::errors::EventSourceError;
use crate::events::Event;
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[async_trait]
pub trait EventSource: Send + Sync + 'static {
    /// Start producing events into `tx` until `cancel` is triggered.
    async fn run(
        &self,
        tx: mpsc::Sender<Event>,
        cancel: CancellationToken,
    ) -> Result<(), EventSourceError>;
}
```

- [ ] **Step 4: Implement `container_runtime.rs`**

```rust
// foundry-core/src/traits/container_runtime.rs
use crate::errors::ContainerError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ContainerSpec {
    pub image: String,
    pub env: HashMap<String, String>,
    pub mounts: Vec<Mount>,
    pub network: Option<String>,
    pub memory_limit_bytes: Option<u64>,
    pub cpu_period: Option<u64>,
    pub cpu_quota: Option<i64>,
    /// Labels applied to the container (used for orphan detection on restart).
    pub labels: HashMap<String, String>,
    /// Kill the container after this many seconds (0 = no limit).
    pub timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct Mount {
    pub source: VolumeSource,
    pub target: PathBuf,
    pub read_only: bool,
}

#[derive(Debug, Clone)]
pub enum VolumeSource {
    /// Docker named volume
    Named(String),
    /// Bind-mount from host filesystem
    HostPath(PathBuf),
}

#[derive(Debug)]
pub struct ContainerResult {
    pub container_id: String,
    pub exit_code: i64,
}

#[async_trait]
pub trait ContainerRuntime: Send + Sync + 'static {
    /// Run a container to completion. Returns when the container exits.
    async fn run_container(&self, spec: ContainerSpec) -> Result<ContainerResult, ContainerError>;

    /// Ensure a named volume exists, creating it if absent.
    async fn ensure_volume(&self, name: &str) -> Result<(), ContainerError>;

    /// Remove a named volume.
    async fn remove_volume(&self, name: &str) -> Result<(), ContainerError>;

    /// Remove a stopped container by ID.
    async fn remove_container(&self, container_id: &str) -> Result<(), ContainerError>;

    /// Write bytes to a path inside a named volume.
    /// Implemented by running a short-lived helper container.
    async fn write_to_volume(
        &self,
        volume: &str,
        path: &str,
        contents: &[u8],
    ) -> Result<(), ContainerError>;

    /// Read bytes from a path inside a named volume.
    async fn read_from_volume(
        &self,
        volume: &str,
        path: &str,
    ) -> Result<Vec<u8>, ContainerError>;

    /// List running containers with a given label key=value.
    async fn list_running_with_label(
        &self,
        label_key: &str,
        label_value: &str,
    ) -> Result<Vec<String>, ContainerError>;

    /// Kill a running container.
    async fn kill_container(&self, container_id: &str) -> Result<(), ContainerError>;
}
```

- [ ] **Step 5: Implement `session_store.rs`**

```rust
// foundry-core/src/traits/session_store.rs
use crate::errors::SessionStoreError;
use crate::types::{IssueKey, IssueSession};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

#[async_trait]
pub trait SessionStore: Send + Sync + 'static {
    /// Create or update a session.
    async fn upsert(&self, session: &IssueSession) -> Result<(), SessionStoreError>;

    /// Read a session by its primary key.
    async fn get(&self, key: &IssueKey) -> Result<Option<IssueSession>, SessionStoreError>;

    /// Read a session by its associated PR number (secondary index).
    async fn get_by_pr(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<Option<IssueSession>, SessionStoreError>;

    /// List all active (non-deleted) sessions.
    async fn list(&self) -> Result<Vec<IssueSession>, SessionStoreError>;

    /// Soft-delete a session (exclude from list()).
    async fn delete(&self, key: &IssueKey) -> Result<(), SessionStoreError>;

    /// Get the global polling high-water mark (most recently processed event timestamp).
    async fn get_poll_watermark(&self) -> Result<Option<DateTime<Utc>>, SessionStoreError>;

    /// Set the global polling high-water mark.
    async fn set_poll_watermark(&self, ts: DateTime<Utc>) -> Result<(), SessionStoreError>;
}
```

- [ ] **Step 6: Implement `code_host.rs`**

```rust
// foundry-core/src/traits/code_host.rs
use crate::errors::CodeHostError;
use crate::types::IssueKey;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeIssue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub owner: String,
    pub repo: String,
    pub assignees: Vec<String>,
    pub state: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeComment {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeReview {
    pub id: u64,
    pub reviewer: String,
    pub state: String,
    pub body: String,
    pub submitted_at: DateTime<Utc>,
}

/// Minimal read-only interface used by `foundryd` for polling and crash recovery.
/// All writes (comments, PRs) go through `foundry-mcp-gitea` inside the container.
#[async_trait]
pub trait CodeHost: Send + Sync + 'static {
    /// List open issues assigned to the bot, updated since `since`.
    async fn list_assigned_issues(
        &self,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<ForgeIssue>, CodeHostError>;

    /// List comments on an issue, updated since `since`.
    async fn list_issue_comments(
        &self,
        key: &IssueKey,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<ForgeComment>, CodeHostError>;

    /// List reviews on a PR.
    async fn list_pr_reviews(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<Vec<ForgeReview>, CodeHostError>;

    /// Get a single issue by number.
    async fn get_issue(&self, key: &IssueKey) -> Result<ForgeIssue, CodeHostError>;

    /// Find an open PR with the given head branch.
    async fn find_pr_by_branch(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<Option<u64>, CodeHostError>;

    /// Check whether a branch exists in the repo.
    async fn branch_exists(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<bool, CodeHostError>;
}
```

- [ ] **Step 7: Run all tests**

```bash
cargo test -p foundry-core 2>&1
```

Expected: all tests pass, no warnings.

- [ ] **Step 8: Commit**

```bash
git add foundry-core/src/traits/
git commit -m "feat(core): add EventSource, ContainerRuntime, SessionStore, CodeHost traits"
```

---

### Task 7: Final verification

- [ ] **Step 1: Verify the full crate compiles cleanly with warnings-as-errors**

```bash
cd /Users/matthew/claudejr/foundry
RUSTFLAGS="-D warnings" cargo build -p foundry-core 2>&1
```

Expected: clean build, no warnings.

- [ ] **Step 2: Run all tests one final time**

```bash
cargo test -p foundry-core -- --nocapture 2>&1
```

Expected: all tests pass.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "chore(core): verify foundry-core compiles cleanly"
```
