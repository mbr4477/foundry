# InReview Session Reconstruction via PR Lookup

**Date:** 2026-03-23
**Status:** Approved

## Problem

On daemon restart, `foundryd` reconstructs in-memory sessions from Gitea. The reconstruction logic in `main.rs` sets a session to `InReview` when `issue.pr_number` is `Some`. However, `GiteaCodeHost::list_assigned_issues` now always returns `pr_number: None` because Gitea's `/api/v1/repos/issues/search` API does not link issues to their associated PRs. As a result, issues with an open PR are incorrectly reconstructed as `Implementing` or `Planning` after a restart.

## Solution

Introduce a `list_open_prs` method on the `CodeHost` trait. During reconstruction, group assigned issues by repo, call `list_open_prs` once per repo, and match PRs to issues by head branch name using the shared `IssueKey::branch_name()` method.

## Design

### 1. `IssueKey::branch_name()`

Add a `branch_name()` method to `IssueKey` in `foundry-core/src/types.rs`, parallel to the existing `volume_name()`:

```rust
pub fn branch_name(&self) -> String {
    format!("foundry/issue-{}", self.issue_number)
}
```

Add a unit test asserting `key.branch_name() == "foundry/issue-42"` for `issue_number: 42`, alongside the existing `volume_name` test.

Update `dispatcher.rs` to call `key.branch_name()` instead of the inline `format!`.

### 2. Remove `HostIssue.pr_number`

`HostIssue.pr_number` in `foundry-core/src/traits/code_host.rs` is now vestigial — `GiteaCodeHost` always returns `None` and the reconstruction logic no longer reads it. Remove the field from `HostIssue` and its construction site in `gitea.rs`.

### 3. `HostPr` struct

Add to `foundry-core/src/traits/code_host.rs` alongside `HostIssue`, `HostComment`, `HostReview`:

```rust
pub struct HostPr {
    pub number: u64,
    pub head_branch: String,
}
```

### 4. `CodeHost` trait method

Add to the `CodeHost` trait:

```rust
async fn list_open_prs(
    &self,
    owner: &str,
    repo: &str,
) -> Result<Vec<HostPr>, CodeHostError>;
```

### 5. `GiteaCodeHost` implementation

Call `GET /api/v1/repos/{owner}/{repo}/pulls?state=open&limit=50` with pagination: loop incrementing `page=1,2,...` (starting at `page=1`) until an empty page is returned, accumulating results. Add internal deserialization structs:

```rust
struct GiteaPrRaw {
    number: u64,
    head: GiteaHeadRef,
}

struct GiteaHeadRef {
    #[serde(rename = "ref")]
    ref_: String,
}
```

Map to `HostPr { number, head_branch: head.ref_ }`.

Add a mockito unit test verifying the happy path. The test response body must use the nested `head` object shape that matches the Gitea API:

```json
[{"number": 7, "head": {"ref": "foundry/issue-7", "label": "user:foundry/issue-7", "sha": "abc123"}}]
```

Assert that `head_branch == "foundry/issue-7"`.

### 6. Reconstruction logic in `main.rs`

Replace the current `issue.pr_number` check with:

1. Group all assigned issues by `(owner, repo)`.
2. For each unique repo, call `code_host.list_open_prs(owner, repo)`. If the call fails, log a warning and treat all issues in that repo as having no matched PR — do not abort reconstruction for other repos.
3. Build a `HashMap<String, u64>` mapping `head_branch → pr_number`.
4. For each issue, look up `key.branch_name()` in the map:
   - Found → `phase = InReview`, `pr_number = Some(matched_pr)`
   - Not found + `has_approve` → `phase = Implementing`
   - Otherwise → `phase = Planning`
5. The existing `InReview` path (check for unaddressed reviews, fire `PollRecovery`) is unchanged.

## Files Changed

| File | Change |
|------|--------|
| `foundry-core/src/types.rs` | Add `IssueKey::branch_name()` |
| `foundry-core/src/traits/code_host.rs` | Add `HostPr`, `list_open_prs` to trait |
| `foundryd/src/code_host/gitea.rs` | Implement `list_open_prs`, add test |
| `foundryd/src/dispatcher.rs` | Use `key.branch_name()` |
| `foundryd/src/main.rs` | Group by repo, call `list_open_prs`, match by branch |
