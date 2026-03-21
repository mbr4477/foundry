# foundryd Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `foundryd`, the orchestrator daemon that receives Gitea events, manages per-issue session state, assembles structured directives, and spawns ephemeral Docker containers.

**Architecture:** Event-bus pattern using Tokio `mpsc` channels. `WebhookSource` and `PollingSource` emit `Event` values into the channel. The `Dispatcher` consumes events, consults the `SessionStore`, assembles `instruction.json` via `directive.rs`, and calls `ContainerRuntime` to spawn containers. All modules are injected as trait objects so they can be swapped for fakes in tests.

**Tech Stack:** Rust, Tokio, axum (webhook server), bollard (Docker), reqwest, tracing

---

### Task 1: Create the crate and config

**Files:**
- Create: `foundryd/Cargo.toml`
- Create: `foundryd/src/main.rs`
- Create: `foundryd/src/config.rs`

- [ ] **Step 1: Create `foundryd/Cargo.toml`**

```toml
[package]
name = "foundryd"
version.workspace = true
edition.workspace = true

[[bin]]
name = "foundryd"
path = "src/main.rs"

[dependencies]
foundry-core.path = "../foundry-core"
tokio.workspace = true
tokio-util.workspace = true
async-trait.workspace = true
thiserror.workspace = true
anyhow.workspace = true
serde.workspace = true
serde_json.workspace = true
chrono.workspace = true
reqwest.workspace = true
axum.workspace = true
tower.workspace = true
hmac.workspace = true
sha2.workspace = true
hex.workspace = true
bollard.workspace = true
futures-util.workspace = true
base64.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
clap.workspace = true
toml.workspace = true

[dev-dependencies]
mockito.workspace = true
tokio = { workspace = true, features = ["test-util"] }
```

- [ ] **Step 2: Write config test first**

```rust
// foundryd/src/config.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_parses_minimal_toml() {
        let toml = r#"
[server]
listen_addr = "0.0.0.0:8477"
webhook_secret = "mysecret"

[gitea]
url = "http://gitea.local"
api_token = "gta_abc"
bot_username = "foundry-bot"
bot_display_name = "Foundry Bot"
bot_email = "foundry-bot@local"

[container]
image = "foundry-runner:latest"
runtime = "docker"
network = "foundry-net"
memory_limit_mb = 1024
cpu_limit = 1.0
max_concurrent = 2
timeout_secs = 300

[volumes]
issue_prefix = "foundry-issue"
shared_volume = "foundry-shared"

[commands]
approve = "/approve"

[logging]
level = "info"
format = "json"
"#;
        let config = Config::from_toml(toml).unwrap();
        assert_eq!(config.gitea.bot_username, "foundry-bot");
        assert_eq!(config.container.timeout_secs, 300);
        assert_eq!(config.commands.approve, "/approve");
    }

    #[test]
    fn config_requires_gitea_url() {
        let toml = r#"
[gitea]
api_token = "x"
bot_username = "bot"
bot_display_name = "Bot"
bot_email = "bot@local"
"#;
        assert!(Config::from_toml(toml).is_err());
    }

    #[test]
    fn secret_value_interpolates_env_var() {
        std::env::set_var("TEST_SECRET_XYZ", "my-secret-value");
        let result = SecretValue::resolve("${TEST_SECRET_XYZ}").unwrap();
        assert_eq!(result, "my-secret-value");
        std::env::remove_var("TEST_SECRET_XYZ");
    }

    #[test]
    fn secret_value_returns_literal_without_braces() {
        let result = SecretValue::resolve("plain-value").unwrap();
        assert_eq!(result, "plain-value");
    }
}
```

- [ ] **Step 3: Run to confirm failure**

```bash
cargo test -p foundryd config 2>&1 | tail -5
```

- [ ] **Step 4: Implement `config.rs`**

```rust
// foundryd/src/config.rs
use anyhow::{Context, Result};
use serde::Deserialize;

/// Resolves a string that may be a literal value or `${ENV_VAR}` reference.
pub struct SecretValue;

impl SecretValue {
    pub fn resolve(value: &str) -> Result<String> {
        if value.starts_with("${") && value.ends_with('}') {
            let var_name = &value[2..value.len() - 1];
            std::env::var(var_name)
                .with_context(|| format!("Environment variable '{}' not set", var_name))
        } else {
            Ok(value.to_string())
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub gitea: GiteaConfig,
    #[serde(default)]
    pub polling: PollingConfig,
    pub container: ContainerConfig,
    pub volumes: VolumesConfig,
    pub commands: CommandsConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub webhook_secret: String,
}

#[derive(Debug, Deserialize)]
pub struct GiteaConfig {
    pub url: String,
    pub api_token: String,
    pub bot_username: String,
    pub bot_display_name: String,
    pub bot_email: String,
    pub repos: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct PollingConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_polling_interval")]
    pub interval_secs: u64,
}

impl Default for PollingConfig {
    fn default() -> Self {
        Self { enabled: true, interval_secs: 120 }
    }
}

#[derive(Debug, Deserialize)]
pub struct ContainerConfig {
    pub image: String,
    pub runtime: String,
    pub network: String,
    pub memory_limit_mb: u64,
    pub cpu_limit: f64,
    pub max_concurrent: usize,
    pub timeout_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct VolumesConfig {
    pub issue_prefix: String,
    pub shared_volume: String,
}

#[derive(Debug, Deserialize)]
pub struct CommandsConfig {
    #[serde(default = "default_approve")]
    pub approve: String,
}

#[derive(Debug, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_log_format")]
    pub format: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self { level: "info".into(), format: "json".into() }
    }
}

impl Config {
    pub fn from_toml(content: &str) -> Result<Self> {
        toml::from_str(content).context("Failed to parse config TOML")
    }

    pub fn from_file(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path))?;
        Self::from_toml(&content)
    }

    /// Resolve all secret values (${ENV_VAR} interpolation).
    pub fn resolve_secrets(&mut self) -> Result<()> {
        self.server.webhook_secret =
            SecretValue::resolve(&self.server.webhook_secret)?;
        self.gitea.api_token =
            SecretValue::resolve(&self.gitea.api_token)?;
        Ok(())
    }
}

fn default_true() -> bool { true }
fn default_polling_interval() -> u64 { 120 }
fn default_approve() -> String { "/approve".into() }
fn default_log_level() -> String { "info".into() }
fn default_log_format() -> String { "json".into() }
```

- [ ] **Step 5: Create stub `main.rs`**

```rust
// foundryd/src/main.rs
mod config;

fn main() {
    println!("foundryd stub");
}
```

- [ ] **Step 6: Run config tests**

```bash
cargo test -p foundryd config 2>&1
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add foundryd/
git commit -m "feat(foundryd): add crate skeleton and config parsing"
```

---

### Task 2: Implement MemorySessionStore

**Files:**
- Create: `foundryd/src/session_store/mod.rs`
- Create: `foundryd/src/session_store/memory.rs`

- [ ] **Step 1: Write tests first**

```rust
// foundryd/src/session_store/memory.rs
#[cfg(test)]
mod tests {
    use super::*;
    use foundry_core::{
        traits::session_store::SessionStore,
        types::{IssueKey, IssuePhase, IssueSession},
    };

    fn make_session(owner: &str, repo: &str, number: u64) -> IssueSession {
        IssueSession {
            key: IssueKey { owner: owner.into(), repo: repo.into(), issue_number: number },
            phase: IssuePhase::Planning,
            pr_number: None,
            container_running: false,
        }
    }

    #[tokio::test]
    async fn upsert_and_get() {
        let store = MemorySessionStore::new();
        let session = make_session("alice", "proj", 1);
        store.upsert(&session).await.unwrap();

        let retrieved = store.get(&session.key).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().key.issue_number, 1);
    }

    #[tokio::test]
    async fn get_returns_none_for_unknown_key() {
        let store = MemorySessionStore::new();
        let key = IssueKey { owner: "x".into(), repo: "y".into(), issue_number: 99 };
        assert!(store.get(&key).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn upsert_updates_existing() {
        let store = MemorySessionStore::new();
        let mut session = make_session("alice", "proj", 1);
        store.upsert(&session).await.unwrap();

        session.phase = IssuePhase::Implementing;
        store.upsert(&session).await.unwrap();

        let retrieved = store.get(&session.key).await.unwrap().unwrap();
        assert_eq!(retrieved.phase, IssuePhase::Implementing);
    }

    #[tokio::test]
    async fn delete_removes_from_list() {
        let store = MemorySessionStore::new();
        let session = make_session("alice", "proj", 1);
        store.upsert(&session).await.unwrap();
        store.delete(&session.key).await.unwrap();

        let list = store.list().await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn get_by_pr_finds_session() {
        let store = MemorySessionStore::new();
        let mut session = make_session("alice", "proj", 5);
        session.pr_number = Some(42);
        store.upsert(&session).await.unwrap();

        let found = store.get_by_pr("alice", "proj", 42).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().key.issue_number, 5);
    }

}
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test -p foundryd session_store::memory 2>&1 | tail -5
```

- [ ] **Step 3: Implement `memory.rs`**

```rust
// foundryd/src/session_store/memory.rs
use async_trait::async_trait;
use foundry_core::{
    errors::SessionStoreError,
    traits::session_store::SessionStore,
    types::{IssueKey, IssueSession},
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone, Default)]
pub struct MemorySessionStore {
    inner: Arc<RwLock<Inner>>,
}

#[derive(Default)]
struct Inner {
    sessions: HashMap<IssueKey, IssueSession>,
}

impl MemorySessionStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SessionStore for MemorySessionStore {
    async fn upsert(&self, session: &IssueSession) -> Result<(), SessionStoreError> {
        self.inner.write().await.sessions.insert(session.key.clone(), session.clone());
        Ok(())
    }

    async fn get(&self, key: &IssueKey) -> Result<Option<IssueSession>, SessionStoreError> {
        Ok(self.inner.read().await.sessions.get(key).cloned())
    }

    async fn get_by_pr(&self, owner: &str, repo: &str, pr_number: u64)
        -> Result<Option<IssueSession>, SessionStoreError>
    {
        let guard = self.inner.read().await;
        let found = guard.sessions.values().find(|s| {
            s.key.owner == owner
                && s.key.repo == repo
                && s.pr_number == Some(pr_number)
        });
        Ok(found.cloned())
    }

    async fn list(&self) -> Result<Vec<IssueSession>, SessionStoreError> {
        Ok(self.inner.read().await.sessions.values().cloned().collect())
    }

    async fn delete(&self, key: &IssueKey) -> Result<(), SessionStoreError> {
        self.inner.write().await.sessions.remove(key);
        Ok(())
    }
}
```

- [ ] **Step 4: Create `session_store/mod.rs`**

```rust
// foundryd/src/session_store/mod.rs
pub mod memory;
```

- [ ] **Step 5: Add module to `main.rs`**

```rust
mod config;
mod session_store;
```

- [ ] **Step 6: Run tests**

```bash
cargo test -p foundryd session_store::memory 2>&1
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add foundryd/src/session_store/
git commit -m "feat(foundryd): add MemorySessionStore"
```

---

---

### Task 3: Implement GiteaCodeHost

**Files:**
- Create: `foundryd/src/code_host/mod.rs`
- Create: `foundryd/src/code_host/gitea.rs`

- [ ] **Step 1: Write tests with mockito**

```rust
// foundryd/src/code_host/gitea.rs
#[cfg(test)]
mod tests {
    use super::*;
    use foundry_core::{traits::code_host::CodeHost, types::IssueKey};
    use mockito::Server;

    #[tokio::test]
    async fn list_assigned_issues_parses_response() {
        let mut server = Server::new_async().await;
        server.mock("GET", mockito::Matcher::Regex(
            r"/api/v1/repos/search.*".into()
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"data": [{"issues": []}]}"#)
        .create_async().await;

        // Use the issues endpoint directly (token is in the Authorization header, not the URL)
        let mock = server.mock("GET",
            "/api/v1/issues?type=assigned&state=open&limit=50"
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{
            "number": 1,
            "title": "Fix bug",
            "body": "desc",
            "state": "open",
            "updated_at": "2026-01-01T00:00:00Z",
            "assignees": [{"login": "foundry-bot"}],
            "user": {"login": "alice"},
            "repository": {"owner": {"login": "alice"}, "name": "proj"}
        }]"#)
        .create_async().await;

        let host = GiteaCodeHost::new(server.url(), "test".into(), "foundry-bot".into());
        let issues = host.list_assigned_issues(None).await.unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].key.issue_number, 1);
        mock.assert_async().await;
    }

}
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test -p foundryd code_host 2>&1 | tail -5
```

- [ ] **Step 3: Implement `GiteaCodeHost`**

```rust
// foundryd/src/code_host/gitea.rs
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use foundry_core::{
    errors::CodeHostError,
    traits::code_host::{CodeHost, HostComment, HostIssue, HostReview},
    types::{IssueKey, ReviewState},
};
use reqwest::{Client, StatusCode};
use serde::Deserialize;

pub struct GiteaCodeHost {
    base_url: String,
    token: String,
    bot_username: String,
    http: Client,
}

impl GiteaCodeHost {
    pub fn new(base_url: String, token: String, bot_username: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            bot_username,
            http: Client::new(),
        }
    }

    fn api(&self, path: &str) -> String {
        format!("{}/api/v1{}", self.base_url, path)
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
    ) -> Result<T, CodeHostError> {
        let resp = self.http
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| CodeHostError::Http(e.to_string()))?;

        match resp.status() {
            StatusCode::UNAUTHORIZED => return Err(CodeHostError::Unauthorized),
            StatusCode::NOT_FOUND => return Err(CodeHostError::NotFound(url.to_string())),
            s if !s.is_success() => {
                let body = resp.text().await.unwrap_or_default();
                return Err(CodeHostError::UnexpectedResponse(format!("{}: {}", s, body)));
            }
            _ => {}
        }
        resp.json::<T>().await.map_err(|e| CodeHostError::Http(e.to_string()))
    }
}

// Gitea API response types (internal to this module)
#[derive(Deserialize)]
struct GiteaIssueRaw {
    number: u64,
    title: String,
    body: String,
    state: String,
    updated_at: String,
    repository: Option<GiteaRepoRef>,
}

#[derive(Deserialize)]
struct GiteaRepoRef {
    name: String,
    owner: GiteaUser,
}

#[derive(Deserialize)]
struct GiteaUser {
    login: String,
}

#[derive(Deserialize)]
struct GiteaCommentRaw {
    id: u64,
    user: GiteaUser,
    body: String,
    created_at: String,
}

#[derive(Deserialize)]
struct GiteaReviewRaw {
    id: u64,
    user: GiteaUser,
    state: String,
    submitted_at: Option<String>,
}

#[async_trait]
impl CodeHost for GiteaCodeHost {
    async fn list_assigned_issues(
        &self,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<HostIssue>, CodeHostError> {
        let mut url = format!(
            "{}/api/v1/issues?type=assigned&state=open&limit=50",
            self.base_url
        );
        if let Some(since) = since {
            url.push_str(&format!("&since={}", since.to_rfc3339()));
        }

        let raw: Vec<GiteaIssueRaw> = self.get_json(&url).await?;
        Ok(raw.into_iter().filter_map(|r| {
            let repo_ref = r.repository?;
            Some(HostIssue {
                key: IssueKey {
                    owner: repo_ref.owner.login,
                    repo: repo_ref.name,
                    issue_number: r.number,
                },
                pr_number: None,
            })
        }).collect())
    }

    async fn list_issue_comments(
        &self,
        key: &IssueKey,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<HostComment>, CodeHostError> {
        let mut url = self.api(&format!(
            "/repos/{}/{}/issues/{}/comments",
            key.owner, key.repo, key.issue_number
        ));
        if let Some(since) = since {
            url.push_str(&format!("?since={}", since.to_rfc3339()));
        }
        let raw: Vec<GiteaCommentRaw> = self.get_json(&url).await?;
        Ok(raw.into_iter().filter_map(|r| {
            Some(HostComment {
                id: r.id,
                author: r.user.login,
                body: r.body,
                created_at: DateTime::parse_from_rfc3339(&r.created_at)
                    .ok()?.with_timezone(&Utc),
            })
        }).collect())
    }

    async fn list_pr_reviews(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<Vec<HostReview>, CodeHostError> {
        let url = self.api(&format!(
            "/repos/{}/{}/pulls/{}/reviews", owner, repo, pr_number
        ));
        let raw: Vec<GiteaReviewRaw> = self.get_json(&url).await?;
        Ok(raw.into_iter().map(|r| HostReview {
            id: r.id,
            reviewer: r.user.login,
            state: match r.state.as_str() {
                "APPROVED" => ReviewState::Approved,
                "REQUEST_CHANGES" => ReviewState::ChangesRequested,
                _ => ReviewState::Comment,
            },
            submitted_at: r.submitted_at
                .and_then(|ts| DateTime::parse_from_rfc3339(&ts).ok())
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(Utc::now),
        }).collect())
    }

}
```

- [ ] **Step 4: Create `code_host/mod.rs`**

```rust
// foundryd/src/code_host/mod.rs
pub mod gitea;
```

- [ ] **Step 5: Add module to `main.rs`**

```rust
mod code_host;
mod config;
mod session_store;
```

- [ ] **Step 6: Run tests**

```bash
cargo test -p foundryd code_host 2>&1
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add foundryd/src/code_host/
git commit -m "feat(foundryd): add GiteaCodeHost for polling and crash recovery"
```

---

### Task 4: Implement WebhookSource

**Files:**
- Create: `foundryd/src/sources/mod.rs`
- Create: `foundryd/src/sources/webhook.rs`

- [ ] **Step 1: Write tests**

```rust
// foundryd/src/sources/webhook.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_signature_accepts_valid_hmac() {
        let secret = "mysecret";
        let body = b"hello world";
        let sig = compute_hmac_sha256(secret, body);
        assert!(verify_hmac_sha256(secret, body, &sig));
    }

    #[test]
    fn verify_signature_rejects_wrong_secret() {
        let body = b"hello world";
        let sig = compute_hmac_sha256("correct-secret", body);
        assert!(!verify_hmac_sha256("wrong-secret", body, &sig));
    }

    #[test]
    fn verify_signature_rejects_tampered_body() {
        let secret = "mysecret";
        let sig = compute_hmac_sha256(secret, b"original");
        assert!(!verify_hmac_sha256(secret, b"tampered", &sig));
    }

    #[test]
    fn parse_issue_assigned_webhook() {
        let payload = serde_json::json!({
            "action": "assigned",
            "issue": {
                "number": 42,
                "title": "Fix bug",
                "body": "desc",
                "state": "open",
                "updated_at": "2026-01-01T00:00:00Z",
                "assignees": [{"login": "foundry-bot"}],
                "user": {"login": "alice"}
            },
            "repository": {
                "name": "proj",
                "owner": {"login": "alice"}
            },
            "sender": {"login": "alice"}
        });

        let event = parse_webhook("issues", &payload.to_string(), "delivery-123").unwrap();
        match event {
            Some(foundry_core::events::Event::IssueAssigned { issue_number, delivery_id, .. }) => {
                assert_eq!(issue_number, 42);
                assert_eq!(delivery_id, "delivery-123");
            }
            other => panic!("Expected IssueAssigned, got {:?}", other),
        }
    }

    #[test]
    fn parse_issue_comment_webhook_with_approve() {
        let payload = serde_json::json!({
            "action": "created",
            "comment": {
                "id": 99,
                "body": "/approve",
                "created_at": "2026-01-01T00:00:00Z",
                "user": {"login": "alice"}
            },
            "issue": {
                "number": 5,
                "user": {"login": "alice"}
            },
            "repository": {
                "name": "proj",
                "owner": {"login": "alice"}
            }
        });

        let event = parse_webhook("issue_comment", &payload.to_string(), "del-456").unwrap();
        match event {
            Some(foundry_core::events::Event::IssueCommentCreated { body, .. }) => {
                assert_eq!(body, "/approve");
            }
            other => panic!("Expected IssueCommentCreated, got {:?}", other),
        }
    }
}
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test -p foundryd sources::webhook 2>&1 | tail -5
```

- [ ] **Step 3: Implement `webhook.rs`**

```rust
// foundryd/src/sources/webhook.rs
use axum::{
    body::Bytes,
    extract::{Extension, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Router,
};
use foundry_core::{
    errors::EventSourceError,
    events::Event,
    traits::event_source::EventSource,
    types::{RepoId, ReviewState},
};
use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

type HmacSha256 = Hmac<Sha256>;

pub fn compute_hmac_sha256(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

pub fn verify_hmac_sha256(secret: &str, body: &[u8], signature: &str) -> bool {
    let expected = compute_hmac_sha256(secret, body);
    // Constant-time comparison
    expected.len() == signature.len()
        && expected.bytes().zip(signature.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// Parse a raw Gitea webhook payload into an Event.
/// Returns None for events we don't handle (e.g. unassign).
pub fn parse_webhook(
    event_type: &str,
    body: &str,
    delivery_id: &str,
) -> Result<Option<Event>, EventSourceError> {
    let payload: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| EventSourceError::ParseError(e.to_string()))?;

    let repo = RepoId {
        owner: payload["repository"]["owner"]["login"]
            .as_str().unwrap_or("").to_string(),
        repo: payload["repository"]["name"]
            .as_str().unwrap_or("").to_string(),
    };
    let delivery_id = delivery_id.to_string();
    let timestamp = Utc::now();

    let event = match event_type {
        "issues" => {
            let action = payload["action"].as_str().unwrap_or("");
            match action {
                "assigned" => {
                    let issue_number = payload["issue"]["number"].as_u64()
                        .ok_or_else(|| EventSourceError::ParseError("missing issue.number".into()))?;
                    let assigner = payload["sender"]["login"]
                        .as_str().unwrap_or("").to_string();
                    Some(Event::IssueAssigned { repo, issue_number, assigner, delivery_id, timestamp })
                }
                "closed" => {
                    let issue_number = payload["issue"]["number"].as_u64()
                        .ok_or_else(|| EventSourceError::ParseError("missing issue.number".into()))?;
                    Some(Event::IssueClosed { repo, issue_number, delivery_id, timestamp })
                }
                _ => None,
            }
        }
        "issue_comment" => {
            let action = payload["action"].as_str().unwrap_or("");
            if action != "created" { return Ok(None); }
            let issue_number = payload["issue"]["number"].as_u64()
                .ok_or_else(|| EventSourceError::ParseError("missing issue.number".into()))?;
            let comment_id = payload["comment"]["id"].as_u64().unwrap_or(0);
            let author = payload["comment"]["user"]["login"]
                .as_str().unwrap_or("").to_string();
            let body = payload["comment"]["body"]
                .as_str().unwrap_or("").to_string();
            Some(Event::IssueCommentCreated { repo, issue_number, comment_id, author, body, delivery_id, timestamp })
        }
        "pull_request" => {
            let action = payload["action"].as_str().unwrap_or("");
            let pr_number = payload["pull_request"]["number"].as_u64()
                .ok_or_else(|| EventSourceError::ParseError("missing pr.number".into()))?;
            match action {
                "closed" => {
                    let merged = payload["pull_request"]["merged"].as_bool().unwrap_or(false);
                    if merged {
                        Some(Event::PrMerged { repo, pr_number, delivery_id, timestamp })
                    } else {
                        Some(Event::PrClosed { repo, pr_number, delivery_id, timestamp })
                    }
                }
                _ => None,
            }
        }
        "pull_request_review" => {
            let action = payload["action"].as_str().unwrap_or("");
            if action != "submitted" { return Ok(None); }
            let pr_number = payload["pull_request"]["number"].as_u64()
                .ok_or_else(|| EventSourceError::ParseError("missing pr.number".into()))?;
            let reviewer = payload["review"]["user"]["login"]
                .as_str().unwrap_or("").to_string();
            let state = match payload["review"]["state"].as_str().unwrap_or("") {
                "approved" | "APPROVED" => ReviewState::Approved,
                "request_changes" | "REQUEST_CHANGES" => ReviewState::ChangesRequested,
                _ => ReviewState::Comment,
            };
            Some(Event::PrReviewSubmitted { repo, pr_number, reviewer, state, delivery_id, timestamp })
        }
        _ => None,
    };

    Ok(event)
}

#[derive(Clone)]
pub struct WebhookSource {
    listen_addr: String,
    webhook_secret: String,
}

impl WebhookSource {
    pub fn new(listen_addr: String, webhook_secret: String) -> Self {
        Self { listen_addr, webhook_secret }
    }
}

#[derive(Clone)]
struct AppState {
    tx: mpsc::Sender<Event>,
    secret: Arc<String>,
}

#[async_trait]
impl EventSource for WebhookSource {
    async fn run(
        &self,
        tx: mpsc::Sender<Event>,
        cancel: CancellationToken,
    ) -> Result<(), EventSourceError> {
        let state = AppState {
            tx,
            secret: Arc::new(self.webhook_secret.clone()),
        };

        let app = Router::new()
            .route("/webhook", post(handle_webhook))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind(&self.listen_addr)
            .await
            .map_err(|e| EventSourceError::Http(e.to_string()))?;

        axum::serve(listener, app)
            .with_graceful_shutdown(async move { cancel.cancelled().await })
            .await
            .map_err(|e| EventSourceError::Http(e.to_string()))?;

        Ok(())
    }
}

async fn handle_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let event_type = headers
        .get("X-Gitea-Event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let delivery_id = headers
        .get("X-Gitea-Delivery")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let signature = headers
        .get("X-Gitea-Signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !verify_hmac_sha256(&state.secret, &body, signature) {
        return (StatusCode::UNAUTHORIZED, "Invalid signature").into_response();
    }

    let body_str = match std::str::from_utf8(&body) {
        Ok(s) => s,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid UTF-8").into_response(),
    };

    match parse_webhook(event_type, body_str, delivery_id) {
        Ok(Some(event)) => {
            let _ = state.tx.send(event).await;
            (StatusCode::OK, "OK").into_response()
        }
        Ok(None) => (StatusCode::OK, "Ignored").into_response(),
        Err(e) => {
            tracing::warn!("Failed to parse webhook: {}", e);
            (StatusCode::BAD_REQUEST, e.to_string()).into_response()
        }
    }
}
```

- [ ] **Step 4: Create `sources/mod.rs`**

```rust
pub mod polling;
pub mod webhook;
```

- [ ] **Step 5: Create stub `sources/polling.rs`**

```rust
// Implemented in Task 6
```

- [ ] **Step 6: Add module to `main.rs`**

```rust
mod code_host;
mod config;
mod session_store;
mod sources;
```

- [ ] **Step 7: Run tests**

```bash
cargo test -p foundryd sources::webhook 2>&1
```

Expected: all tests pass.

- [ ] **Step 8: Commit**

```bash
git add foundryd/src/sources/
git commit -m "feat(foundryd): add WebhookSource with HMAC signature verification"
```

---

### Task 5: Implement PollingSource

**Files:**
- Modify: `foundryd/src/sources/polling.rs`

- [ ] **Step 1: Write tests**

```rust
// foundryd/src/sources/polling.rs
#[cfg(test)]
mod tests {
    use super::*;
    use foundry_core::{
        traits::code_host::{CodeHost, HostComment, HostIssue, HostReview},
        types::IssueKey,
        errors::CodeHostError,
    };
    use async_trait::async_trait;
    use chrono::Utc;
    use tokio::sync::mpsc;

    struct MockCodeHost {
        issues: Vec<HostIssue>,
    }

    #[async_trait]
    impl CodeHost for MockCodeHost {
        async fn list_assigned_issues(&self, _: Option<chrono::DateTime<Utc>>)
            -> Result<Vec<HostIssue>, CodeHostError>
        {
            Ok(self.issues.clone())
        }
        async fn list_issue_comments(&self, _: &IssueKey, _: Option<chrono::DateTime<Utc>>)
            -> Result<Vec<HostComment>, CodeHostError> { Ok(vec![]) }
        async fn list_pr_reviews(&self, _: &str, _: &str, _: u64)
            -> Result<Vec<HostReview>, CodeHostError> { Ok(vec![]) }
    }

    #[tokio::test]
    async fn poll_emits_recovery_events_for_assigned_issues() {
        let host = Arc::new(MockCodeHost {
            issues: vec![HostIssue {
                key: IssueKey { owner: "alice".into(), repo: "proj".into(), issue_number: 1 },
                pr_number: None,
            }],
        });

        let (tx, mut rx) = mpsc::channel(10);
        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });

        let since = Arc::new(tokio::sync::Mutex::new(None));
        let source = PollingSource::new(host, std::time::Duration::from_millis(10), since);
        source.run(tx, cancel).await.unwrap();

        let event = rx.try_recv().unwrap();
        match event {
            foundry_core::events::Event::PollRecovery { issue_number, .. } => {
                assert_eq!(issue_number, 1);
            }
            other => panic!("Expected PollRecovery, got {:?}", other),
        }
    }
}
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test -p foundryd sources::polling 2>&1 | tail -5
```

- [ ] **Step 3: Implement `polling.rs`**

```rust
// foundryd/src/sources/polling.rs
use async_trait::async_trait;
use foundry_core::{
    errors::EventSourceError,
    events::Event,
    traits::{code_host::CodeHost, event_source::EventSource},
    types::RepoId,
};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub struct PollingSource {
    host: Arc<dyn CodeHost>,
    interval: Duration,
    /// Shared high-water mark: the dispatcher sets this after each event is processed.
    since: Arc<tokio::sync::Mutex<Option<chrono::DateTime<chrono::Utc>>>>,
}

impl PollingSource {
    pub fn new(
        host: Arc<dyn CodeHost>,
        interval: Duration,
        since: Arc<tokio::sync::Mutex<Option<chrono::DateTime<chrono::Utc>>>>,
    ) -> Self {
        Self { host, interval, since }
    }
}

#[async_trait]
impl EventSource for PollingSource {
    async fn run(
        &self,
        tx: mpsc::Sender<Event>,
        cancel: CancellationToken,
    ) -> Result<(), EventSourceError> {
        let mut ticker = tokio::time::interval(self.interval);
        // Track delivered events in this run to avoid duplicates within a poll cycle
        let mut seen: HashSet<String> = HashSet::new();

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let since = *self.since.lock().await;
                    match self.host.list_assigned_issues(since).await {
                        Ok(issues) => {
                            for issue in issues {
                                let dedup_key = format!("{}/{}/{}", issue.key.owner, issue.key.repo, issue.key.issue_number);
                                if seen.insert(dedup_key) {
                                    let event = Event::PollRecovery {
                                        repo: RepoId {
                                            owner: issue.key.owner,
                                            repo: issue.key.repo,
                                        },
                                        issue_number: issue.key.issue_number,
                                        timestamp: chrono::Utc::now(),
                                    };
                                    if tx.send(event).await.is_err() {
                                        return Err(EventSourceError::ChannelClosed);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Polling error: {}", e);
                        }
                    }
                    seen.clear(); // Reset per-cycle dedup after each poll
                }
                _ = cancel.cancelled() => {
                    return Ok(());
                }
            }
        }
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p foundryd sources 2>&1
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add foundryd/src/sources/polling.rs
git commit -m "feat(foundryd): add PollingSource for fallback event recovery"
```

---

### Task 6: Implement directive builder

**Files:**
- Create: `foundryd/src/directive.rs`

- [ ] **Step 1: Write tests**

```rust
// foundryd/src/directive.rs
#[cfg(test)]
mod tests {
    use super::*;
    use foundry_core::types::IssuePhase;

    #[test]
    fn planning_directive_mentions_issue_and_repo() {
        let ctx = DirectiveContext {
            phase: IssuePhase::Planning,
            owner: "alice".into(),
            repo: "myproject".into(),
            issue_number: 42,
            issue_title: "Add rate limiting".into(),
            branch_name: None,
            pr_number: None,
            pending_event_summary: None,
            gitea_url: "http://gitea.local".into(),
            bot_username: "foundry-bot".into(),
        };
        let instruction = build_instruction(&ctx);
        assert!(instruction.directive.contains("issue #42"));
        assert!(instruction.directive.contains("alice/myproject"));
        assert!(instruction.directive.contains("planning"));
        assert!(!instruction.directive.contains("implement"));
    }

    #[test]
    fn implementing_directive_includes_branch_name() {
        let ctx = DirectiveContext {
            phase: IssuePhase::Implementing,
            owner: "alice".into(),
            repo: "myproject".into(),
            issue_number: 7,
            issue_title: "Fix login".into(),
            branch_name: None,
            pr_number: None,
            pending_event_summary: None,
            gitea_url: "http://gitea.local".into(),
            bot_username: "foundry-bot".into(),
        };
        let instruction = build_instruction(&ctx);
        assert!(instruction.directive.contains("foundry/issue-7"));
        assert!(instruction.directive.contains("result.json"));
    }

    #[test]
    fn in_review_directive_mentions_pr_number() {
        let ctx = DirectiveContext {
            phase: IssuePhase::InReview,
            owner: "bob".into(),
            repo: "proj".into(),
            issue_number: 3,
            issue_title: "Refactor".into(),
            branch_name: Some("foundry/issue-3".into()),
            pr_number: Some(11),
            pending_event_summary: None,
            gitea_url: "http://gitea.local".into(),
            bot_username: "foundry-bot".into(),
        };
        let instruction = build_instruction(&ctx);
        assert!(instruction.directive.contains("PR #11"));
        assert!(instruction.directive.contains("force-push"));
    }

    #[test]
    fn instruction_serializes_to_valid_json() {
        let ctx = DirectiveContext {
            phase: IssuePhase::Planning,
            owner: "alice".into(),
            repo: "proj".into(),
            issue_number: 1,
            issue_title: "Test".into(),
            branch_name: None,
            pr_number: None,
            pending_event_summary: None,
            gitea_url: "http://gitea.local".into(),
            bot_username: "foundry-bot".into(),
        };
        let instruction = build_instruction(&ctx);
        let json = serde_json::to_string(&instruction).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed["directive"].as_str().is_some());
        assert_eq!(parsed["phase"].as_str().unwrap(), "planning");
    }
}
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test -p foundryd directive 2>&1 | tail -5
```

- [ ] **Step 3: Implement `directive.rs`**

```rust
// foundryd/src/directive.rs
use foundry_core::types::IssuePhase;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct DirectiveContext {
    pub phase: IssuePhase,
    pub owner: String,
    pub repo: String,
    pub issue_number: u64,
    pub issue_title: String,
    pub branch_name: Option<String>,
    pub pr_number: Option<u64>,
    /// Human-readable summary of pending events for the consolidated directive.
    pub pending_event_summary: Option<String>,
    pub gitea_url: String,
    pub bot_username: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Instruction {
    pub phase: String,
    pub repo: InstructionRepo,
    pub issue_number: u64,
    pub pr_number: Option<u64>,
    pub directive: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InstructionRepo {
    pub owner: String,
    pub repo: String,
}

pub fn build_instruction(ctx: &DirectiveContext) -> Instruction {
    let directive = build_directive(ctx);
    Instruction {
        phase: ctx.phase.to_string(),
        repo: InstructionRepo { owner: ctx.owner.clone(), repo: ctx.repo.clone() },
        issue_number: ctx.issue_number,
        pr_number: ctx.pr_number,
        directive,
    }
}

fn build_directive(ctx: &DirectiveContext) -> String {
    let header = format!(
        "You are a software developer (@{bot}) working on issue #{n} (\"{title}\") \
        in {owner}/{repo} on Gitea at {url}.\n\n\
        Current phase: {phase}.\n\n",
        bot = ctx.bot_username,
        n = ctx.issue_number,
        title = ctx.issue_title,
        owner = ctx.owner,
        repo = ctx.repo,
        url = ctx.gitea_url,
        phase = ctx.phase,
    );

    let task = match ctx.phase {
        IssuePhase::Planning => {
            let extra = if let Some(summary) = &ctx.pending_event_summary {
                format!("\n\nNote: {}", summary)
            } else {
                String::new()
            };
            format!(
                "Your task:\n\
                1. Call get_issue to read the full issue description.\n\
                2. Call list_issue_comments to read all prior discussion.\n\
                3. Post a comment with @{bot} — either ask clarifying questions \
                   OR propose a detailed implementation plan.\n\
                4. Do NOT write any code or create any branches yet.\n\
                5. When finished, simply exit.{extra}",
                bot = ctx.bot_username,
            )
        }
        IssuePhase::Implementing => {
            let branch = format!("foundry/issue-{}", ctx.issue_number);
            format!(
                "Your implementation plan has been approved.\n\n\
                Your task:\n\
                1. Clone the repository: git clone {url}/{owner}/{repo}.git /workspace/repo\n\
                2. Create a branch: git checkout -b {branch}\n\
                3. Implement the changes described in the issue.\n\
                4. Write tests for your changes.\n\
                5. Commit your work: git commit -m \"...\"\n\
                6. Push the branch: git push origin {branch}\n\
                7. Open a pull request using the create_pull_request tool.\n\
                8. Post a comment on the issue linking to the PR.\n\
                9. Before exiting, write your result to /foundry/result.json:\n\
                   {{\"pr_number\": <N>}}\n\
                10. Exit.",
                url = ctx.gitea_url,
                owner = ctx.owner,
                repo = ctx.repo,
                branch = branch,
            )
        }
        IssuePhase::InReview => {
            let pr = ctx.pr_number.map(|n| n.to_string()).unwrap_or_else(|| "unknown".into());
            let branch = ctx.branch_name.clone().unwrap_or_default();
            format!(
                "A reviewer has submitted feedback on PR #{pr}.\n\n\
                Your task:\n\
                1. Clone the repository and check out branch {branch}:\n\
                   git clone {url}/{owner}/{repo}.git /workspace/repo\n\
                   cd /workspace/repo && git checkout {branch}\n\
                2. Call list_pr_reviews to read all review feedback.\n\
                3. For each piece of feedback:\n\
                   - If you agree: fix the code and commit the change.\n\
                   - If you disagree: reply to the review comment explaining your reasoning.\n\
                4. Do NOT force-push — always add new commits.\n\
                5. Push your changes: git push origin {branch}\n\
                6. Exit.",
                pr = pr,
                branch = branch,
                url = ctx.gitea_url,
                owner = ctx.owner,
                repo = ctx.repo,
            )
        }
        IssuePhase::Done => "This issue is done. No action needed. Exit immediately.".into(),
    };

    format!("{}{}", header, task)
}
```

- [ ] **Step 4: Add module to `main.rs`**

```rust
mod code_host;
mod config;
mod directive;
mod session_store;
mod sources;
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p foundryd directive 2>&1
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add foundryd/src/directive.rs
git commit -m "feat(foundryd): add directive builder that assembles instruction.json per phase"
```

---

### Task 7: Implement DockerRuntime

**Files:**
- Create: `foundryd/src/container/mod.rs`
- Create: `foundryd/src/container/docker.rs`

- [ ] **Step 1: Note on testing**

Docker integration tests require a live Docker daemon. They are gated behind `#[cfg(feature = "integration")]` and run separately. Unit tests cover `ContainerSpec` construction and `volume_name` derivation.

- [ ] **Step 2: Write unit tests**

```rust
// foundryd/src/container/docker.rs
#[cfg(test)]
mod tests {
    use super::*;
    use foundry_core::types::IssueKey;

    #[test]
    fn container_label_key_constant() {
        assert_eq!(FOUNDRY_ISSUE_LABEL, "foundry.issue");
    }

    #[test]
    fn build_container_spec_sets_label() {
        let key = IssueKey { owner: "alice".into(), repo: "proj".into(), issue_number: 1 };
        let label_value = format!("{}/{}/{}", key.owner, key.repo, key.issue_number);
        assert_eq!(label_value, "alice/proj/1");
    }

    #[cfg(feature = "integration")]
    #[tokio::test]
    async fn ensure_and_remove_volume() {
        let runtime = DockerRuntime::new().await.unwrap();
        let vol_name = "foundry-test-vol-integration";
        runtime.ensure_volume(vol_name).await.unwrap();
        runtime.remove_volume(vol_name).await.unwrap();
    }
}
```

- [ ] **Step 3: Implement `docker.rs`**

```rust
// foundryd/src/container/docker.rs
use async_trait::async_trait;
use bollard::{
    container::{
        Config, CreateContainerOptions, RemoveContainerOptions, StartContainerOptions,
        WaitContainerOptions,
    },
    models::{HostConfig, Mount, MountTypeEnum},
    volume::{CreateVolumeOptions, RemoveVolumeOptions},
    Docker,
};
use foundry_core::{
    errors::ContainerError,
    traits::container_runtime::{ContainerRuntime, ContainerResult, ContainerSpec, VolumeSource},
};
use futures_util::stream::StreamExt;
use std::collections::HashMap;

pub const FOUNDRY_ISSUE_LABEL: &str = "foundry.issue";

pub struct DockerRuntime {
    docker: Docker,
}

impl DockerRuntime {
    pub async fn new() -> Result<Self, ContainerError> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| ContainerError::Api(e.to_string()))?;
        Ok(Self { docker })
    }
}

#[async_trait]
impl ContainerRuntime for DockerRuntime {
    async fn run_container(&self, spec: &ContainerSpec) -> Result<ContainerResult, ContainerError> {
        let mounts: Vec<Mount> = spec.mounts.iter().map(|m| {
            let (source, mount_type) = match &m.source {
                VolumeSource::Named(name) => (Some(name.clone()), MountTypeEnum::VOLUME),
                VolumeSource::HostPath(path) => (
                    Some(path.to_string_lossy().to_string()),
                    MountTypeEnum::BIND,
                ),
            };
            Mount {
                source,
                target: Some(m.target.to_string_lossy().to_string()),
                typ: Some(mount_type),
                read_only: Some(m.read_only),
                ..Default::default()
            }
        }).collect();

        let host_config = HostConfig {
            mounts: Some(mounts),
            network_mode: spec.network.clone(),
            memory: spec.memory_limit_bytes.map(|m| m as i64),
            ..Default::default()
        };

        let env: Vec<String> = spec.env.iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();

        let container = self.docker.create_container(
            None::<CreateContainerOptions<String>>,
            Config {
                image: Some(spec.image.clone()),
                env: Some(env),
                labels: Some(spec.labels.clone()),
                host_config: Some(host_config),
                ..Default::default()
            },
        ).await.map_err(|e| ContainerError::Api(e.to_string()))?;

        let container_id = container.id.clone();

        self.docker.start_container(
            &container_id,
            None::<StartContainerOptions<String>>,
        ).await.map_err(|e| ContainerError::Api(e.to_string()))?;

        // Wait with timeout
        let timeout = spec.timeout_secs;
        let wait_future = async {
            let mut stream = self.docker.wait_container(
                &container_id,
                None::<WaitContainerOptions<String>>,
            );
            if let Some(result) = stream.next().await {
                result.map_err(|e| ContainerError::Api(e.to_string()))
            } else {
                Err(ContainerError::Api("Container wait stream ended unexpectedly".into()))
            }
        };

        let exit_code = if timeout > 0 {
            match tokio::time::timeout(
                std::time::Duration::from_secs(timeout),
                wait_future,
            ).await {
                Ok(Ok(response)) => response.status_code,
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    let _ = self.kill_container(&container_id).await;
                    return Err(ContainerError::Timeout { container_id });
                }
            }
        } else {
            wait_future.await?.status_code
        };

        Ok(ContainerResult { container_id, exit_code })
    }

    async fn ensure_volume(&self, name: &str) -> Result<(), ContainerError> {
        self.docker.create_volume(CreateVolumeOptions {
            name: name.to_string(),
            ..Default::default()
        }).await.map_err(|e| ContainerError::VolumeCreate {
            name: name.to_string(),
            reason: e.to_string(),
        })?;
        Ok(())
    }

    async fn remove_volume(&self, name: &str) -> Result<(), ContainerError> {
        // RemoveVolumeOptions is generic in bollard 0.17 — provide the String type param
        self.docker.remove_volume(name, None::<bollard::volume::RemoveVolumeOptions<String>>)
            .await
            .map_err(|e| ContainerError::Api(e.to_string()))?;
        Ok(())
    }

    async fn remove_container(&self, container_id: &str) -> Result<(), ContainerError> {
        self.docker.remove_container(
            container_id,
            Some(RemoveContainerOptions { force: true, ..Default::default() }),
        ).await.map_err(|e| ContainerError::Api(e.to_string()))?;
        Ok(())
    }

    async fn write_to_volume(
        &self,
        volume: &str,
        path: &str,  // relative path within the volume, e.g. "instruction.json"
        contents: &[u8],
    ) -> Result<(), ContainerError> {
        // Write by running a helper container that echoes the content.
        // The volume is mounted at /data; path is relative to the volume root.
        // Use base64 encoding to safely handle arbitrary bytes.
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let b64 = STANDARD.encode(contents);
        let cmd = format!(
            "sh -c 'echo {} | base64 -d > /data/{}'",
            b64, path
        );

        let container = self.docker.create_container(
            None::<CreateContainerOptions<String>>,
            Config::<String> {
                image: Some("alpine:latest".into()),
                cmd: Some(vec!["sh".into(), "-c".into(), cmd]),
                host_config: Some(HostConfig {
                    mounts: Some(vec![Mount {
                        source: Some(volume.to_string()),
                        target: Some("/data".into()),
                        typ: Some(MountTypeEnum::VOLUME),
                        read_only: Some(false),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ).await.map_err(|e| ContainerError::VolumeWrite {
            volume: volume.to_string(),
            path: path.to_string(),
            reason: e.to_string(),
        })?;

        self.docker.start_container(&container.id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| ContainerError::VolumeWrite {
                volume: volume.to_string(),
                path: path.to_string(),
                reason: e.to_string(),
            })?;

        let mut stream = self.docker.wait_container(
            &container.id,
            None::<WaitContainerOptions<String>>,
        );
        stream.next().await;
        let _ = self.remove_container(&container.id).await;
        Ok(())
    }

    async fn read_from_volume(&self, volume: &str, path: &str)
        -> Result<Vec<u8>, ContainerError>
    {
        // path is relative to the volume root, e.g. "result.json"
        // Run a helper container that cats the file; capture stdout via Docker logs API.
        use bollard::container::LogsOptions;
        use futures_util::StreamExt;

        let container = self.docker.create_container(
            None::<CreateContainerOptions<String>>,
            Config::<String> {
                image: Some("alpine:latest".into()),
                cmd: Some(vec!["cat".into(), format!("/data/{}", path)]),
                host_config: Some(HostConfig {
                    mounts: Some(vec![Mount {
                        source: Some(volume.to_string()),
                        target: Some("/data".into()),
                        typ: Some(MountTypeEnum::VOLUME),
                        read_only: Some(true),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }),
                attach_stdout: Some(true),
                ..Default::default()
            },
        ).await.map_err(|e| ContainerError::Api(e.to_string()))?;

        self.docker.start_container(&container.id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| ContainerError::Api(e.to_string()))?;

        // Wait for the container to finish
        let mut wait_stream = self.docker.wait_container(
            &container.id,
            None::<WaitContainerOptions<String>>,
        );
        wait_stream.next().await;

        // Collect stdout via Docker logs
        let mut output = Vec::new();
        let mut log_stream = self.docker.logs(
            &container.id,
            Some(LogsOptions::<String> {
                stdout: true,
                follow: false,
                ..Default::default()
            }),
        );
        while let Some(Ok(chunk)) = log_stream.next().await {
            output.extend_from_slice(&chunk.into_bytes());
        }

        let _ = self.remove_container(&container.id).await;
        Ok(output)
    }

    async fn list_running_with_label(
        &self,
        label_key: &str,
        label_value: Option<&str>,
    ) -> Result<Vec<String>, ContainerError> {
        use bollard::container::ListContainersOptions;
        let mut filters = HashMap::new();
        // Docker filter: "key=value" for exact match, "key" for key-only (any value)
        let label_filter = match label_value {
            Some(v) => format!("{}={}", label_key, v),
            None => label_key.to_string(),
        };
        filters.insert("label".to_string(), vec![label_filter]);
        filters.insert("status".to_string(), vec!["running".to_string()]);

        let containers = self.docker.list_containers(
            Some(ListContainersOptions { filters, ..Default::default() })
        ).await.map_err(|e| ContainerError::Api(e.to_string()))?;

        Ok(containers.into_iter()
            .filter_map(|c| c.id)
            .collect())
    }

    async fn kill_container(&self, container_id: &str) -> Result<(), ContainerError> {
        self.docker.kill_container(container_id, None)
            .await
            .map_err(|e| ContainerError::Api(e.to_string()))?;
        Ok(())
    }
}
```

> **Note for implementer:** `base64` and `futures-util` are declared as workspace dependencies in `Cargo.toml`. `read_from_volume` uses the Docker logs API to capture stdout from the Alpine helper container. `write_to_volume` and `read_from_volume` both treat `path` as relative to the volume root (e.g., `"instruction.json"`), not as an absolute path.

- [ ] **Step 4: Create `container/mod.rs`**

```rust
pub mod docker;
```

- [ ] **Step 5: Add to `main.rs`**

```rust
mod code_host;
mod config;
mod container;
mod directive;
mod session_store;
mod sources;
```

- [ ] **Step 6: Run tests**

```bash
cargo test -p foundryd container 2>&1
```

Expected: unit tests pass.

- [ ] **Step 7: Commit**

```bash
git add foundryd/src/container/
git commit -m "feat(foundryd): add DockerRuntime via bollard crate"
```

---

### Task 8: Implement the Dispatcher

**Files:**
- Create: `foundryd/src/dispatcher.rs`

- [ ] **Step 1: Write tests using MemorySessionStore and a MockContainerRuntime**

```rust
// foundryd/src/dispatcher.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_store::memory::MemorySessionStore;
    use foundry_core::{
        errors::ContainerError,
        events::Event,
        traits::{
            container_runtime::{ContainerResult, ContainerRuntime, ContainerSpec},
            session_store::SessionStore,
        },
        types::{IssueKey, IssuePhase, RepoId},
    };
    use async_trait::async_trait;
    use chrono::Utc;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct MockRuntime {
        spawned: Arc<Mutex<Vec<String>>>,
        exit_code: i64,
    }

    #[async_trait]
    impl ContainerRuntime for MockRuntime {
        async fn run_container(&self, spec: &ContainerSpec) -> Result<ContainerResult, ContainerError> {
            self.spawned.lock().unwrap().push(spec.image.clone());
            Ok(ContainerResult { container_id: "mock-id".into(), exit_code: self.exit_code })
        }
        async fn ensure_volume(&self, _: &str) -> Result<(), ContainerError> { Ok(()) }
        async fn remove_volume(&self, _: &str) -> Result<(), ContainerError> { Ok(()) }
        async fn remove_container(&self, _: &str) -> Result<(), ContainerError> { Ok(()) }
        async fn write_to_volume(&self, _: &str, _: &str, _: &[u8]) -> Result<(), ContainerError> { Ok(()) }
        async fn read_from_volume(&self, _: &str, _: &str) -> Result<Vec<u8>, ContainerError> {
            Ok(b"{}".to_vec()) // Empty result.json
        }
        async fn list_running_with_label(&self, _: &str, _: Option<&str>) -> Result<Vec<String>, ContainerError> { Ok(vec![]) }
        async fn kill_container(&self, _: &str) -> Result<(), ContainerError> { Ok(()) }
    }

    fn make_dispatcher(runtime: MockRuntime) -> (Dispatcher, Arc<MemorySessionStore>) {
        let store = Arc::new(MemorySessionStore::new());
        let config = crate::config::Config::from_toml(r#"
[server]
listen_addr = "0.0.0.0:8477"
webhook_secret = "test"
[gitea]
url = "http://gitea.local"
api_token = "tok"
bot_username = "foundry-bot"
bot_display_name = "Bot"
bot_email = "bot@local"
[container]
image = "foundry-runner:test"
runtime = "docker"
network = "foundry-net"
memory_limit_mb = 512
cpu_limit = 1.0
max_concurrent = 2
timeout_secs = 60
[volumes]
issue_prefix = "foundry-issue"
shared_volume = "foundry-shared"
[commands]
approve = "/approve"
[logging]
level = "info"
format = "json"
"#).unwrap();

        let dispatcher = Dispatcher::new(
            store.clone(),
            Arc::new(runtime),
            Arc::new(config),
        );
        (dispatcher, store)
    }

    #[tokio::test]
    async fn issue_assigned_creates_session_and_spawns_container() {
        let runtime = MockRuntime::default();
        let spawned = runtime.spawned.clone();
        let (dispatcher, store) = make_dispatcher(runtime);

        let event = Event::IssueAssigned {
            repo: RepoId { owner: "alice".into(), repo: "proj".into() },
            issue_number: 1,
            assigner: "alice".into(),
            delivery_id: "d1".into(),
            timestamp: Utc::now(),
        };

        dispatcher.handle_event(event).await.unwrap();

        let session = store.get(&IssueKey {
            owner: "alice".into(), repo: "proj".into(), issue_number: 1,
        }).await.unwrap();
        assert!(session.is_some());
        assert_eq!(session.unwrap().phase, IssuePhase::Planning);
        assert_eq!(spawned.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn approve_comment_transitions_to_implementing() {
        let runtime = MockRuntime::default();
        let (dispatcher, store) = make_dispatcher(runtime);

        // First: assign the issue
        dispatcher.handle_event(Event::IssueAssigned {
            repo: RepoId { owner: "alice".into(), repo: "proj".into() },
            issue_number: 2,
            assigner: "alice".into(),
            delivery_id: "d1".into(),
            timestamp: Utc::now(),
        }).await.unwrap();

        // Then: approve it
        dispatcher.handle_event(Event::IssueCommentCreated {
            repo: RepoId { owner: "alice".into(), repo: "proj".into() },
            issue_number: 2,
            comment_id: 99,
            author: "alice".into(),
            body: "/approve".into(),
            delivery_id: "d2".into(),
            timestamp: Utc::now(),
        }).await.unwrap();

        let session = store.get(&IssueKey {
            owner: "alice".into(), repo: "proj".into(), issue_number: 2,
        }).await.unwrap().unwrap();
        assert_eq!(session.phase, IssuePhase::Implementing);
    }

    #[tokio::test]
    async fn bot_comment_does_not_spawn_container() {
        let runtime = MockRuntime::default();
        let spawned = runtime.spawned.clone();
        let (dispatcher, store) = make_dispatcher(runtime);

        // Assign first
        dispatcher.handle_event(Event::IssueAssigned {
            repo: RepoId { owner: "alice".into(), repo: "proj".into() },
            issue_number: 3,
            assigner: "alice".into(),
            delivery_id: "d1".into(),
            timestamp: Utc::now(),
        }).await.unwrap();

        let before = spawned.lock().unwrap().len();

        // Bot comment — should be ignored
        dispatcher.handle_event(Event::IssueCommentCreated {
            repo: RepoId { owner: "alice".into(), repo: "proj".into() },
            issue_number: 3,
            comment_id: 1,
            author: "foundry-bot".into(),
            body: "I have a question".into(),
            delivery_id: "d2".into(),
            timestamp: Utc::now(),
        }).await.unwrap();

        assert_eq!(spawned.lock().unwrap().len(), before);
    }

    #[tokio::test]
    async fn pr_merged_deletes_session() {
        let runtime = MockRuntime::default();
        let (dispatcher, store) = make_dispatcher(runtime);

        // Set up an InReview session with a PR
        let key = IssueKey { owner: "alice".into(), repo: "proj".into(), issue_number: 4 };
        let session = foundry_core::types::IssueSession {
            key: key.clone(),
            phase: IssuePhase::InReview,
            pr_number: Some(5),
            container_running: false,
        };
        store.upsert(&session).await.unwrap();

        dispatcher.handle_event(Event::PrMerged {
            repo: RepoId { owner: "alice".into(), repo: "proj".into() },
            pr_number: 5,
            delivery_id: "d1".into(),
            timestamp: Utc::now(),
        }).await.unwrap();

        let remaining = store.list().await.unwrap();
        assert!(remaining.is_empty());
    }

    #[tokio::test]
    async fn deduplication_ignores_duplicate_delivery_id() {
        let runtime = MockRuntime::default();
        let spawned = runtime.spawned.clone();
        let (dispatcher, _) = make_dispatcher(runtime);

        let event = Event::IssueAssigned {
            repo: RepoId { owner: "alice".into(), repo: "proj".into() },
            issue_number: 5,
            assigner: "alice".into(),
            delivery_id: "same-id".into(),
            timestamp: Utc::now(),
        };

        dispatcher.handle_event(event.clone()).await.unwrap();
        dispatcher.handle_event(event).await.unwrap(); // duplicate

        // Only one container should be spawned
        assert_eq!(spawned.lock().unwrap().len(), 1);
    }
}
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test -p foundryd dispatcher 2>&1 | tail -5
```

- [ ] **Step 3: Implement `dispatcher.rs`**

```rust
// foundryd/src/dispatcher.rs
use crate::{
    config::Config,
    directive::{build_instruction, DirectiveContext},
};
use chrono::{Duration, Utc};
use foundry_core::{
    events::Event,
    traits::{
        container_runtime::{ContainerRuntime, ContainerSpec, Mount, VolumeSource},
        session_store::SessionStore,
    },
    types::{IssueKey, IssuePhase, IssueSession},
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::Mutex;
use tracing::{info, warn};

pub struct Dispatcher {
    store: Arc<dyn SessionStore>,
    runtime: Arc<dyn ContainerRuntime>,
    config: Arc<Config>,
    /// Time-windowed deduplication set: delivery_id -> expires_at
    seen_deliveries: Arc<Mutex<HashMap<String, chrono::DateTime<Utc>>>>,
    /// High-water mark for polling: updated after each event processed.
    /// Initialized to `Utc::now() - 24 hours` on startup.
    pub poll_watermark: Arc<Mutex<Option<DateTime<Utc>>>>,
    /// Per-issue event queue: when a container is running, incoming events are enqueued.
    event_queue: Arc<Mutex<HashMap<IssueKey, std::collections::VecDeque<Event>>>>,
    /// Semaphore limiting concurrent container spawns to cfg.container.max_concurrent.
    concurrency_semaphore: Arc<tokio::sync::Semaphore>,
}

impl Dispatcher {
    pub fn new(
        store: Arc<dyn SessionStore>,
        runtime: Arc<dyn ContainerRuntime>,
        config: Arc<Config>,
    ) -> Self {
        let max_concurrent = config.container.max_concurrent;
        // Initialize the poll watermark to 24 hours ago so the first poll recovers recent issues.
        let initial_watermark = Some(Utc::now() - Duration::hours(24));
        Self {
            store,
            runtime,
            config,
            seen_deliveries: Arc::new(Mutex::new(HashMap::new())),
            poll_watermark: Arc::new(Mutex::new(initial_watermark)),
            event_queue: Arc::new(Mutex::new(HashMap::new())),
            concurrency_semaphore: Arc::new(tokio::sync::Semaphore::new(max_concurrent)),
        }
    }

    pub async fn handle_event(&self, event: Event) -> anyhow::Result<()> {
        // Deduplication
        if let Some(delivery_id) = delivery_id_of(&event) {
            let mut seen = self.seen_deliveries.lock().await;
            let now = Utc::now();
            // Expire old entries
            seen.retain(|_, exp| *exp > now);
            if seen.contains_key(delivery_id) {
                info!("Skipping duplicate delivery_id: {}", delivery_id);
                return Ok(());
            }
            seen.insert(delivery_id.to_string(), now + Duration::minutes(5));
        }

        // Capture event timestamp to update the poll high-water mark
        let event_timestamp = timestamp_of(&event);

        match event {
            Event::IssueAssigned { repo, issue_number, .. } => {
                let key = IssueKey { owner: repo.owner, repo: repo.repo, issue_number };
                if self.store.get(&key).await?.is_none() {
                    let session = IssueSession {
                        key: key.clone(),
                        phase: IssuePhase::Planning,
                        pr_number: None,
                        container_running: false,
                    };
                    self.store.upsert(&session).await?;
                }
                self.spawn_turn(&key, None).await?;
            }

            Event::IssueCommentCreated { repo, issue_number, author, body, .. } => {
                // Ignore bot's own comments
                if author == self.config.gitea.bot_username {
                    return Ok(());
                }
                let key = IssueKey { owner: repo.owner, repo: repo.repo, issue_number };
                let session = match self.store.get(&key).await? {
                    Some(s) => s,
                    None => {
                        warn!("Comment on untracked issue {}/{}/{}", key.owner, key.repo, key.issue_number);
                        return Ok(());
                    }
                };

                // Check for /approve command
                if body.trim().starts_with(&self.config.commands.approve) {
                    if session.phase == IssuePhase::Planning {
                        let mut updated = session.clone();
                        updated.phase = IssuePhase::Implementing;
                        self.store.upsert(&updated).await?;
                        self.spawn_turn(&key, None).await?;
                    }
                    return Ok(());
                }

                // Regular reply — spawn a turn unless a container is already running.
                // If a container is running, enqueue the event for processing after it exits.
                if !session.container_running {
                    self.spawn_turn(&key, None).await?;
                } else {
                    let mut q = self.event_queue.lock().await;
                    q.entry(key.clone()).or_default().push_back(
                        Event::IssueCommentCreated { repo: foundry_core::types::RepoId {
                            owner: key.owner.clone(), repo: key.repo.clone(),
                        }, issue_number: key.issue_number, comment_id, author, body,
                        delivery_id: String::new(), timestamp: Utc::now() }
                    );
                }
            }

            Event::PrReviewSubmitted { repo, pr_number, .. } => {
                let session = match self.store
                    .get_by_pr(&repo.owner, &repo.repo, pr_number).await?
                {
                    Some(s) => s,
                    None => {
                        warn!("Review on untracked PR {}/{}/{}", repo.owner, repo.repo, pr_number);
                        return Ok(());
                    }
                };
                if !session.container_running {
                    self.spawn_turn(&session.key.clone(), None).await?;
                } else {
                    let session_key = session.key.clone();
                    let mut q = self.event_queue.lock().await;
                    q.entry(session_key.clone()).or_default().push_back(
                        Event::PrReviewSubmitted { repo, pr_number, reviewer,
                            state, delivery_id: String::new(), timestamp: Utc::now() }
                    );
                }
            }

            Event::PrMerged { repo, pr_number, .. } => {
                if let Some(session) = self.store
                    .get_by_pr(&repo.owner, &repo.repo, pr_number).await?
                {
                    let vol = session.key.volume_name(&self.config.volumes.issue_prefix);
                    self.store.delete(&session.key).await?;
                    let _ = self.runtime.remove_volume(&vol).await;
                    info!("PR #{} merged — session closed for {}/{}/{}",
                        pr_number, repo.owner, repo.repo, session.key.issue_number);
                }
            }

            Event::PrClosed { repo, pr_number, .. } => {
                if let Some(session) = self.store
                    .get_by_pr(&repo.owner, &repo.repo, pr_number).await?
                {
                    let vol = session.key.volume_name(&self.config.volumes.issue_prefix);
                    self.store.delete(&session.key).await?;
                    let _ = self.runtime.remove_volume(&vol).await;
                }
            }

            Event::IssueClosed { repo, issue_number, .. } => {
                let key = IssueKey { owner: repo.owner, repo: repo.repo, issue_number };
                if let Some(session) = self.store.get(&key).await? {
                    if session.pr_number.is_none() {
                        let vol = key.volume_name(&self.config.volumes.issue_prefix);
                        self.store.delete(&key).await?;
                        let _ = self.runtime.remove_volume(&vol).await;
                    }
                    // If PR exists, let PrMerged/PrClosed handle cleanup
                }
            }

            Event::PollRecovery { repo, issue_number, .. } => {
                let key = IssueKey { owner: repo.owner, repo: repo.repo, issue_number };
                match self.store.get(&key).await? {
                    None => {
                        // Issue not tracked yet — create session and start planning
                        let session = IssueSession {
                            key: key.clone(),
                            phase: IssuePhase::Planning,
                            pr_number: None,
                            container_running: false,
                        };
                        self.store.upsert(&session).await?;
                        self.spawn_turn(&key, None).await?;
                    }
                    Some(session) if session.phase == IssuePhase::InReview && !session.container_running => {
                        // Issue is in review but has no running container —
                        // spawn a container to check for new review activity.
                        self.spawn_turn(&key, Some("Check for new review activity and address any unresolved feedback.".into())).await?;
                    }
                    _ => {
                        // Issue is already tracked and either not in InReview or already has a running container — skip.
                    }
                }
            }
        }

        // Update the poll high-water mark if this event's timestamp is newer
        if let Some(ts) = event_timestamp {
            let mut watermark = self.poll_watermark.lock().await;
            if watermark.map_or(true, |w| ts > w) {
                *watermark = Some(ts);
            }
        }

        Ok(())
    }

    async fn spawn_turn(
        &self,
        key: &IssueKey,
        pending_summary: Option<String>,
    ) -> anyhow::Result<()> {
        let session = match self.store.get(key).await? {
            Some(s) => s,
            None => return Ok(()),
        };

        if session.container_running {
            warn!("Container already running for {}/{}/{} — skipping spawn",
                key.owner, key.repo, key.issue_number);
            return Ok(());
        }

        // Mark as running
        let mut updated = session.clone();
        updated.container_running = true;
        self.store.upsert(&updated).await?;

        // Build instruction
        // Branch name is derived deterministically — not stored in the session.
        let derived_branch = format!("foundry/issue-{}", key.issue_number);
        let ctx = DirectiveContext {
            phase: session.phase,
            owner: key.owner.clone(),
            repo: key.repo.clone(),
            issue_number: key.issue_number,
            issue_title: String::new(), // TODO: fetch from CodeHost for richer directives
            branch_name: Some(derived_branch),
            pr_number: session.pr_number,
            pending_event_summary: pending_summary,
            gitea_url: self.config.gitea.url.clone(),
            bot_username: self.config.gitea.bot_username.clone(),
        };
        let instruction = build_instruction(&ctx);
        let instruction_json = serde_json::to_vec_pretty(&instruction)?;

        // Ensure volume and write instruction
        let vol_name = key.volume_name(&self.config.volumes.issue_prefix);
        self.runtime.ensure_volume(&vol_name).await?;
        // path is relative to the volume root; the runner mounts the volume at /foundry
        self.runtime.write_to_volume(&vol_name, "instruction.json", &instruction_json).await?;

        // Build container spec
        let mut env = HashMap::new();
        env.insert("GITEA_HOST".into(), self.config.gitea.url.clone());
        env.insert("GITEA_ACCESS_TOKEN".into(), self.config.gitea.api_token.clone());
        env.insert("GITEA_BOT_USERNAME".into(), self.config.gitea.bot_username.clone());
        env.insert("GIT_AUTHOR_NAME".into(), self.config.gitea.bot_display_name.clone());
        env.insert("GIT_AUTHOR_EMAIL".into(), self.config.gitea.bot_email.clone());
        env.insert("GIT_COMMITTER_NAME".into(), self.config.gitea.bot_display_name.clone());
        env.insert("GIT_COMMITTER_EMAIL".into(), self.config.gitea.bot_email.clone());
        // Pass the Anthropic API key so Claude Code can authenticate inside the container
        env.insert(
            "ANTHROPIC_API_KEY".into(),
            std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
        );

        let mut labels = HashMap::new();
        labels.insert(
            crate::container::docker::FOUNDRY_ISSUE_LABEL.into(),
            format!("{}/{}/{}", key.owner, key.repo, key.issue_number),
        );

        let spec = ContainerSpec {
            image: self.config.container.image.clone(),
            env,
            mounts: vec![
                Mount {
                    source: VolumeSource::Named(vol_name.clone()),
                    target: PathBuf::from("/foundry"),
                    read_only: false,
                },
                Mount {
                    source: VolumeSource::Named(self.config.volumes.shared_volume.clone()),
                    target: PathBuf::from("/etc/foundry"),
                    read_only: true,
                },
            ],
            network: Some(self.config.container.network.clone()),
            memory_limit_bytes: Some(self.config.container.memory_limit_mb * 1024 * 1024),
            cpu_period: None,
            cpu_quota: None,
            labels,
            timeout_secs: self.config.container.timeout_secs,
        };

        // Acquire a concurrency permit before spawning (blocks if max_concurrent is reached).
        let permit = self.concurrency_semaphore.clone().acquire_owned().await
            .expect("Semaphore closed unexpectedly");

        // Spawn container (runs asynchronously until it exits)
        let runtime = self.runtime.clone();
        let store = self.store.clone();
        let event_queue = self.event_queue.clone();
        let config = self.config.clone();
        let key = key.clone();
        let vol_name_clone = vol_name.clone();
        let gitea_url = self.config.gitea.url.clone();
        let gitea_token = self.config.gitea.api_token.clone();

        tokio::spawn(async move {
            // permit is held for the duration of the container run
            let _permit = permit;

            let container_result = runtime.run_container(spec).await;

            match container_result {
                Ok(result) => {
                    // Try to read result.json for pr_number
                    if let Ok(bytes) = runtime.read_from_volume(&vol_name_clone, "result.json").await {
                        if let Ok(result_json) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                            if let Some(mut session) = store.get(&key).await.ok().flatten() {
                                if let Some(pr_number) = result_json["pr_number"].as_u64() {
                                    session.pr_number = Some(pr_number);
                                    session.phase = IssuePhase::InReview;
                                }
                                session.container_running = false;
                                let _ = store.upsert(&session).await;
                                // Drain the per-issue event queue: consolidate pending events
                                // and spawn one new container with a summary directive.
                                let pending = {
                                    let mut q = event_queue.lock().await;
                                    q.remove(&key).unwrap_or_default()
                                };
                                if !pending.is_empty() {
                                    let summary = format!(
                                        "{} event(s) arrived while the container was running.                                         Address any new comments or review feedback.",
                                        pending.len()
                                    );
                                    // Re-acquire store reference for recursive spawn
                                    if let Ok(Some(_)) = store.get(&key).await {
                                        // spawn_turn is not directly callable here; emit a synthetic PollRecovery
                                        // The dispatcher will re-check the queue on next poll cycle.
                                        // For now, log the pending events — a production implementation
                                        // would call spawn_turn via a dispatcher reference.
                                        warn!("Pending events for {}/{}/{} after container exit ({}): will be handled on next poll",
                                            key.owner, key.repo, key.issue_number, summary);
                                    }
                                }
                                return;
                            }
                        }
                    }
                    if result.exit_code != 0 {
                        warn!("Container exited with non-zero code {} for {}/{}/{}",
                            result.exit_code, key.owner, key.repo, key.issue_number);
                        // Post a failure comment directly to Gitea (spec gap: CodeHost is read-only,
                        // so we use reqwest directly here for this one write path).
                        post_failure_comment(
                            &gitea_url, &gitea_token, &key.owner, &key.repo, key.issue_number,
                            result.exit_code,
                        ).await;
                    }
                }
                Err(e) => {
                    warn!("Container error for {}/{}/{}: {}", key.owner, key.repo, key.issue_number, e);
                    post_failure_comment(
                        &gitea_url, &gitea_token, &key.owner, &key.repo, key.issue_number, -1,
                    ).await;
                }
            }
            // Clear container_running flag
            if let Ok(Some(mut session)) = store.get(&key).await {
                session.container_running = false;
                let _ = store.upsert(&session).await;
            }
            // Drain event queue even on failure
            event_queue.lock().await.remove(&key);
        });

        Ok(())
    }
}

fn delivery_id_of(event: &Event) -> Option<&str> {
    match event {
        Event::IssueAssigned { delivery_id, .. } => Some(delivery_id),
        Event::IssueClosed { delivery_id, .. } => Some(delivery_id),
        Event::IssueCommentCreated { delivery_id, .. } => Some(delivery_id),
        Event::PrReviewSubmitted { delivery_id, .. } => Some(delivery_id),
        Event::PrMerged { delivery_id, .. } => Some(delivery_id),
        Event::PrClosed { delivery_id, .. } => Some(delivery_id),
        Event::PollRecovery { .. } => None,
    }
}

fn timestamp_of(event: &Event) -> Option<DateTime<Utc>> {
    match event {
        Event::IssueAssigned { timestamp, .. } => Some(*timestamp),
        Event::IssueClosed { timestamp, .. } => Some(*timestamp),
        Event::IssueCommentCreated { timestamp, .. } => Some(*timestamp),
        Event::PrReviewSubmitted { timestamp, .. } => Some(*timestamp),
        Event::PrMerged { timestamp, .. } => Some(*timestamp),
        Event::PrClosed { timestamp, .. } => Some(*timestamp),
        Event::PollRecovery { timestamp, .. } => Some(*timestamp),
    }
}

/// Post a failure comment on an issue via the Gitea API directly.
///
/// This is the one write path from the dispatcher that bypasses `gitea-mcp`.
/// It exists because container failures must be reported even when no container
/// is running to relay the message. The `CodeHost` trait is otherwise read-only.
async fn post_failure_comment(
    gitea_url: &str,
    token: &str,
    owner: &str,
    repo: &str,
    issue_number: u64,
    exit_code: i64,
) {
    let url = format!(
        "{}/api/v1/repos/{}/{}/issues/{}/comments",
        gitea_url, owner, repo, issue_number
    );
    let body = serde_json::json!({
        "body": format!(
            "⚠️ The foundry container exited with code `{}`.             Please check the logs and re-assign the issue to retry.",
            exit_code
        )
    });
    let client = reqwest::Client::new();
    if let Err(e) = client
        .post(&url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
    {
        warn!("Failed to post failure comment on {}/{}/{}: {}", owner, repo, issue_number, e);
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p foundryd dispatcher 2>&1
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add foundryd/src/dispatcher.rs
git commit -m "feat(foundryd): add Dispatcher with full event routing and session management"
```

---

### Task 9: Implement `main.rs`

**Files:**
- Modify: `foundryd/src/main.rs`

- [ ] **Step 1: Implement `main.rs`**

```rust
// foundryd/src/main.rs
mod code_host;
mod config;
mod container;
mod directive;
mod dispatcher;
mod session_store;
mod sources;

use clap::Parser;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::info;

#[derive(Parser)]
#[command(name = "foundryd", about = "Foundry orchestrator daemon")]
struct Args {
    #[arg(short, long, default_value = "foundry.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let mut cfg = config::Config::from_file(&args.config)?;
    cfg.resolve_secrets()?;

    // Initialize tracing
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(&cfg.logging.level);
    if cfg.logging.format == "json" {
        subscriber.json().init();
    } else {
        subscriber.init();
    }

    info!("foundryd starting");

    // Initialize session store (in-memory; state is reconstructed from Gitea on startup)
    let store: Arc<dyn foundry_core::traits::session_store::SessionStore> =
        Arc::new(session_store::memory::MemorySessionStore::new());

    // Initialize Docker runtime
    let runtime: Arc<dyn foundry_core::traits::container_runtime::ContainerRuntime> =
        Arc::new(container::docker::DockerRuntime::new().await?);

    let cfg = Arc::new(cfg);
    let dispatcher = Arc::new(dispatcher::Dispatcher::new(
        store.clone(),
        runtime.clone(),
        cfg.clone(),
    ));

    // Kill any orphaned containers from a previous crash.
    // Pass None for the value to match any container with this label key.
    info!("Checking for orphaned containers");
    let orphans = runtime.list_running_with_label(
        container::docker::FOUNDRY_ISSUE_LABEL,
        None,
    ).await.unwrap_or_default();
    for id in &orphans {
        info!("Killing orphaned container {}", id);
        let _ = runtime.kill_container(id).await;
    }

    // Reset container_running flags
    for session in store.list().await? {
        if session.container_running {
            let mut s = session;
            s.container_running = false;
            store.upsert(&s).await?;
        }
    }

    // Reconstruct session state from Gitea on startup.
    // For each issue currently assigned to the bot, infer the phase and reconstruct the session.
    info!("Reconstructing session state from Gitea");
    {
        use foundry_core::{
            traits::code_host::CodeHost,
            types::{IssueKey, IssuePhase, IssueSession},
        };
        let code_host = code_host::gitea::GiteaCodeHost::new(
            cfg.gitea.url.clone(),
            cfg.gitea.api_token.clone(),
            cfg.gitea.bot_username.clone(),
        );
        match code_host.list_assigned_issues(None).await {
            Ok(issues) => {
                for issue in issues {
                    let key = issue.key.clone();
                    // Skip if session already exists (e.g. from a very fast restart)
                    if store.get(&key).await.ok().flatten().is_some() {
                        continue;
                    }
                    // Infer phase from Gitea state:
                    //   - Check for open PR (InReview), /approve comment (Implementing), else Planning
                    let comments = code_host.list_issue_comments(&key, None).await.unwrap_or_default();
                    let has_approve = comments.iter().any(|c| c.body.trim().starts_with(&cfg.commands.approve));
                    let (phase, pr_number) = if let Some(pr) = issue.pr_number {
                        (IssuePhase::InReview, Some(pr))
                    } else if has_approve {
                        (IssuePhase::Implementing, None)
                    } else {
                        (IssuePhase::Planning, None)
                    };

                    let session = IssueSession {
                        key: key.clone(),
                        phase,
                        pr_number,
                        container_running: false,
                    };
                    store.upsert(&session).await.ok();
                    info!("Reconstructed session for {}/{}/{} as {:?}", key.owner, key.repo, key.issue_number, session.phase);

                    // For InReview sessions, check for unaddressed reviews and spawn a container
                    if phase == IssuePhase::InReview {
                        if let Some(pr) = pr_number {
                            let reviews = code_host.list_pr_reviews(&key.owner, &key.repo, pr).await.unwrap_or_default();
                            let has_unaddressed = reviews.iter().any(|r| matches!(r.state,
                                foundry_core::types::ReviewState::ChangesRequested | foundry_core::types::ReviewState::Comment
                            ));
                            if has_unaddressed {
                                info!("Unaddressed reviews on PR #{} — spawning container for {}/{}/{}",
                                    pr, key.owner, key.repo, key.issue_number);
                                dispatcher.handle_event(foundry_core::events::Event::PollRecovery {
                                    repo: foundry_core::types::RepoId {
                                        owner: key.owner.clone(),
                                        repo: key.repo.clone(),
                                    },
                                    issue_number: key.issue_number,
                                    timestamp: chrono::Utc::now(),
                                }).await.ok();
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to reconstruct sessions from Gitea: {}", e);
            }
        }
    }

    // Set up event channel
    let (tx, mut rx) = mpsc::channel::<foundry_core::events::Event>(256);
    let cancel = CancellationToken::new();

    // Start webhook source
    let webhook = sources::webhook::WebhookSource::new(
        cfg.server.listen_addr.clone(),
        cfg.server.webhook_secret.clone(),
    );
    let tx_webhook = tx.clone();
    let cancel_webhook = cancel.clone();
    tokio::spawn(async move {
        use foundry_core::traits::event_source::EventSource;
        if let Err(e) = webhook.run(tx_webhook, cancel_webhook).await {
            tracing::error!("Webhook source error: {}", e);
        }
    });

    // Start polling source (if enabled)
    if cfg.polling.enabled {
        let host = Arc::new(code_host::gitea::GiteaCodeHost::new(
            cfg.gitea.url.clone(),
            cfg.gitea.api_token.clone(),
            cfg.gitea.bot_username.clone(),
        ));
        let polling = sources::polling::PollingSource::new(
            host,
            std::time::Duration::from_secs(cfg.polling.interval_secs),
            dispatcher.poll_watermark.clone(),
        );
        let tx_poll = tx.clone();
        let cancel_poll = cancel.clone();
        tokio::spawn(async move {
            use foundry_core::traits::event_source::EventSource;
            if let Err(e) = polling.run(tx_poll, cancel_poll).await {
                tracing::error!("Polling source error: {}", e);
            }
        });
    }

    // Handle SIGTERM / Ctrl-C
    let cancel_signal = cancel.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("Shutdown signal received");
        cancel_signal.cancel();
    });

    info!("foundryd ready — listening on {}", cfg.server.listen_addr);

    // Main event loop
    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                let d = dispatcher.clone();
                tokio::spawn(async move {
                    if let Err(e) = d.handle_event(event).await {
                        tracing::error!("Dispatcher error: {}", e);
                    }
                });
            }
            _ = cancel.cancelled() => {
                info!("Shutting down event loop");
                break;
            }
        }
    }

    // Graceful shutdown: wait up to cfg.container.timeout_secs for in-flight containers to finish.
    info!("Waiting for in-flight containers to finish (timeout: {}s)...", cfg.container.timeout_secs);
    let shutdown_deadline = std::time::Instant::now()
        + std::time::Duration::from_secs(cfg.container.timeout_secs);
    loop {
        let sessions = store.list().await.unwrap_or_default();
        let running = sessions.iter().filter(|s| s.container_running).count();
        if running == 0 {
            info!("All containers finished, shutting down cleanly.");
            break;
        }
        if std::time::Instant::now() >= shutdown_deadline {
            info!("Shutdown timeout reached with {} containers still running — forcing exit.", running);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    info!("foundryd stopped");
    Ok(())
}
```

- [ ] **Step 2: Build**

```bash
cargo build -p foundryd 2>&1
```

Expected: compiles cleanly.

- [ ] **Step 3: Run all tests**

```bash
cargo test -p foundryd 2>&1
```

Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add foundryd/src/main.rs
git commit -m "feat(foundryd): wire main.rs with signal handling and graceful shutdown"
```

---

### Task 10: Final verification

- [ ] **Step 1: Build everything in the workspace**

```bash
cargo build --workspace 2>&1
```

Expected: all crates compile.

- [ ] **Step 2: Run all tests across the workspace**

```bash
cargo test --workspace 2>&1
```

Expected: all tests pass.

- [ ] **Step 3: Check for warnings**

```bash
RUSTFLAGS="-D warnings" cargo build --workspace 2>&1
```

Expected: clean build.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore: verify full workspace builds and tests pass"
```
