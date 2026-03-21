use async_trait::async_trait;
use chrono::{DateTime, Utc};
use foundry_core::{
    errors::EventSourceError,
    events::Event,
    traits::{code_host::CodeHost, event_source::EventSource},
    types::RepoId,
};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

pub struct PollingSource {
    host: Arc<dyn CodeHost>,
    interval: Duration,
    since: Arc<Mutex<Option<DateTime<Utc>>>>,
}

impl PollingSource {
    pub fn new(
        host: Arc<dyn CodeHost>,
        interval: Duration,
        since: Arc<Mutex<Option<DateTime<Utc>>>>,
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
        let mut interval = tokio::time::interval(self.interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let since = {
                        let guard = self.since.lock().await;
                        *guard
                    };

                    match self.host.list_assigned_issues(since).await {
                        Ok(issues) => {
                            let mut seen: HashSet<String> = HashSet::new();
                            for issue in issues {
                                let key_str = format!(
                                    "{}/{}/{}",
                                    issue.key.owner, issue.key.repo, issue.key.issue_number
                                );
                                if seen.contains(&key_str) {
                                    continue;
                                }
                                seen.insert(key_str);

                                let event = Event::PollRecovery {
                                    repo: RepoId {
                                        owner: issue.key.owner.clone(),
                                        repo: issue.key.repo.clone(),
                                    },
                                    issue_number: issue.key.issue_number,
                                    timestamp: Utc::now(),
                                };

                                if tx.send(event).await.is_err() {
                                    return Err(EventSourceError::ChannelClosed);
                                }
                            }
                            // Seen set intentionally cleared each cycle
                            // so issues are re-emitted for crash recovery
                        }
                        Err(e) => {
                            warn!("Polling error: {}", e);
                        }
                    }
                }
                _ = cancel.cancelled() => {
                    debug!("PollingSource cancelled");
                    break;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use foundry_core::{
        errors::CodeHostError,
        traits::code_host::{CodeHost, HostComment, HostIssue, HostReview},
        types::IssueKey,
    };

    struct MockCodeHost {
        issues: Vec<HostIssue>,
    }

    #[async_trait]
    impl CodeHost for MockCodeHost {
        async fn list_assigned_issues(
            &self,
            _since: Option<DateTime<Utc>>,
        ) -> Result<Vec<HostIssue>, CodeHostError> {
            Ok(self.issues.clone())
        }

        async fn list_issue_comments(
            &self,
            _key: &IssueKey,
            _since: Option<DateTime<Utc>>,
        ) -> Result<Vec<HostComment>, CodeHostError> {
            Ok(vec![])
        }

        async fn list_pr_reviews(
            &self,
            _owner: &str,
            _repo: &str,
            _pr_number: u64,
        ) -> Result<Vec<HostReview>, CodeHostError> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn poll_emits_recovery_events_for_assigned_issues() {
        let host = Arc::new(MockCodeHost {
            issues: vec![HostIssue {
                key: IssueKey {
                    owner: "alice".into(),
                    repo: "proj".into(),
                    issue_number: 1,
                },
                pr_number: None,
            }],
        });

        let since = Arc::new(Mutex::new(None));
        let polling = PollingSource::new(
            host,
            Duration::from_millis(10),
            since,
        );

        let (tx, mut rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            polling.run(tx, cancel_clone).await.ok();
        });

        // Wait for at least one event
        let event = tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("Timed out waiting for event")
            .expect("Channel closed");

        cancel.cancel();

        match event {
            Event::PollRecovery { issue_number, .. } => {
                assert_eq!(issue_number, 1);
            }
            _ => panic!("Expected PollRecovery event, got {:?}", event),
        }
    }
}
