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
