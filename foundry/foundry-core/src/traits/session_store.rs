use crate::errors::SessionStoreError;
use crate::types::{IssueKey, IssueSession};
use async_trait::async_trait;

#[async_trait]
pub trait SessionStore: Send + Sync + 'static {
    async fn upsert(&self, session: &IssueSession) -> Result<(), SessionStoreError>;
    async fn get(&self, key: &IssueKey) -> Result<Option<IssueSession>, SessionStoreError>;
    async fn get_by_pr(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<Option<IssueSession>, SessionStoreError>;
    async fn list(&self) -> Result<Vec<IssueSession>, SessionStoreError>;
    async fn delete(&self, key: &IssueKey) -> Result<(), SessionStoreError>;
}
