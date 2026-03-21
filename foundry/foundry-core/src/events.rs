use crate::types::{RepoId, ReviewState};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Event {
    IssueAssigned {
        repo: RepoId,
        issue_number: u64,
        assigner: String,
        delivery_id: String,
        timestamp: DateTime<Utc>,
    },
    IssueClosed {
        repo: RepoId,
        issue_number: u64,
        delivery_id: String,
        timestamp: DateTime<Utc>,
    },
    IssueCommentCreated {
        repo: RepoId,
        issue_number: u64,
        comment_id: u64,
        author: String,
        /// Body included here as an optimization; Claude may also fetch via MCP.
        body: String,
        delivery_id: String,
        timestamp: DateTime<Utc>,
    },
    /// PR reviews may have many inline comments — Claude fetches them via MCP.
    /// No body field here by design.
    PrReviewSubmitted {
        repo: RepoId,
        pr_number: u64,
        reviewer: String,
        state: ReviewState,
        delivery_id: String,
        timestamp: DateTime<Utc>,
    },
    PrMerged {
        repo: RepoId,
        pr_number: u64,
        delivery_id: String,
        timestamp: DateTime<Utc>,
    },
    PrClosed {
        repo: RepoId,
        pr_number: u64,
        delivery_id: String,
        timestamp: DateTime<Utc>,
    },
    /// Synthetic event from the polling fallback — no delivery_id.
    /// Deduplicated by (repo, issue_number, timestamp window).
    PollRecovery {
        repo: RepoId,
        issue_number: u64,
        timestamp: DateTime<Utc>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn event_serializes_to_json() {
        let event = Event::IssueAssigned {
            repo: RepoId { owner: "alice".into(), repo: "proj".into() },
            issue_number: 1,
            assigner: "bob".into(),
            delivery_id: "abc123".into(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("IssueAssigned"));
        assert!(json.contains("abc123"));
    }

    #[test]
    fn event_roundtrips_through_json() {
        let original = Event::IssueCommentCreated {
            repo: RepoId { owner: "alice".into(), repo: "proj".into() },
            issue_number: 5,
            comment_id: 99,
            author: "charlie".into(),
            body: "/approve".into(),
            delivery_id: "xyz".into(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let roundtripped: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(
            serde_json::to_value(&original).unwrap(),
            serde_json::to_value(&roundtripped).unwrap()
        );
    }
}
