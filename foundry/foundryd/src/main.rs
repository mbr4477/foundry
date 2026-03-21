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

    // Initialize tracing — use env-filter for level, format determined by config
    let filter = cfg.logging.level.clone();
    if cfg.logging.format == "json" {
        tracing_subscriber::fmt()
            .with_env_filter(filter.as_str())
            .json()
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter.as_str())
            .init();
    }

    info!("foundryd starting");

    let store: Arc<dyn foundry_core::traits::session_store::SessionStore> =
        Arc::new(session_store::memory::MemorySessionStore::new());

    let runtime: Arc<dyn foundry_core::traits::container_runtime::ContainerRuntime> =
        Arc::new(container::docker::DockerRuntime::new().await?);

    let cfg = Arc::new(cfg);
    let dispatcher = Arc::new(dispatcher::Dispatcher::new(
        store.clone(),
        runtime.clone(),
        cfg.clone(),
    ));

    // Kill orphaned containers from previous crash
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

    // Reconstruct session state from Gitea
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
                    if store.get(&key).await.ok().flatten().is_some() {
                        continue;
                    }
                    let comments = code_host.list_issue_comments(&key, None).await.unwrap_or_default();
                    let has_approve = comments.iter().any(|c| c.body.trim().starts_with(&cfg.commands.approve));
                    let (phase, pr_number) = if let Some(pr) = issue.pr_number {
                        (IssuePhase::InReview, Some(pr))
                    } else if has_approve {
                        (IssuePhase::Implementing, None)
                    } else {
                        (IssuePhase::Planning, None)
                    };
                    let session = IssueSession { key: key.clone(), phase, pr_number, container_running: false };
                    store.upsert(&session).await.ok();
                    info!("Reconstructed session for {}/{}/{} as {:?}", key.owner, key.repo, key.issue_number, session.phase);
                    if phase == IssuePhase::InReview {
                        if let Some(pr) = pr_number {
                            let reviews = code_host.list_pr_reviews(&key.owner, &key.repo, pr).await.unwrap_or_default();
                            let has_unaddressed = reviews.iter().any(|r| matches!(r.state,
                                foundry_core::types::ReviewState::ChangesRequested | foundry_core::types::ReviewState::Comment
                            ));
                            if has_unaddressed {
                                dispatcher.handle_event(foundry_core::events::Event::PollRecovery {
                                    repo: foundry_core::types::RepoId { owner: key.owner.clone(), repo: key.repo.clone() },
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

    // Start polling source
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

    let cancel_signal = cancel.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("Shutdown signal received");
        cancel_signal.cancel();
    });

    info!("foundryd ready — listening on {}", cfg.server.listen_addr);

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

    // Graceful shutdown
    info!("Waiting for in-flight containers (timeout: {}s)...", cfg.container.timeout_secs);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(cfg.container.timeout_secs);
    loop {
        let sessions = store.list().await.unwrap_or_default();
        let running = sessions.iter().filter(|s| s.container_running).count();
        if running == 0 || std::time::Instant::now() >= deadline {
            if running > 0 {
                info!("Shutdown timeout reached with {} containers still running", running);
            } else {
                info!("All containers finished, shutting down cleanly.");
            }
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    info!("foundryd stopped");
    Ok(())
}
