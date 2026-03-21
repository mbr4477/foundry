use async_trait::async_trait;
use foundry_core::{
    errors::SessionStoreError,
    traits::session_store::SessionStore,
    types::{IssueKey, IssueSession},
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct MemorySessionStore {
    sessions: Arc<RwLock<HashMap<IssueKey, IssueSession>>>,
}

impl MemorySessionStore {
    pub fn new() -> Self {
        Self { sessions: Arc::new(RwLock::new(HashMap::new())) }
    }
}

impl Default for MemorySessionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SessionStore for MemorySessionStore {
    async fn upsert(&self, session: &IssueSession) -> Result<(), SessionStoreError> {
        let mut map = self.sessions.write().await;
        map.insert(session.key.clone(), session.clone());
        Ok(())
    }

    async fn get(&self, key: &IssueKey) -> Result<Option<IssueSession>, SessionStoreError> {
        let map = self.sessions.read().await;
        Ok(map.get(key).cloned())
    }

    async fn get_by_pr(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<Option<IssueSession>, SessionStoreError> {
        let map = self.sessions.read().await;
        let found = map.values().find(|s| {
            s.key.owner == owner
                && s.key.repo == repo
                && s.pr_number == Some(pr_number)
        });
        Ok(found.cloned())
    }

    async fn list(&self) -> Result<Vec<IssueSession>, SessionStoreError> {
        let map = self.sessions.read().await;
        Ok(map.values().cloned().collect())
    }

    async fn delete(&self, key: &IssueKey) -> Result<(), SessionStoreError> {
        let mut map = self.sessions.write().await;
        map.remove(key);
        Ok(())
    }
}

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
