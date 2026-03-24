# InReview Session Reconstruction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix startup session reconstruction so issues with an open PR are correctly identified as `InReview` by querying Gitea's PR API.

**Architecture:** Add `IssueKey::branch_name()` as the canonical branch-name source, add `list_open_prs` to the `CodeHost` trait, implement it in `GiteaCodeHost` with a paginated fetch, then rewrite the `main.rs` reconstruction block to group issues by repo, look up open PRs once per repo, and match by branch name.

**Tech Stack:** Rust, async-trait, serde/serde_json, reqwest, mockito (tests), tokio

---

## File Map

| File | Change |
|------|--------|
| `foundry/foundry-core/src/types.rs` | Add `IssueKey::branch_name()` + unit test |
| `foundry/foundry-core/src/traits/code_host.rs` | Remove `HostIssue.pr_number`; add `HostPr` struct + `list_open_prs` to trait |
| `foundry/foundryd/src/code_host/gitea.rs` | Remove vestigial `pr_number: None`; add `GiteaPrRaw`/`GiteaHeadRef` structs + `list_open_prs` impl + test |
| `foundry/foundryd/src/dispatcher.rs` | Replace inline `format!` with `key.branch_name()` |
| `foundry/foundryd/src/main.rs` | Rewrite reconstruction block to group by repo, call `list_open_prs`, match by branch |

---

## Task 1: Add `IssueKey::branch_name()`

**Files:**
- Modify: `foundry/foundry-core/src/types.rs`

- [ ] **Step 1: Write the failing test**

In `foundry/foundry-core/src/types.rs`, inside the existing `#[cfg(test)] mod tests` block (after the `volume_name` tests around line 86), add:

```rust
#[test]
fn branch_name_uses_issue_number() {
    let key = IssueKey { owner: "alice".into(), repo: "proj".into(), issue_number: 42 };
    assert_eq!(key.branch_name(), "foundry/issue-42");
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd foundry && cargo test -p foundry-core branch_name_uses_issue_number
```

Expected: compile error — `branch_name` does not exist yet.

- [ ] **Step 3: Implement `branch_name()`**

In `foundry/foundry-core/src/types.rs`, inside the `impl IssueKey` block (after `volume_name`, around line 22), add:

```rust
/// Derives the Git branch name for this issue.
pub fn branch_name(&self) -> String {
    format!("foundry/issue-{}", self.issue_number)
}
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cd foundry && cargo test -p foundry-core branch_name_uses_issue_number
```

Expected: `test types::tests::branch_name_uses_issue_number ... ok`

- [ ] **Step 5: Commit**

```bash
cd foundry && git add foundry-core/src/types.rs
git commit -m "feat: add IssueKey::branch_name()"
```

---

## Task 2: Update `dispatcher.rs` to use `branch_name()`

**Files:**
- Modify: `foundry/foundryd/src/dispatcher.rs:373`

- [ ] **Step 1: Replace the inline format**

In `foundry/foundryd/src/dispatcher.rs` at line 373, replace:

```rust
let branch_name = Some(format!("foundry/issue-{}", key.issue_number));
```

with:

```rust
let branch_name = Some(key.branch_name());
```

- [ ] **Step 2: Run tests to verify nothing broke**

```bash
cd foundry && cargo test -p foundryd
```

Expected: all existing tests pass.

- [ ] **Step 3: Commit**

```bash
cd foundry && git add foundryd/src/dispatcher.rs
git commit -m "refactor: use IssueKey::branch_name() in dispatcher"
```

---

## Task 3: Remove `HostIssue.pr_number` and add `HostPr` + `list_open_prs` to the trait

**Files:**
- Modify: `foundry/foundry-core/src/traits/code_host.rs`

- [ ] **Step 1: Remove `pr_number` from `HostIssue`, add `HostPr` + `list_open_prs`, and fix the `gitea.rs` call site**

This step touches two files so that the commit compiles cleanly.

**`foundry/foundry-core/src/traits/code_host.rs`** — replace the entire file with:

```rust
use crate::errors::CodeHostError;
use crate::types::{IssueKey, ReviewState};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostIssue {
    pub key: IssueKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostComment {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostReview {
    pub id: u64,
    pub reviewer: String,
    pub state: ReviewState,
    pub submitted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostPr {
    pub number: u64,
    pub head_branch: String,
}

#[async_trait]
pub trait CodeHost: Send + Sync + 'static {
    async fn list_assigned_issues(
        &self,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<HostIssue>, CodeHostError>;
    async fn list_issue_comments(
        &self,
        key: &IssueKey,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<HostComment>, CodeHostError>;
    async fn list_pr_reviews(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<Vec<HostReview>, CodeHostError>;
    async fn list_open_prs(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<Vec<HostPr>, CodeHostError>;
}
```

**`foundry/foundryd/src/code_host/gitea.rs`** — in the `list_assigned_issues` `filter_map` closure (around line 117), update `HostIssue` construction to remove the now-deleted `pr_number` field:

```rust
Some(HostIssue {
    key: IssueKey {
        owner: repo.owner,
        repo: repo.name,
        issue_number: issue.number,
    },
})
```

Also unconditionally delete the `GiteaPullRequestRef` struct (currently lines 49–53 with `#[allow(dead_code)]`) — it is present in the codebase and unused.

- [ ] **Step 2: Check that the compiler guides you to remaining call sites**

```bash
cd foundry && cargo build 2>&1 | head -40
```

Expected: compile errors only in `main.rs` (`list_open_prs` not yet implemented on the trait, `issue.pr_number` still referenced). `gitea.rs` should compile cleanly at this point.

- [ ] **Step 3: Commit the trait change and gitea.rs fix together**

```bash
cd foundry && git add foundry-core/src/traits/code_host.rs foundryd/src/code_host/gitea.rs
git commit -m "feat: add HostPr and list_open_prs to CodeHost trait; remove vestigial HostIssue.pr_number"
```

---

## Task 4: Implement `list_open_prs` in `GiteaCodeHost`

**Files:**
- Modify: `foundry/foundryd/src/code_host/gitea.rs`

- [ ] **Step 1: Write the failing test**

In `foundry/foundryd/src/code_host/gitea.rs`, inside the existing `#[cfg(test)] mod tests` block, add:

```rust
#[tokio::test]
async fn list_open_prs_parses_response() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/api/v1/repos/alice/proj/pulls?state=open&limit=50&page=1")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"[{"number": 7, "head": {"ref": "foundry/issue-7", "label": "user:foundry/issue-7", "sha": "abc123"}}]"#,
        )
        .create_async()
        .await;
    // Second page — empty, terminates the loop
    let mock2 = server
        .mock("GET", "/api/v1/repos/alice/proj/pulls?state=open&limit=50&page=2")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create_async()
        .await;

    let host = GiteaCodeHost::new(server.url(), "test".into(), "foundry-bot".into());
    let prs = host.list_open_prs("alice", "proj").await.unwrap();
    assert_eq!(prs.len(), 1);
    assert_eq!(prs[0].number, 7);
    assert_eq!(prs[0].head_branch, "foundry/issue-7");
    mock.assert_async().await;
    mock2.assert_async().await;
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd foundry && cargo test -p foundryd list_open_prs_parses_response
```

Expected: compile error — `list_open_prs` not yet implemented.

- [ ] **Step 3: Update the `gitea.rs` import to include `HostPr`**

At the top of `foundry/foundryd/src/code_host/gitea.rs`, update the import line from:

```rust
use foundry_core::{
    errors::CodeHostError,
    traits::code_host::{CodeHost, HostComment, HostIssue, HostReview},
    types::{IssueKey, ReviewState},
};
```

to:

```rust
use foundry_core::{
    errors::CodeHostError,
    traits::code_host::{CodeHost, HostComment, HostIssue, HostPr, HostReview},
    types::{IssueKey, ReviewState},
};
```

- [ ] **Step 4: Add the deserialization structs and implement `list_open_prs`**

In `gitea.rs`, add after the existing raw structs (before the `impl CodeHost`):

```rust
#[derive(Debug, Deserialize)]
struct GiteaPrRaw {
    number: u64,
    head: GiteaHeadRef,
}

#[derive(Debug, Deserialize)]
struct GiteaHeadRef {
    #[serde(rename = "ref")]
    ref_: String,
}
```

Then inside `impl CodeHost for GiteaCodeHost`, add:

```rust
async fn list_open_prs(
    &self,
    owner: &str,
    repo: &str,
) -> Result<Vec<HostPr>, CodeHostError> {
    let mut all = Vec::new();
    let mut page = 1u32;
    loop {
        let url = format!("{}/api/v1/repos/{}/{}/pulls", self.base_url, owner, repo);
        let resp = self
            .client
            .get(&url)
            .query(&[
                ("state", "open"),
                ("limit", "50"),
                ("page", &page.to_string()),
            ])
            .header("Authorization", format!("token {}", self.token))
            .send()
            .await
            .map_err(|e| CodeHostError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(CodeHostError::Http(format!("HTTP {}", resp.status())));
        }

        let prs: Vec<GiteaPrRaw> = resp
            .json()
            .await
            .map_err(|e| CodeHostError::UnexpectedResponse(e.to_string()))?;

        if prs.is_empty() {
            break;
        }

        all.extend(prs.into_iter().map(|p| HostPr {
            number: p.number,
            head_branch: p.head.ref_,
        }));
        page += 1;
    }
    Ok(all)
}
```

- [ ] **Step 5: Run test to verify it passes**

```bash
cd foundry && cargo test -p foundryd list_open_prs_parses_response
```

Expected: `test code_host::gitea::tests::list_open_prs_parses_response ... ok`

- [ ] **Step 6: Run all foundryd tests**

```bash
cd foundry && cargo test -p foundryd
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
cd foundry && git add foundryd/src/code_host/gitea.rs
git commit -m "feat: implement list_open_prs in GiteaCodeHost"
```

---

## Task 5: Rewrite `main.rs` reconstruction to use `list_open_prs`

**Files:**
- Modify: `foundry/foundryd/src/main.rs:89-149` (the reconstruction block)

- [ ] **Step 1: Add `IssueKey` to the reconstruction block's `use` statement**

Inside `main.rs`, the reconstruction block (around line 80) has:

```rust
use foundry_core::{
    traits::code_host::CodeHost,
    types::{IssuePhase, IssueSession},
};
```

Update it to:

```rust
use foundry_core::{
    traits::code_host::CodeHost,
    types::{IssueKey, IssuePhase, IssueSession},
};
```

- [ ] **Step 2: Rewrite the reconstruction block**

Replace the entire `for issue in issues { ... }` loop (currently lines ~93–149 in `main.rs`) with the following. The surrounding `match code_host.list_assigned_issues(None).await { Ok(issues) => { ... }` wrapper stays unchanged.

Note: `pr_map` uses a `(owner, repo, branch)` three-tuple key rather than a plain `String` to avoid false matches when two repos happen to have identically named branches.

```rust
Ok(issues) => {
    debug!("{:?}", issues);

    // Group issues by (owner, repo) so we call list_open_prs once per repo
    let mut by_repo: std::collections::HashMap<(String, String), Vec<IssueKey>> =
        std::collections::HashMap::new();
    for issue in &issues {
        by_repo
            .entry((issue.key.owner.clone(), issue.key.repo.clone()))
            .or_default()
            .push(issue.key.clone());
    }

    // Fetch open PRs for each repo; build branch -> pr_number map
    let mut pr_map: std::collections::HashMap<(String, String, String), u64> =
        std::collections::HashMap::new();
    for ((owner, repo), _) in &by_repo {
        match code_host.list_open_prs(owner, repo).await {
            Ok(prs) => {
                for pr in prs {
                    pr_map.insert((owner.clone(), repo.clone(), pr.head_branch), pr.number);
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to list open PRs for {}/{}: {} — treating as no open PRs",
                    owner, repo, e
                );
            }
        }
    }

    // Reconstruct each session
    for issue in issues {
        let key = issue.key.clone();
        if store.get(&key).await.ok().flatten().is_some() {
            continue;
        }

        let comments = code_host
            .list_issue_comments(&key, None)
            .await
            .unwrap_or_default();
        let has_approve = comments
            .iter()
            .any(|c| c.body.trim().starts_with(&cfg.commands.approve));

        let lookup = (key.owner.clone(), key.repo.clone(), key.branch_name());
        let matched_pr = pr_map.get(&lookup).copied();

        let (phase, pr_number) = if let Some(pr) = matched_pr {
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
        info!(
            "Reconstructed session for {}/{}/{} as {:?}",
            key.owner, key.repo, key.issue_number, session.phase
        );

        if phase == IssuePhase::InReview {
            if let Some(pr) = pr_number {
                let reviews = code_host
                    .list_pr_reviews(&key.owner, &key.repo, pr)
                    .await
                    .unwrap_or_default();
                let has_unaddressed = reviews.iter().any(|r| {
                    matches!(
                        r.state,
                        foundry_core::types::ReviewState::ChangesRequested
                            | foundry_core::types::ReviewState::Comment
                    )
                });
                if has_unaddressed {
                    dispatcher
                        .handle_event(foundry_core::events::Event::PollRecovery {
                            repo: foundry_core::types::RepoId {
                                owner: key.owner.clone(),
                                repo: key.repo.clone(),
                            },
                            issue_number: key.issue_number,
                            timestamp: chrono::Utc::now(),
                        })
                        .await
                        .ok();
                }
            }
        }
    }
}
```

- [ ] **Step 3: Build to verify it compiles**

```bash
cd foundry && cargo build -p foundryd
```

Expected: clean build, no errors or warnings about unused `pr_number` or missing `list_open_prs`.

- [ ] **Step 4: Run all tests**

```bash
cd foundry && cargo test
```

Expected: all tests pass across all crates.

- [ ] **Step 5: Commit**

```bash
cd foundry && git add foundryd/src/main.rs
git commit -m "fix: reconstruct InReview sessions by matching open PRs via branch name"
```

---

## Final Check

- [ ] **Run the full test suite one more time**

```bash
cd foundry && cargo test
```

Expected: all tests pass.

- [ ] **Verify no warnings**

```bash
cd foundry && cargo build 2>&1 | grep -i warning
```

Expected: no new warnings introduced by this change.
