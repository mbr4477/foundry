use async_trait::async_trait;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
    Router,
};
use foundry_core::{
    errors::EventSourceError,
    events::Event,
    traits::event_source::EventSource,
    types::{RepoId, ReviewState},
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

type HmacSha256 = Hmac<Sha256>;

#[allow(dead_code)]
pub fn compute_hmac_sha256(secret: &str, body: &[u8]) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
    mac.update(body);
    let result = mac.finalize().into_bytes();
    hex::encode(result)
}

pub fn verify_hmac_sha256(secret: &str, body: &[u8], signature: &str) -> bool {
    // Strip "sha256=" prefix if present
    let sig_hex = signature.strip_prefix("sha256=").unwrap_or(signature);
    // Decode the provided signature from hex
    let sig_bytes = match hex::decode(sig_hex) {
        Ok(b) => b,
        Err(_) => return false,
    };
    // Use HMAC's constant-time verify
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
    mac.update(body);
    mac.verify_slice(&sig_bytes).is_ok()
}

/// Internal JSON shape for webhook payloads (shared fields)
#[derive(serde::Deserialize)]
struct WebhookPayload {
    action: Option<String>,
    issue: Option<WebhookIssue>,
    comment: Option<WebhookComment>,
    pull_request: Option<WebhookPr>,
    review: Option<WebhookReview>,
    repository: Option<WebhookRepo>,
    sender: Option<WebhookUser>,
}

#[derive(serde::Deserialize)]
struct WebhookIssue {
    number: u64,
}

#[derive(serde::Deserialize)]
struct WebhookComment {
    id: u64,
    body: String,
}

#[derive(serde::Deserialize)]
struct WebhookPr {
    number: u64,
    merged: Option<bool>,
}

#[derive(serde::Deserialize)]
struct WebhookReview {
    #[serde(rename = "type")]
    type_: String,
}

#[derive(serde::Deserialize)]
struct WebhookRepo {
    owner: WebhookUser,
    name: String,
}

#[derive(serde::Deserialize)]
struct WebhookUser {
    login: String,
}

pub fn parse_webhook(
    event_type: &str,
    body: &str,
    delivery_id: &str,
) -> Result<Option<Event>, EventSourceError> {
    let payload: WebhookPayload =
        serde_json::from_str(body).map_err(|e| EventSourceError::ParseError(e.to_string()))?;

    let action = payload.action.as_deref().unwrap_or("");

    let repo = if let Some(ref r) = payload.repository {
        RepoId {
            owner: r.owner.login.clone(),
            repo: r.name.clone(),
        }
    } else {
        return Ok(None);
    };

    let now = chrono::Utc::now();

    let event = match (event_type, action) {
        ("issues", "assigned") => {
            let issue = payload.issue.ok_or_else(|| {
                EventSourceError::ParseError("Missing issue in IssueAssigned".into())
            })?;
            let assigner = payload.sender.map(|s| s.login).unwrap_or_default();
            Some(Event::IssueAssigned {
                repo,
                issue_number: issue.number,
                assigner,
                delivery_id: delivery_id.to_string(),
                timestamp: now,
            })
        }
        ("issues", "closed") => {
            let issue = payload.issue.ok_or_else(|| {
                EventSourceError::ParseError("Missing issue in IssueClosed".into())
            })?;
            Some(Event::IssueClosed {
                repo,
                issue_number: issue.number,
                delivery_id: delivery_id.to_string(),
                timestamp: now,
            })
        }
        ("issue_comment", "created") => {
            let issue = payload.issue.ok_or_else(|| {
                EventSourceError::ParseError("Missing issue in IssueCommentCreated".into())
            })?;
            let comment = payload.comment.ok_or_else(|| {
                EventSourceError::ParseError("Missing comment in IssueCommentCreated".into())
            })?;
            let author = payload.sender.map(|s| s.login).unwrap_or_default();
            Some(Event::IssueCommentCreated {
                repo,
                issue_number: issue.number,
                comment_id: comment.id,
                author,
                body: comment.body,
                delivery_id: delivery_id.to_string(),
                timestamp: now,
            })
        }
        ("pull_request", "closed") => {
            let pr = payload.pull_request.ok_or_else(|| {
                EventSourceError::ParseError("Missing pull_request in PrClosed/PrMerged".into())
            })?;
            if pr.merged == Some(true) {
                Some(Event::PrMerged {
                    repo,
                    pr_number: pr.number,
                    delivery_id: delivery_id.to_string(),
                    timestamp: now,
                })
            } else {
                Some(Event::PrClosed {
                    repo,
                    pr_number: pr.number,
                    delivery_id: delivery_id.to_string(),
                    timestamp: now,
                })
            }
        }
        ("pull_request_rejected", "reviewed")
        | ("pull_request_comment", "reviewed")
        | ("pull_request_approved", "reviewed") => {
            let pr = payload.pull_request.ok_or_else(|| {
                EventSourceError::ParseError("Missing pull_request in PrReviewSubmitted".into())
            })?;
            let review = payload.review.ok_or_else(|| {
                EventSourceError::ParseError("Missing review in PrReviewSubmitted".into())
            })?;
            let reviewer = payload.sender.map(|s| s.login).unwrap_or_default();
            let state = match review.type_.as_str() {
                "pull_request_review_approved" => ReviewState::Approved,
                "pull_request_review_rejected" => ReviewState::ChangesRequested,
                _ => ReviewState::Comment,
            };
            Some(Event::PrReviewSubmitted {
                repo,
                pr_number: pr.number,
                reviewer,
                state,
                delivery_id: delivery_id.to_string(),
                timestamp: now,
            })
        }
        _ => None,
    };

    Ok(event)
}

// Axum handler state
struct WebhookState {
    secret: String,
    tx: mpsc::Sender<Event>,
}

async fn webhook_handler(
    State(state): State<Arc<WebhookState>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    // Extract headers
    let event_type = match headers.get("X-Gitea-Event").and_then(|v| v.to_str().ok()) {
        Some(e) => e.to_string(),
        None => {
            debug!("Missing X-Gitea-Event header");
            return StatusCode::BAD_REQUEST;
        }
    };

    let delivery_id = headers
        .get("X-Gitea-Delivery")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();

    let signature = headers
        .get("X-Gitea-Signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if !verify_hmac_sha256(&state.secret, &body, &signature) {
        warn!("HMAC verification failed for delivery {}", delivery_id);
        return StatusCode::UNAUTHORIZED;
    }

    let body_str = match std::str::from_utf8(&body) {
        Ok(s) => s,
        Err(_) => return StatusCode::BAD_REQUEST,
    };

    match parse_webhook(&event_type, body_str, &delivery_id) {
        Ok(Some(event)) => {
            if state.tx.send(event).await.is_err() {
                warn!("Dispatcher channel closed");
            }
            StatusCode::OK
        }
        Ok(None) => StatusCode::OK,
        Err(e) => {
            warn!("Failed to parse webhook: {}", e);
            StatusCode::BAD_REQUEST
        }
    }
}

pub struct WebhookSource {
    listen_addr: String,
    secret: String,
}

impl WebhookSource {
    pub fn new(listen_addr: String, secret: String) -> Self {
        Self {
            listen_addr,
            secret,
        }
    }
}

#[async_trait]
impl EventSource for WebhookSource {
    async fn run(
        &self,
        tx: mpsc::Sender<Event>,
        cancel: CancellationToken,
    ) -> Result<(), EventSourceError> {
        let state = Arc::new(WebhookState {
            secret: self.secret.clone(),
            tx,
        });

        let app = Router::new()
            .route("/webhook", post(webhook_handler))
            .with_state(state);

        let addr: std::net::SocketAddr = self
            .listen_addr
            .parse()
            .map_err(|e: std::net::AddrParseError| EventSourceError::Http(e.to_string()))?;

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| EventSourceError::Http(e.to_string()))?;

        axum::serve(listener, app)
            .with_graceful_shutdown(async move { cancel.cancelled().await })
            .await
            .map_err(|e| EventSourceError::Http(e.to_string()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_issue_payload(action: &str, issue_number: u64) -> String {
        serde_json::json!({
            "action": action,
            "issue": { "number": issue_number },
            "repository": {
                "owner": { "login": "alice" },
                "name": "proj"
            },
            "sender": { "login": "bob" }
        })
        .to_string()
    }

    fn make_comment_payload(
        issue_number: u64,
        comment_id: u64,
        body: &str,
        author: &str,
    ) -> String {
        serde_json::json!({
            "action": "created",
            "issue": { "number": issue_number },
            "comment": { "id": comment_id, "body": body },
            "repository": {
                "owner": { "login": "alice" },
                "name": "proj"
            },
            "sender": { "login": author }
        })
        .to_string()
    }

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
        let sig = compute_hmac_sha256(secret, b"original body");
        assert!(!verify_hmac_sha256(secret, b"tampered body", &sig));
    }

    #[test]
    fn parse_issue_assigned_webhook() {
        let body = make_issue_payload("assigned", 42);
        let event = parse_webhook("issues", &body, "del-1").unwrap().unwrap();
        match event {
            Event::IssueAssigned {
                repo, issue_number, ..
            } => {
                assert_eq!(repo.owner, "alice");
                assert_eq!(repo.repo, "proj");
                assert_eq!(issue_number, 42);
            }
            _ => panic!("Expected IssueAssigned"),
        }
    }

    #[test]
    fn parse_issue_comment_webhook_with_approve() {
        let body = make_comment_payload(5, 100, "/approve", "alice");
        let event = parse_webhook("issue_comment", &body, "del-2")
            .unwrap()
            .unwrap();
        match event {
            Event::IssueCommentCreated {
                body,
                author,
                comment_id,
                ..
            } => {
                assert_eq!(body, "/approve");
                assert_eq!(author, "alice");
                assert_eq!(comment_id, 100);
            }
            _ => panic!("Expected IssueCommentCreated"),
        }
    }

    #[test]
    fn parse_pr_comment_webhook() {
        let body = serde_json::json!({
            "action": "reviewed",
            "pull_request": { "number": 7, "merged": false},
            "repository": {
                "owner": { "login": "alice" },
                "name": "proj"
            },
            "sender": { "login": "alice" },
            "review": {"type": "pull_request_review_comment", "content": ""}
        })
        .to_string();
        let event = parse_webhook("pull_request_comment", &body, "del-3")
            .unwrap()
            .unwrap();
        match event {
            Event::PrReviewSubmitted {
                pr_number, state, ..
            } => {
                assert_eq!(pr_number, 7);
                assert_eq!(state, ReviewState::Comment);
            }
            _ => panic!("Expected PrReviewSubmitted"),
        }
    }
    #[test]
    fn parse_pr_review_rejected_webhook() {
        let body = serde_json::json!({
            "action": "reviewed",
            "pull_request": { "number": 7, "merged": false},
            "repository": {
                "owner": { "login": "alice" },
                "name": "proj"
            },
            "sender": { "login": "alice" },
            "review": {"type": "pull_request_review_rejected", "content": ""}
        })
        .to_string();
        let event = parse_webhook("pull_request_rejected", &body, "del-3")
            .unwrap()
            .unwrap();
        match event {
            Event::PrReviewSubmitted {
                pr_number, state, ..
            } => {
                assert_eq!(pr_number, 7);
                assert_eq!(state, ReviewState::ChangesRequested);
            }
            _ => panic!("Expected PrReviewSubmitted"),
        }
    }

    #[test]
    fn parse_pr_review_approved_webhook() {
        let body = serde_json::json!({
            "action": "reviewed",
            "pull_request": { "number": 7, "merged": false},
            "repository": {
                "owner": { "login": "alice" },
                "name": "proj"
            },
            "sender": { "login": "alice" },
            "review": {"type": "pull_request_review_approved", "content": ""}
        })
        .to_string();
        let event = parse_webhook("pull_request_approved", &body, "del-3")
            .unwrap()
            .unwrap();
        match event {
            Event::PrReviewSubmitted {
                pr_number, state, ..
            } => {
                assert_eq!(pr_number, 7);
                assert_eq!(state, ReviewState::Approved);
            }
            _ => panic!("Expected PrReviewSubmitted"),
        }
    }
    #[test]
    fn parse_pr_merged_webhook() {
        let body = serde_json::json!({
            "action": "closed",
            "pull_request": { "number": 7, "merged": true },
            "repository": { "owner": { "login": "alice" }, "name": "proj" },
            "sender": { "login": "alice" }
        })
        .to_string();
        let event = parse_webhook("pull_request", &body, "del-3")
            .unwrap()
            .unwrap();
        match event {
            Event::PrMerged { pr_number, .. } => assert_eq!(pr_number, 7),
            _ => panic!("Expected PrMerged"),
        }
    }

    #[test]
    fn parse_unknown_event_returns_none() {
        let body = make_issue_payload("labeled", 1);
        let result = parse_webhook("issues", &body, "del-4").unwrap();
        assert!(result.is_none());
    }
}
