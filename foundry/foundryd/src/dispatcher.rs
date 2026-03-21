use crate::{
    config::Config,
    container::docker::FOUNDRY_ISSUE_LABEL,
    directive::{build_instruction, DirectiveContext},
};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use foundry_core::{
    errors::ContainerError,
    events::Event,
    traits::{
        container_runtime::{ContainerRuntime, ContainerSpec, Mount, VolumeSource},
        session_store::SessionStore,
    },
    types::{IssueKey, IssuePhase, IssueSession, RepoId},
};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};
use tokio::sync::{Mutex, Semaphore};
use tracing::{debug, info, warn};

pub struct Dispatcher {
    pub store: Arc<dyn SessionStore>,
    pub runtime: Arc<dyn ContainerRuntime>,
    pub config: Arc<Config>,
    seen_deliveries: Arc<Mutex<HashMap<String, DateTime<Utc>>>>,
    pub poll_watermark: Arc<Mutex<Option<DateTime<Utc>>>>,
    event_queue: Arc<Mutex<HashMap<IssueKey, VecDeque<Event>>>>,
    concurrency_semaphore: Arc<Semaphore>,
}

impl Dispatcher {
    pub fn new(
        store: Arc<dyn SessionStore>,
        runtime: Arc<dyn ContainerRuntime>,
        config: Arc<Config>,
    ) -> Self {
        let max_concurrent = config.container.max_concurrent;
        let watermark = Utc::now() - ChronoDuration::hours(24);
        Self {
            store,
            runtime,
            config,
            seen_deliveries: Arc::new(Mutex::new(HashMap::new())),
            poll_watermark: Arc::new(Mutex::new(Some(watermark))),
            event_queue: Arc::new(Mutex::new(HashMap::new())),
            concurrency_semaphore: Arc::new(Semaphore::new(max_concurrent)),
        }
    }

    /// Check and record delivery ID. Returns true if this is a duplicate.
    async fn is_duplicate(&self, delivery_id: &str) -> bool {
        let mut seen = self.seen_deliveries.lock().await;
        let now = Utc::now();
        // Clean up old entries (5-minute TTL)
        seen.retain(|_, ts| now - *ts < ChronoDuration::minutes(5));
        if seen.contains_key(delivery_id) {
            return true;
        }
        seen.insert(delivery_id.to_string(), now);
        false
    }

    /// Update poll watermark with a new timestamp.
    async fn advance_watermark(&self, ts: &DateTime<Utc>) {
        let mut wm = self.poll_watermark.lock().await;
        if wm.map(|w| ts > &w).unwrap_or(true) {
            *wm = Some(*ts);
        }
    }

    /// Enqueue an event for a specific issue key.
    async fn enqueue_event(&self, key: &IssueKey, event: Event) {
        let mut queues = self.event_queue.lock().await;
        queues.entry(key.clone()).or_default().push_back(event);
    }

    /// Drain and return queued events for an issue.
    #[allow(dead_code)]
    async fn drain_queue(&self, key: &IssueKey) -> Vec<Event> {
        let mut queues = self.event_queue.lock().await;
        queues.remove(key).map(|q| q.into_iter().collect()).unwrap_or_default()
    }

    pub async fn handle_event(&self, event: Event) -> anyhow::Result<()> {
        // Extract delivery_id and check dedup
        let delivery_id = match &event {
            Event::IssueAssigned { delivery_id, .. } => Some(delivery_id.clone()),
            Event::IssueClosed { delivery_id, .. } => Some(delivery_id.clone()),
            Event::IssueCommentCreated { delivery_id, .. } => Some(delivery_id.clone()),
            Event::PrReviewSubmitted { delivery_id, .. } => Some(delivery_id.clone()),
            Event::PrMerged { delivery_id, .. } => Some(delivery_id.clone()),
            Event::PrClosed { delivery_id, .. } => Some(delivery_id.clone()),
            Event::PollRecovery { .. } => None,
        };

        if let Some(ref did) = delivery_id {
            if self.is_duplicate(did).await {
                debug!("Duplicate delivery {}, skipping", did);
                return Ok(());
            }
        }

        // Advance watermark
        let ts = match &event {
            Event::IssueAssigned { timestamp, .. } => Some(*timestamp),
            Event::IssueClosed { timestamp, .. } => Some(*timestamp),
            Event::IssueCommentCreated { timestamp, .. } => Some(*timestamp),
            Event::PrReviewSubmitted { timestamp, .. } => Some(*timestamp),
            Event::PrMerged { timestamp, .. } => Some(*timestamp),
            Event::PrClosed { timestamp, .. } => Some(*timestamp),
            Event::PollRecovery { timestamp, .. } => Some(*timestamp),
        };
        if let Some(ref t) = ts {
            self.advance_watermark(t).await;
        }

        match event {
            Event::IssueAssigned { repo, issue_number, .. } => {
                let key = IssueKey { owner: repo.owner, repo: repo.repo, issue_number };
                let existing = self.store.get(&key).await?;
                if existing.is_none() {
                    let session = IssueSession {
                        key: key.clone(),
                        phase: IssuePhase::Planning,
                        pr_number: None,
                        container_running: false,
                    };
                    self.store.upsert(&session).await?;
                    info!("Created session for {}/{}/{}", key.owner, key.repo, key.issue_number);
                }
                self.spawn_turn(&key, None).await?;
            }

            Event::IssueClosed { repo, issue_number, .. } => {
                let key = IssueKey { owner: repo.owner, repo: repo.repo, issue_number };
                if let Some(session) = self.store.get(&key).await? {
                    // Only delete if no PR (otherwise handled by PrMerged/PrClosed)
                    if session.pr_number.is_none() {
                        self.store.delete(&key).await?;
                        let vol = key.volume_name(&self.config.volumes.issue_prefix);
                        let _ = self.runtime.remove_volume(&vol).await;
                        info!("Deleted session for closed issue {}/{}/{}", key.owner, key.repo, key.issue_number);
                    }
                }
            }

            Event::IssueCommentCreated { repo, issue_number, author, body, comment_id, .. } => {
                let key = IssueKey { owner: repo.owner.clone(), repo: repo.repo.clone(), issue_number };
                // Ignore bot comments
                if author == self.config.gitea.bot_username {
                    debug!("Ignoring bot comment from {}", author);
                    return Ok(());
                }

                let session = match self.store.get(&key).await? {
                    Some(s) => s,
                    None => return Ok(()),
                };

                let is_approve = body.trim().starts_with(&self.config.commands.approve);

                if is_approve && session.phase == IssuePhase::Planning {
                    // Transition to Implementing
                    let mut updated = session.clone();
                    updated.phase = IssuePhase::Implementing;
                    self.store.upsert(&updated).await?;
                    info!("Approved issue {}/{}/{}, transitioning to Implementing", key.owner, key.repo, key.issue_number);
                    self.spawn_turn(&key, None).await?;
                } else if !session.container_running {
                    self.spawn_turn(&key, Some(format!("Comment from {}: {}", author, body))).await?;
                } else {
                    self.enqueue_event(&key, Event::IssueCommentCreated {
                        repo: RepoId { owner: repo.owner, repo: repo.repo },
                        issue_number,
                        comment_id,
                        author,
                        body,
                        delivery_id: String::new(),
                        timestamp: Utc::now(),
                    }).await;
                }
            }

            Event::PrReviewSubmitted { repo, pr_number, reviewer, state, .. } => {
                let session = self.store
                    .get_by_pr(&repo.owner, &repo.repo, pr_number)
                    .await?;
                if let Some(session) = session {
                    let key = session.key.clone();
                    if !session.container_running {
                        let summary = format!("Review by {}: {:?}", reviewer, state);
                        self.spawn_turn(&key, Some(summary)).await?;
                    } else {
                        self.enqueue_event(&key, Event::PrReviewSubmitted {
                            repo,
                            pr_number,
                            reviewer,
                            state,
                            delivery_id: String::new(),
                            timestamp: Utc::now(),
                        }).await;
                    }
                }
            }

            Event::PrMerged { repo, pr_number, .. } => {
                if let Some(session) = self.store.get_by_pr(&repo.owner, &repo.repo, pr_number).await? {
                    let key = session.key.clone();
                    self.store.delete(&key).await?;
                    let vol = key.volume_name(&self.config.volumes.issue_prefix);
                    let _ = self.runtime.remove_volume(&vol).await;
                    info!("PR #{} merged, deleted session for {}/{}/{}", pr_number, key.owner, key.repo, key.issue_number);
                }
            }

            Event::PrClosed { repo, pr_number, .. } => {
                if let Some(session) = self.store.get_by_pr(&repo.owner, &repo.repo, pr_number).await? {
                    let key = session.key.clone();
                    self.store.delete(&key).await?;
                    let vol = key.volume_name(&self.config.volumes.issue_prefix);
                    let _ = self.runtime.remove_volume(&vol).await;
                    info!("PR #{} closed, deleted session for {}/{}/{}", pr_number, key.owner, key.repo, key.issue_number);
                }
            }

            Event::PollRecovery { repo, issue_number, .. } => {
                let key = IssueKey { owner: repo.owner, repo: repo.repo, issue_number };
                let session = self.store.get(&key).await?;

                match session {
                    None => {
                        // Create new Planning session and spawn
                        let new_session = IssueSession {
                            key: key.clone(),
                            phase: IssuePhase::Planning,
                            pr_number: None,
                            container_running: false,
                        };
                        self.store.upsert(&new_session).await?;
                        self.spawn_turn(&key, None).await?;
                    }
                    Some(s) if s.phase == IssuePhase::InReview && !s.container_running => {
                        let summary = Some("Poll recovery: check for unaddressed review feedback".to_string());
                        self.spawn_turn(&key, summary).await?;
                    }
                    _ => {
                        debug!("PollRecovery ignored for {}/{}/{}: already handled", key.owner, key.repo, key.issue_number);
                    }
                }
            }
        }

        Ok(())
    }

    pub async fn spawn_turn(
        &self,
        key: &IssueKey,
        pending_summary: Option<String>,
    ) -> anyhow::Result<()> {
        let session = match self.store.get(key).await? {
            Some(s) => s,
            None => {
                warn!("spawn_turn: session not found for {}/{}/{}", key.owner, key.repo, key.issue_number);
                return Ok(());
            }
        };

        if session.container_running {
            debug!("spawn_turn: container already running for {}/{}/{}", key.owner, key.repo, key.issue_number);
            return Ok(());
        }

        // Mark container_running = true
        let mut updated = session.clone();
        updated.container_running = true;
        self.store.upsert(&updated).await?;

        // Build instruction
        let branch_name = Some(format!("foundry/issue-{}", key.issue_number));
        let ctx = DirectiveContext {
            phase: session.phase,
            owner: key.owner.clone(),
            repo: key.repo.clone(),
            issue_number: key.issue_number,
            issue_title: format!("Issue #{}", key.issue_number),
            branch_name,
            pr_number: session.pr_number,
            pending_event_summary: pending_summary,
            gitea_url: self.config.gitea.url.clone(),
            bot_username: self.config.gitea.bot_username.clone(),
        };

        let instruction = build_instruction(&ctx);
        let instruction_json = serde_json::to_vec(&instruction)?;

        // Ensure volume and write instruction
        let vol_name = key.volume_name(&self.config.volumes.issue_prefix);
        self.runtime.ensure_volume(&vol_name).await
            .unwrap_or_else(|e| warn!("ensure_volume failed: {}", e));
        self.runtime.write_to_volume(&vol_name, "instruction.json", &instruction_json).await
            .unwrap_or_else(|e| warn!("write_to_volume failed: {}", e));

        // Build container spec
        let mut env = HashMap::new();
        env.insert("GITEA_HOST".to_string(), self.config.gitea.url.clone());
        env.insert("GITEA_ACCESS_TOKEN".to_string(), self.config.gitea.api_token.clone());
        env.insert("GITEA_BOT_USERNAME".to_string(), self.config.gitea.bot_username.clone());
        env.insert("GIT_AUTHOR_NAME".to_string(), self.config.gitea.bot_display_name.clone());
        env.insert("GIT_AUTHOR_EMAIL".to_string(), self.config.gitea.bot_email.clone());
        // ANTHROPIC_API_KEY may be passed through the environment
        if let Ok(key_val) = std::env::var("ANTHROPIC_API_KEY") {
            env.insert("ANTHROPIC_API_KEY".to_string(), key_val);
        }

        let mut labels = HashMap::new();
        labels.insert(
            FOUNDRY_ISSUE_LABEL.to_string(),
            format!("{}/{}/{}", key.owner, key.repo, key.issue_number),
        );

        let mounts = vec![
            Mount {
                source: VolumeSource::Named(vol_name.clone()),
                target: std::path::PathBuf::from("/foundry"),
                read_only: false,
            },
            Mount {
                source: VolumeSource::Named(self.config.volumes.shared_volume.clone()),
                target: std::path::PathBuf::from("/etc/foundry"),
                read_only: true,
            },
        ];

        let memory_limit_bytes = Some(self.config.container.memory_limit_mb * 1024 * 1024);
        // CPU: 100000 period, quota = cpu_limit * period
        let cpu_period = Some(100_000u64);
        let cpu_quota = Some((self.config.container.cpu_limit * 100_000.0) as i64);

        let spec = ContainerSpec {
            image: self.config.container.image.clone(),
            env,
            mounts,
            network: Some(self.config.container.network.clone()),
            memory_limit_bytes,
            cpu_period,
            cpu_quota,
            labels,
            timeout_secs: self.config.container.timeout_secs,
        };

        // Clone everything needed for spawn
        let runtime = self.runtime.clone();
        let store = self.store.clone();
        let config = self.config.clone();
        let key_clone = key.clone();
        let event_queue = self.event_queue.clone();
        let semaphore = self.concurrency_semaphore.clone();
        let vol_name_clone = vol_name.clone();

        tokio::spawn(async move {
            // Acquire concurrency permit
            let _permit = match semaphore.acquire().await {
                Ok(p) => p,
                Err(_) => {
                    warn!("Semaphore closed, aborting spawn_turn");
                    return;
                }
            };

            let result = runtime.run_container(&spec).await;

            match result {
                Ok(container_result) => {
                    info!(
                        "Container {} exited with code {}",
                        container_result.container_id, container_result.exit_code
                    );

                    // Try to read result.json for pr_number
                    let mut new_pr_number = None;
                    if let Ok(bytes) = runtime.read_from_volume(&vol_name_clone, "result.json").await {
                        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                            new_pr_number = json.get("pr_number").and_then(|v| v.as_u64());
                        }
                    }

                    // Update session
                    if let Ok(Some(mut s)) = store.get(&key_clone).await {
                        s.container_running = false;
                        if let Some(pr) = new_pr_number {
                            s.pr_number = Some(pr);
                            s.phase = IssuePhase::InReview;
                        }
                        let _ = store.upsert(&s).await;
                    }

                    // Clean up the container
                    let _ = runtime.remove_container(&container_result.container_id).await;
                }
                Err(ContainerError::Timeout { container_id }) => {
                    warn!("Container timed out: {}", container_id);
                    // Post failure comment (best-effort via reqwest)
                    post_failure_comment(&config, &key_clone, "Container timed out").await;

                    if let Ok(Some(mut s)) = store.get(&key_clone).await {
                        s.container_running = false;
                        let _ = store.upsert(&s).await;
                    }
                    let _ = runtime.remove_container(&container_id).await;
                }
                Err(e) => {
                    warn!("Container error for {}/{}/{}: {}", key_clone.owner, key_clone.repo, key_clone.issue_number, e);
                    post_failure_comment(&config, &key_clone, &e.to_string()).await;

                    if let Ok(Some(mut s)) = store.get(&key_clone).await {
                        s.container_running = false;
                        let _ = store.upsert(&s).await;
                    }
                }
            }

            // Drain event queue (clear it)
            {
                let mut queues = event_queue.lock().await;
                queues.remove(&key_clone);
            }
        });

        Ok(())
    }
}

async fn post_failure_comment(config: &Config, key: &IssueKey, reason: &str) {
    let url = format!(
        "{}/api/v1/repos/{}/{}/issues/{}/comments",
        config.gitea.url, key.owner, key.repo, key.issue_number
    );
    let body = serde_json::json!({
        "body": format!("⚠️ Foundry container failed: {}", reason)
    });
    let client = reqwest::Client::new();
    if let Err(e) = client
        .post(&url)
        .header("Authorization", format!("token {}", config.gitea.api_token))
        .json(&body)
        .send()
        .await
    {
        warn!("Failed to post failure comment: {}", e);
    }
}

// ------- TESTS -------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::*, session_store::memory::MemorySessionStore};
    use async_trait::async_trait;
    use foundry_core::{
        errors::ContainerError,
        traits::container_runtime::{ContainerResult, ContainerRuntime, ContainerSpec},
        types::IssueKey,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Mock container runtime
    struct MockRuntime {
        spawn_count: Arc<AtomicUsize>,
    }

    impl MockRuntime {
        fn new() -> Arc<Self> {
            Arc::new(Self { spawn_count: Arc::new(AtomicUsize::new(0)) })
        }
    }

    #[async_trait]
    impl ContainerRuntime for MockRuntime {
        async fn run_container(&self, _spec: &ContainerSpec) -> Result<ContainerResult, ContainerError> {
            self.spawn_count.fetch_add(1, Ordering::SeqCst);
            Ok(ContainerResult { container_id: "mock-container".to_string(), exit_code: 0 })
        }

        async fn ensure_volume(&self, _name: &str) -> Result<(), ContainerError> { Ok(()) }
        async fn remove_volume(&self, _name: &str) -> Result<(), ContainerError> { Ok(()) }
        async fn remove_container(&self, _id: &str) -> Result<(), ContainerError> { Ok(()) }
        async fn write_to_volume(&self, _vol: &str, _path: &str, _contents: &[u8]) -> Result<(), ContainerError> { Ok(()) }
        async fn read_from_volume(&self, _vol: &str, _path: &str) -> Result<Vec<u8>, ContainerError> {
            Ok(b"{}".to_vec())
        }
        async fn list_running_with_label(&self, _key: &str, _val: Option<&str>) -> Result<Vec<String>, ContainerError> { Ok(vec![]) }
        async fn kill_container(&self, _id: &str) -> Result<(), ContainerError> { Ok(()) }
    }

    fn make_config() -> Arc<Config> {
        let toml = r#"
[server]
listen_addr = "0.0.0.0:8477"
webhook_secret = "secret"

[gitea]
url = "http://gitea.local"
api_token = "token"
bot_username = "foundry-bot"
bot_display_name = "Foundry Bot"
bot_email = "bot@local"

[container]
image = "foundry-runner:latest"
runtime = "docker"
network = "foundry-net"
memory_limit_mb = 512
cpu_limit = 0.5
max_concurrent = 4
timeout_secs = 60

[volumes]
issue_prefix = "foundry-issue"
shared_volume = "foundry-shared"

[commands]
approve = "/approve"
"#;
        Arc::new(Config::from_toml(toml).unwrap())
    }

    fn make_dispatcher(runtime: Arc<dyn ContainerRuntime>) -> Dispatcher {
        let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
        let config = make_config();
        Dispatcher::new(store, runtime, config)
    }

    #[tokio::test]
    async fn issue_assigned_creates_session_and_spawns_container() {
        let runtime = MockRuntime::new();
        let spawn_count = runtime.spawn_count.clone();
        let dispatcher = make_dispatcher(runtime);

        let event = Event::IssueAssigned {
            repo: RepoId { owner: "alice".into(), repo: "proj".into() },
            issue_number: 1,
            assigner: "bob".into(),
            delivery_id: "del-1".into(),
            timestamp: Utc::now(),
        };

        dispatcher.handle_event(event).await.unwrap();
        // Give spawn time to run
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let key = IssueKey { owner: "alice".into(), repo: "proj".into(), issue_number: 1 };
        let session = dispatcher.store.get(&key).await.unwrap();
        assert!(session.is_some(), "Session should exist");

        assert_eq!(spawn_count.load(Ordering::SeqCst), 1, "Should spawn 1 container");
    }

    #[tokio::test]
    async fn approve_comment_transitions_to_implementing() {
        let runtime = MockRuntime::new();
        let dispatcher = make_dispatcher(runtime);

        // First, create a Planning session via IssueAssigned
        dispatcher.handle_event(Event::IssueAssigned {
            repo: RepoId { owner: "alice".into(), repo: "proj".into() },
            issue_number: 2,
            assigner: "bob".into(),
            delivery_id: "del-1".into(),
            timestamp: Utc::now(),
        }).await.unwrap();

        // Wait for container to finish
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Reset container_running
        let key = IssueKey { owner: "alice".into(), repo: "proj".into(), issue_number: 2 };
        if let Ok(Some(mut s)) = dispatcher.store.get(&key).await {
            s.container_running = false;
            dispatcher.store.upsert(&s).await.unwrap();
        }

        // Send /approve comment
        dispatcher.handle_event(Event::IssueCommentCreated {
            repo: RepoId { owner: "alice".into(), repo: "proj".into() },
            issue_number: 2,
            comment_id: 99,
            author: "alice".into(),
            body: "/approve".into(),
            delivery_id: "del-2".into(),
            timestamp: Utc::now(),
        }).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let session = dispatcher.store.get(&key).await.unwrap().unwrap();
        assert_eq!(session.phase, IssuePhase::Implementing, "Phase should be Implementing after /approve");
    }

    #[tokio::test]
    async fn bot_comment_does_not_spawn_container() {
        let runtime = MockRuntime::new();
        let spawn_count = runtime.spawn_count.clone();
        let dispatcher = make_dispatcher(runtime);

        // Create session
        dispatcher.handle_event(Event::IssueAssigned {
            repo: RepoId { owner: "alice".into(), repo: "proj".into() },
            issue_number: 3,
            assigner: "alice".into(),
            delivery_id: "del-1".into(),
            timestamp: Utc::now(),
        }).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let count_after_assign = spawn_count.load(Ordering::SeqCst);

        // Bot comment should be ignored
        dispatcher.handle_event(Event::IssueCommentCreated {
            repo: RepoId { owner: "alice".into(), repo: "proj".into() },
            issue_number: 3,
            comment_id: 1,
            author: "foundry-bot".into(),
            body: "I am working on it".into(),
            delivery_id: "del-2".into(),
            timestamp: Utc::now(),
        }).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert_eq!(
            spawn_count.load(Ordering::SeqCst),
            count_after_assign,
            "Bot comment should not spawn container"
        );
    }

    #[tokio::test]
    async fn pr_merged_deletes_session() {
        let runtime = MockRuntime::new();
        let dispatcher = make_dispatcher(runtime);

        // Set up InReview session
        let key = IssueKey { owner: "alice".into(), repo: "proj".into(), issue_number: 5 };
        let session = IssueSession {
            key: key.clone(),
            phase: IssuePhase::InReview,
            pr_number: Some(42),
            container_running: false,
        };
        dispatcher.store.upsert(&session).await.unwrap();

        // PrMerged event
        dispatcher.handle_event(Event::PrMerged {
            repo: RepoId { owner: "alice".into(), repo: "proj".into() },
            pr_number: 42,
            delivery_id: "del-3".into(),
            timestamp: Utc::now(),
        }).await.unwrap();

        let after = dispatcher.store.get(&key).await.unwrap();
        assert!(after.is_none(), "Session should be deleted after PrMerged");
    }

    #[tokio::test]
    async fn deduplication_ignores_duplicate_delivery_id() {
        let runtime = MockRuntime::new();
        let spawn_count = runtime.spawn_count.clone();
        let dispatcher = make_dispatcher(runtime);

        let event = Event::IssueAssigned {
            repo: RepoId { owner: "alice".into(), repo: "proj".into() },
            issue_number: 10,
            assigner: "bob".into(),
            delivery_id: "same-delivery-id".into(),
            timestamp: Utc::now(),
        };

        dispatcher.handle_event(event.clone()).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        dispatcher.handle_event(event).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Should only spawn once
        assert_eq!(spawn_count.load(Ordering::SeqCst), 1, "Duplicate delivery should be deduped");
    }
}
