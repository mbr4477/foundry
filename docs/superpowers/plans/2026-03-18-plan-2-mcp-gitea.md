# foundry-mcp-gitea Implementation Plan

> **⚠️ SUPERSEDED — Do not execute this plan.**
> The custom `foundry-mcp-gitea` Rust MCP server has been replaced by the official `gitea-mcp` binary from the Gitea project. See the design spec and plan 5 (foundry-runner) for current implementation guidance.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `foundry-mcp-gitea`, a standalone MCP server binary that runs inside each ephemeral container and exposes Gitea API operations as typed tools for Claude Code.

**Architecture:** A single Rust binary that speaks the MCP stdio protocol (JSON-RPC 2.0 over stdin/stdout). A `GiteaClient` wraps `reqwest` for all HTTP calls. Tool handlers are grouped by domain (issues, pull requests, repository) and registered with the MCP server on startup. Reads `GITEA_URL` and `GITEA_TOKEN` from environment — completely stateless.

**Tech Stack:** Rust, `rmcp` (MCP SDK), `reqwest`, `serde_json`, `tokio`, `mockito` (tests)

---

### Task 1: Create the crate

**Files:**
- Create: `foundry-mcp-gitea/Cargo.toml`
- Create: `foundry-mcp-gitea/src/main.rs`
- Create: `foundry-mcp-gitea/src/client.rs`
- Create: `foundry-mcp-gitea/src/tools/mod.rs`
- Create: `foundry-mcp-gitea/src/tools/issues.rs`
- Create: `foundry-mcp-gitea/src/tools/pull_requests.rs`
- Create: `foundry-mcp-gitea/src/tools/repository.rs`

- [ ] **Step 1: Create `foundry-mcp-gitea/Cargo.toml`**

```toml
[package]
name = "foundry-mcp-gitea"
version.workspace = true
edition.workspace = true

[[bin]]
name = "foundry-mcp-gitea"
path = "src/main.rs"

[dependencies]
foundry-core.path = "../foundry-core"
tokio.workspace = true
serde.workspace = true
serde_json.workspace = true
reqwest.workspace = true
thiserror.workspace = true
anyhow.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
rmcp.workspace = true

[dev-dependencies]
mockito = { workspace = true }
tokio = { workspace = true, features = ["test-util"] }
```

- [ ] **Step 2: Create stub `src/main.rs`**

```rust
// foundry-mcp-gitea/src/main.rs
mod client;
mod tools;

fn main() {
    println!("foundry-mcp-gitea stub");
}
```

- [ ] **Step 3: Create empty stub files**

```bash
mkdir -p foundry-mcp-gitea/src/tools
touch foundry-mcp-gitea/src/client.rs
touch foundry-mcp-gitea/src/tools/mod.rs
touch foundry-mcp-gitea/src/tools/issues.rs
touch foundry-mcp-gitea/src/tools/pull_requests.rs
touch foundry-mcp-gitea/src/tools/repository.rs
```

- [ ] **Step 4: Verify it compiles**

```bash
cargo build -p foundry-mcp-gitea 2>&1
```

Expected: `Compiling foundry-mcp-gitea v0.1.0` with no errors.

- [ ] **Step 5: Commit**

```bash
git add foundry-mcp-gitea/
git commit -m "chore(mcp-gitea): add crate skeleton"
```

---

### Task 2: Implement the Gitea HTTP client

**Files:**
- Modify: `foundry-mcp-gitea/src/client.rs`

- [ ] **Step 1: Write tests using mockito**

```rust
// foundry-mcp-gitea/src/client.rs
#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;

    #[tokio::test]
    async fn get_issue_returns_parsed_issue() {
        let mut server = Server::new_async().await;
        let mock = server.mock("GET", "/api/v1/repos/alice/proj/issues/1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{
                "number": 1,
                "title": "Test issue",
                "body": "Issue body",
                "state": "open",
                "updated_at": "2026-01-01T00:00:00Z",
                "assignees": [{"login": "foundry-bot"}],
                "user": {"login": "alice"}
            }"#)
            .create_async().await;

        let client = GiteaClient::new(server.url(), "test-token".into());
        let issue = client.get_issue("alice", "proj", 1).await.unwrap();

        assert_eq!(issue.number, 1);
        assert_eq!(issue.title, "Test issue");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_issue_returns_unauthorized_on_401() {
        let mut server = Server::new_async().await;
        server.mock("GET", "/api/v1/repos/alice/proj/issues/1")
            .with_status(401)
            .create_async().await;

        let client = GiteaClient::new(server.url(), "bad-token".into());
        let result = client.get_issue("alice", "proj", 1).await;

        assert!(matches!(result, Err(GiteaClientError::Unauthorized)));
    }

    #[tokio::test]
    async fn create_comment_posts_body() {
        let mut server = Server::new_async().await;
        let mock = server.mock("POST", "/api/v1/repos/alice/proj/issues/1/comments")
            .match_body(mockito::Matcher::JsonString(
                r#"{"body": "Hello @alice"}"#.into()
            ))
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id": 42, "body": "Hello @alice", "created_at": "2026-01-01T00:00:00Z", "user": {"login": "foundry-bot"}}"#)
            .create_async().await;

        let client = GiteaClient::new(server.url(), "test-token".into());
        let result = client.create_issue_comment("alice", "proj", 1, "Hello @alice").await;

        assert!(result.is_ok());
        mock.assert_async().await;
    }
}
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test -p foundry-mcp-gitea client 2>&1 | tail -5
```

- [ ] **Step 3: Implement `GiteaClient`**

```rust
// foundry-mcp-gitea/src/client.rs
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GiteaClientError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Unauthorized — check GITEA_TOKEN")]
    Unauthorized,
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Unexpected response {status}: {body}")]
    Unexpected { status: u16, body: String },
}

/// Gitea API response types
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GiteaIssue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: String,
    pub updated_at: String,
    pub assignees: Option<Vec<GiteaUser>>,
    pub user: GiteaUser,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GiteaUser {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GiteaComment {
    pub id: u64,
    pub body: String,
    pub created_at: String,
    pub user: GiteaUser,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GiteaRepo {
    pub name: String,
    pub description: Option<String>,
    pub default_branch: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GiteaPr {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: String,
    pub head: GiteaBranch,
    pub base: GiteaBranch,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GiteaBranch {
    pub label: String,
    #[serde(rename = "ref")]
    pub ref_name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GiteaReview {
    pub id: u64,
    pub user: GiteaUser,
    pub state: String,
    pub body: String,
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GiteaPrReviewComment {
    pub id: u64,
    pub body: String,
    pub created_at: String,
    pub user: GiteaUser,
    pub path: Option<String>,
    pub diff_hunk: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct CreateCommentBody<'a> {
    body: &'a str,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreatePrBody<'a> {
    pub title: &'a str,
    pub body: &'a str,
    pub head: &'a str,
    pub base: &'a str,
}

pub struct GiteaClient {
    base_url: String,
    token: String,
    http: Client,
}

impl GiteaClient {
    pub fn new(base_url: String, token: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            http: Client::new(),
        }
    }

    fn api(&self, path: &str) -> String {
        format!("{}/api/v1{}", self.base_url, path)
    }

    async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response, GiteaClientError> {
        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED {
            return Err(GiteaClientError::Unauthorized);
        }
        if status == StatusCode::NOT_FOUND {
            let url = resp.url().to_string();
            return Err(GiteaClientError::NotFound(url));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(GiteaClientError::Unexpected { status: status.as_u16(), body });
        }
        Ok(resp)
    }

    pub async fn get_issue(&self, owner: &str, repo: &str, number: u64)
        -> Result<GiteaIssue, GiteaClientError>
    {
        let resp = self.http
            .get(self.api(&format!("/repos/{}/{}/issues/{}", owner, repo, number)))
            .bearer_auth(&self.token)
            .send().await?;
        Ok(Self::check_status(resp).await?.json().await?)
    }

    pub async fn list_issue_comments(&self, owner: &str, repo: &str, number: u64, since: Option<&str>)
        -> Result<Vec<GiteaComment>, GiteaClientError>
    {
        let mut url = self.api(&format!("/repos/{}/{}/issues/{}/comments", owner, repo, number));
        if let Some(since) = since {
            url.push_str(&format!("?since={}", since));
        }
        let resp = self.http.get(&url).bearer_auth(&self.token).send().await?;
        Ok(Self::check_status(resp).await?.json().await?)
    }

    pub async fn create_issue_comment(&self, owner: &str, repo: &str, number: u64, body: &str)
        -> Result<GiteaComment, GiteaClientError>
    {
        let resp = self.http
            .post(self.api(&format!("/repos/{}/{}/issues/{}/comments", owner, repo, number)))
            .bearer_auth(&self.token)
            .json(&CreateCommentBody { body })
            .send().await?;
        Ok(Self::check_status(resp).await?.json().await?)
    }

    pub async fn get_repo(&self, owner: &str, repo: &str)
        -> Result<GiteaRepo, GiteaClientError>
    {
        let resp = self.http
            .get(self.api(&format!("/repos/{}/{}", owner, repo)))
            .bearer_auth(&self.token)
            .send().await?;
        Ok(Self::check_status(resp).await?.json().await?)
    }

    pub async fn get_authenticated_user(&self) -> Result<GiteaUser, GiteaClientError> {
        let resp = self.http
            .get(self.api("/user"))
            .bearer_auth(&self.token)
            .send().await?;
        Ok(Self::check_status(resp).await?.json().await?)
    }

    pub async fn create_pull_request(&self, owner: &str, repo: &str, body: &CreatePrBody<'_>)
        -> Result<GiteaPr, GiteaClientError>
    {
        let resp = self.http
            .post(self.api(&format!("/repos/{}/{}/pulls", owner, repo)))
            .bearer_auth(&self.token)
            .json(body)
            .send().await?;
        Ok(Self::check_status(resp).await?.json().await?)
    }

    pub async fn get_pull_request(&self, owner: &str, repo: &str, pr_number: u64)
        -> Result<GiteaPr, GiteaClientError>
    {
        let resp = self.http
            .get(self.api(&format!("/repos/{}/{}/pulls/{}", owner, repo, pr_number)))
            .bearer_auth(&self.token)
            .send().await?;
        Ok(Self::check_status(resp).await?.json().await?)
    }

    pub async fn list_pr_reviews(&self, owner: &str, repo: &str, pr_number: u64)
        -> Result<Vec<GiteaReview>, GiteaClientError>
    {
        let resp = self.http
            .get(self.api(&format!("/repos/{}/{}/pulls/{}/reviews", owner, repo, pr_number)))
            .bearer_auth(&self.token)
            .send().await?;
        Ok(Self::check_status(resp).await?.json().await?)
    }

    pub async fn create_pr_comment(&self, owner: &str, repo: &str, pr_number: u64, body: &str)
        -> Result<GiteaComment, GiteaClientError>
    {
        // Gitea PR comments go through the issues endpoint
        self.create_issue_comment(owner, repo, pr_number, body).await
    }

    pub async fn edit_issue(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        title: Option<&str>,
        body: Option<&str>,
        state: Option<&str>,
    ) -> Result<GiteaIssue, GiteaClientError> {
        #[derive(Serialize)]
        struct EditIssueBody<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            title: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            body: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            state: Option<&'a str>,
        }
        let resp = self.http
            .patch(self.api(&format!("/repos/{}/{}/issues/{}", owner, repo, number)))
            .bearer_auth(&self.token)
            .json(&EditIssueBody { title, body, state })
            .send().await?;
        Ok(Self::check_status(resp).await?.json().await?)
    }

    pub async fn update_pull_request(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
        title: Option<&str>,
        body: Option<&str>,
    ) -> Result<GiteaPr, GiteaClientError> {
        #[derive(Serialize)]
        struct UpdatePrBody<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            title: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            body: Option<&'a str>,
        }
        let resp = self.http
            .patch(self.api(&format!("/repos/{}/{}/pulls/{}", owner, repo, pr_number)))
            .bearer_auth(&self.token)
            .json(&UpdatePrBody { title, body })
            .send().await?;
        Ok(Self::check_status(resp).await?.json().await?)
    }

    pub async fn list_pr_review_comments(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<Vec<GiteaPrReviewComment>, GiteaClientError> {
        let resp = self.http
            .get(self.api(&format!("/repos/{}/{}/pulls/{}/reviews/comments", owner, repo, pr_number)))
            .bearer_auth(&self.token)
            .send().await?;
        Ok(Self::check_status(resp).await?.json().await?)
    }

    pub async fn reply_to_pr_review_comment(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
        comment_id: u64,
        body: &str,
    ) -> Result<GiteaComment, GiteaClientError> {
        #[derive(Serialize)]
        struct ReplyBody<'a> {
            body: &'a str,
        }
        let resp = self.http
            .post(self.api(&format!(
                "/repos/{}/{}/pulls/{}/reviews/comments/{}/replies",
                owner, repo, pr_number, comment_id
            )))
            .bearer_auth(&self.token)
            .json(&ReplyBody { body })
            .send().await?;
        Ok(Self::check_status(resp).await?.json().await?)
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p foundry-mcp-gitea client 2>&1
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add foundry-mcp-gitea/src/client.rs
git commit -m "feat(mcp-gitea): add GiteaClient with HTTP methods and error handling"
```

---

### Task 3: Implement MCP tool handlers

**Files:**
- Modify: `foundry-mcp-gitea/src/tools/issues.rs`
- Modify: `foundry-mcp-gitea/src/tools/pull_requests.rs`
- Modify: `foundry-mcp-gitea/src/tools/repository.rs`
- Modify: `foundry-mcp-gitea/src/tools/mod.rs`

Note: `rmcp` tool handlers use the `#[tool]` proc-macro. Each tool function takes typed input via a derive-deserialized struct and returns a `Result<CallToolResult, McpError>`. Refer to `rmcp` docs for exact API — adapt if the macro API differs.

- [ ] **Step 1: Implement `tools/issues.rs`**

```rust
// foundry-mcp-gitea/src/tools/issues.rs
use crate::client::GiteaClient;
use rmcp::{tool, CallToolResult, Content, McpError};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct GetIssueInput {
    pub owner: String,
    pub repo: String,
    pub issue_number: u64,
}

pub async fn get_issue(
    client: Arc<GiteaClient>,
    input: GetIssueInput,
) -> Result<CallToolResult, McpError> {
    let issue = client
        .get_issue(&input.owner, &input.repo, input.issue_number)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    let text = serde_json::to_string_pretty(&issue)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

#[derive(Deserialize)]
pub struct ListIssueCommentsInput {
    pub owner: String,
    pub repo: String,
    pub issue_number: u64,
    pub since: Option<String>,
}

pub async fn list_issue_comments(
    client: Arc<GiteaClient>,
    input: ListIssueCommentsInput,
) -> Result<CallToolResult, McpError> {
    let comments = client
        .list_issue_comments(
            &input.owner,
            &input.repo,
            input.issue_number,
            input.since.as_deref(),
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    let text = serde_json::to_string_pretty(&comments)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

#[derive(Deserialize)]
pub struct CreateIssueCommentInput {
    pub owner: String,
    pub repo: String,
    pub issue_number: u64,
    pub body: String,
}

pub async fn create_issue_comment(
    client: Arc<GiteaClient>,
    input: CreateIssueCommentInput,
) -> Result<CallToolResult, McpError> {
    let comment = client
        .create_issue_comment(&input.owner, &input.repo, input.issue_number, &input.body)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(format!(
        "Comment created with id {}",
        comment.id
    ))]))
}

#[derive(Deserialize)]
pub struct EditIssueInput {
    pub owner: String,
    pub repo: String,
    pub issue_number: u64,
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: Option<String>,
}

pub async fn edit_issue(
    client: Arc<GiteaClient>,
    input: EditIssueInput,
) -> Result<CallToolResult, McpError> {
    let issue = client
        .edit_issue(
            &input.owner,
            &input.repo,
            input.issue_number,
            input.title.as_deref(),
            input.body.as_deref(),
            input.state.as_deref(),
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    let text = serde_json::to_string_pretty(&issue)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(text)]))
}
```

- [ ] **Step 2: Implement `tools/pull_requests.rs`**

```rust
// foundry-mcp-gitea/src/tools/pull_requests.rs
use crate::client::{CreatePrBody, GiteaClient};
use rmcp::{CallToolResult, Content, McpError};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct CreatePullRequestInput {
    pub owner: String,
    pub repo: String,
    pub title: String,
    pub body: String,
    pub head: String,
    pub base: String,
}

pub async fn create_pull_request(
    client: Arc<GiteaClient>,
    input: CreatePullRequestInput,
) -> Result<CallToolResult, McpError> {
    let pr = client
        .create_pull_request(
            &input.owner,
            &input.repo,
            &CreatePrBody {
                title: &input.title,
                body: &input.body,
                head: &input.head,
                base: &input.base,
            },
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    let text = serde_json::to_string_pretty(&pr)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

#[derive(Deserialize)]
pub struct GetPullRequestInput {
    pub owner: String,
    pub repo: String,
    pub pr_number: u64,
}

pub async fn get_pull_request(
    client: Arc<GiteaClient>,
    input: GetPullRequestInput,
) -> Result<CallToolResult, McpError> {
    let pr = client
        .get_pull_request(&input.owner, &input.repo, input.pr_number)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    let text = serde_json::to_string_pretty(&pr)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

#[derive(Deserialize)]
pub struct ListPrReviewsInput {
    pub owner: String,
    pub repo: String,
    pub pr_number: u64,
}

pub async fn list_pr_reviews(
    client: Arc<GiteaClient>,
    input: ListPrReviewsInput,
) -> Result<CallToolResult, McpError> {
    let reviews = client
        .list_pr_reviews(&input.owner, &input.repo, input.pr_number)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    let text = serde_json::to_string_pretty(&reviews)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

#[derive(Deserialize)]
pub struct CreatePrCommentInput {
    pub owner: String,
    pub repo: String,
    pub pr_number: u64,
    pub body: String,
}

pub async fn create_pr_comment(
    client: Arc<GiteaClient>,
    input: CreatePrCommentInput,
) -> Result<CallToolResult, McpError> {
    let comment = client
        .create_pr_comment(&input.owner, &input.repo, input.pr_number, &input.body)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(format!(
        "Comment created with id {}",
        comment.id
    ))]))
}

#[derive(Deserialize)]
pub struct UpdatePullRequestInput {
    pub owner: String,
    pub repo: String,
    pub pr_number: u64,
    pub title: Option<String>,
    pub body: Option<String>,
}

pub async fn update_pull_request(
    client: Arc<GiteaClient>,
    input: UpdatePullRequestInput,
) -> Result<CallToolResult, McpError> {
    let pr = client
        .update_pull_request(
            &input.owner,
            &input.repo,
            input.pr_number,
            input.title.as_deref(),
            input.body.as_deref(),
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    let text = serde_json::to_string_pretty(&pr)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

#[derive(Deserialize)]
pub struct ListPrReviewCommentsInput {
    pub owner: String,
    pub repo: String,
    pub pr_number: u64,
}

pub async fn list_pr_review_comments(
    client: Arc<GiteaClient>,
    input: ListPrReviewCommentsInput,
) -> Result<CallToolResult, McpError> {
    let comments = client
        .list_pr_review_comments(&input.owner, &input.repo, input.pr_number)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    let text = serde_json::to_string_pretty(&comments)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

#[derive(Deserialize)]
pub struct ReplyToPrCommentInput {
    pub owner: String,
    pub repo: String,
    pub pr_number: u64,
    pub comment_id: u64,
    pub body: String,
}

pub async fn reply_to_pr_comment(
    client: Arc<GiteaClient>,
    input: ReplyToPrCommentInput,
) -> Result<CallToolResult, McpError> {
    let comment = client
        .reply_to_pr_review_comment(
            &input.owner,
            &input.repo,
            input.pr_number,
            input.comment_id,
            &input.body,
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(format!(
        "Reply posted with id {}",
        comment.id
    ))]))
}
```

- [ ] **Step 3: Implement `tools/repository.rs`**

```rust
// foundry-mcp-gitea/src/tools/repository.rs
use crate::client::GiteaClient;
use rmcp::{CallToolResult, Content, McpError};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct GetRepoInput {
    pub owner: String,
    pub repo: String,
}

pub async fn get_repo(
    client: Arc<GiteaClient>,
    input: GetRepoInput,
) -> Result<CallToolResult, McpError> {
    let repo = client
        .get_repo(&input.owner, &input.repo)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    let text = serde_json::to_string_pretty(&repo)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

pub async fn get_authenticated_user(
    client: Arc<GiteaClient>,
) -> Result<CallToolResult, McpError> {
    let user = client
        .get_authenticated_user()
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    let text = serde_json::to_string_pretty(&user)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(text)]))
}
```

- [ ] **Step 4: Commit**

```bash
git add foundry-mcp-gitea/src/tools/
git commit -m "feat(mcp-gitea): add tool handlers for issues, PRs, and repository"
```

---

### Task 4: Wire the MCP server in `main.rs`

**Files:**
- Modify: `foundry-mcp-gitea/src/main.rs`

- [ ] **Step 1: Implement `main.rs`**

```rust
// foundry-mcp-gitea/src/main.rs
mod client;
mod tools;

use client::GiteaClient;
use rmcp::{ServerHandler, ServiceExt, transport::stdio};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr) // MCP uses stdio; logs must go to stderr
        .init();

    let gitea_url = std::env::var("GITEA_URL")
        .expect("GITEA_URL environment variable is required");
    let gitea_token = std::env::var("GITEA_TOKEN")
        .expect("GITEA_TOKEN environment variable is required");

    let client = Arc::new(GiteaClient::new(gitea_url, gitea_token));

    let service = FoundryGiteaServer { client }
        .serve(stdio())
        .await?;

    service.waiting().await?;
    Ok(())
}

#[derive(Clone)]
struct FoundryGiteaServer {
    client: Arc<GiteaClient>,
}

// Tool registration — consult rmcp docs for the exact ServerHandler impl pattern.
// The pattern is typically: implement list_tools() returning tool metadata,
// and call_tool() dispatching to handlers by name.
#[rmcp::tool_router]
impl FoundryGiteaServer {
    #[tool(description = "Get a single issue by number")]
    async fn get_issue(&self, #[tool(param)] input: tools::issues::GetIssueInput)
        -> rmcp::Result<rmcp::CallToolResult>
    {
        tools::issues::get_issue(self.client.clone(), input).await
            .map_err(Into::into)
    }

    #[tool(description = "List comments on an issue")]
    async fn list_issue_comments(
        &self,
        #[tool(param)] input: tools::issues::ListIssueCommentsInput,
    ) -> rmcp::Result<rmcp::CallToolResult> {
        tools::issues::list_issue_comments(self.client.clone(), input).await
            .map_err(Into::into)
    }

    #[tool(description = "Post a comment on an issue")]
    async fn create_issue_comment(
        &self,
        #[tool(param)] input: tools::issues::CreateIssueCommentInput,
    ) -> rmcp::Result<rmcp::CallToolResult> {
        tools::issues::create_issue_comment(self.client.clone(), input).await
            .map_err(Into::into)
    }

    #[tool(description = "Open a new pull request")]
    async fn create_pull_request(
        &self,
        #[tool(param)] input: tools::pull_requests::CreatePullRequestInput,
    ) -> rmcp::Result<rmcp::CallToolResult> {
        tools::pull_requests::create_pull_request(self.client.clone(), input).await
            .map_err(Into::into)
    }

    #[tool(description = "Get a pull request by number")]
    async fn get_pull_request(
        &self,
        #[tool(param)] input: tools::pull_requests::GetPullRequestInput,
    ) -> rmcp::Result<rmcp::CallToolResult> {
        tools::pull_requests::get_pull_request(self.client.clone(), input).await
            .map_err(Into::into)
    }

    #[tool(description = "List all reviews on a pull request")]
    async fn list_pr_reviews(
        &self,
        #[tool(param)] input: tools::pull_requests::ListPrReviewsInput,
    ) -> rmcp::Result<rmcp::CallToolResult> {
        tools::pull_requests::list_pr_reviews(self.client.clone(), input).await
            .map_err(Into::into)
    }

    #[tool(description = "Post a general comment on a pull request")]
    async fn create_pr_comment(
        &self,
        #[tool(param)] input: tools::pull_requests::CreatePrCommentInput,
    ) -> rmcp::Result<rmcp::CallToolResult> {
        tools::pull_requests::create_pr_comment(self.client.clone(), input).await
            .map_err(Into::into)
    }

    #[tool(description = "Get repository metadata including default branch")]
    async fn get_repo(
        &self,
        #[tool(param)] input: tools::repository::GetRepoInput,
    ) -> rmcp::Result<rmcp::CallToolResult> {
        tools::repository::get_repo(self.client.clone(), input).await
            .map_err(Into::into)
    }

    #[tool(description = "Get the authenticated bot user's username")]
    async fn get_authenticated_user(&self) -> rmcp::Result<rmcp::CallToolResult> {
        tools::repository::get_authenticated_user(self.client.clone()).await
            .map_err(Into::into)
    }

    #[tool(description = "Edit an issue's title, body, or state")]
    async fn edit_issue(
        &self,
        #[tool(param)] input: tools::issues::EditIssueInput,
    ) -> rmcp::Result<rmcp::CallToolResult> {
        tools::issues::edit_issue(self.client.clone(), input).await
            .map_err(Into::into)
    }

    #[tool(description = "Update a pull request's title or body")]
    async fn update_pull_request(
        &self,
        #[tool(param)] input: tools::pull_requests::UpdatePullRequestInput,
    ) -> rmcp::Result<rmcp::CallToolResult> {
        tools::pull_requests::update_pull_request(self.client.clone(), input).await
            .map_err(Into::into)
    }

    #[tool(description = "List inline review comments on a pull request")]
    async fn list_pr_review_comments(
        &self,
        #[tool(param)] input: tools::pull_requests::ListPrReviewCommentsInput,
    ) -> rmcp::Result<rmcp::CallToolResult> {
        tools::pull_requests::list_pr_review_comments(self.client.clone(), input).await
            .map_err(Into::into)
    }

    #[tool(description = "Reply to an inline review comment on a pull request")]
    async fn reply_to_pr_comment(
        &self,
        #[tool(param)] input: tools::pull_requests::ReplyToPrCommentInput,
    ) -> rmcp::Result<rmcp::CallToolResult> {
        tools::pull_requests::reply_to_pr_comment(self.client.clone(), input).await
            .map_err(Into::into)
    }
}
```

> **Note for implementer:** The `rmcp` proc-macro API (`#[tool_router]`, `#[tool]`) may differ from the above. Check the installed `rmcp` version's examples at `https://github.com/modelcontextprotocol/rust-sdk`. Adapt the registration pattern to match — the tool handler functions themselves do not need to change.

- [ ] **Step 2: Build the binary**

```bash
cargo build -p foundry-mcp-gitea 2>&1
```

Expected: compiles cleanly.

- [ ] **Step 3: Smoke test: binary prints help without crashing**

```bash
./target/debug/foundry-mcp-gitea --help 2>&1 || true
# Or: check the binary exists
ls -lh target/debug/foundry-mcp-gitea
```

Expected: binary exists, is executable.

- [ ] **Step 4: Commit**

```bash
git add foundry-mcp-gitea/src/main.rs
git commit -m "feat(mcp-gitea): wire MCP server with all tool registrations"
```

---

### Task 5: Run all tests

- [ ] **Step 1: Run all tests**

```bash
cargo test -p foundry-mcp-gitea 2>&1
```

Expected: all tests pass.

- [ ] **Step 2: Build in release mode**

```bash
cargo build --release -p foundry-mcp-gitea 2>&1
```

Expected: release binary at `target/release/foundry-mcp-gitea`.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "chore(mcp-gitea): verify all tests pass and release build succeeds"
```
