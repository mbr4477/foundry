use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoId {
    pub owner: String,
    pub repo: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IssueKey {
    pub owner: String,
    pub repo: String,
    pub issue_number: u64,
}

impl IssueKey {
    /// Derives the Docker volume name for this issue.
    /// Uses `__` as separator since Gitea names cannot contain `__`.
    pub fn volume_name(&self, prefix: &str) -> String {
        format!("{}__{}__{}__{}", prefix, self.owner, self.repo, self.issue_number)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueSession {
    pub key: IssueKey,
    pub phase: IssuePhase,
    /// Set once Claude opens the PR (read from result.json).
    pub pr_number: Option<u64>,
    /// True while a container is actively running for this issue.
    pub container_running: bool,
}

impl IssueSession {
    pub fn volume_name(&self, prefix: &str) -> String {
        self.key.volume_name(prefix)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IssuePhase {
    /// Assigned; Claude is reading the issue and asking clarifying questions.
    Planning,
    /// /approve received; Claude is implementing on a branch.
    Implementing,
    /// PR is open; Claude is responding to review feedback.
    InReview,
    /// PR merged or issue closed; no further action.
    Done,
}

impl fmt::Display for IssuePhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IssuePhase::Planning => write!(f, "planning"),
            IssuePhase::Implementing => write!(f, "implementing"),
            IssuePhase::InReview => write!(f, "in-review"),
            IssuePhase::Done => write!(f, "done"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    Approved,
    ChangesRequested,
    Comment,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_key_equality() {
        let a = IssueKey { owner: "alice".into(), repo: "myproject".into(), issue_number: 42 };
        let b = IssueKey { owner: "alice".into(), repo: "myproject".into(), issue_number: 42 };
        assert_eq!(a, b);
    }

    #[test]
    fn volume_name_uses_double_underscore_separator() {
        let key = IssueKey { owner: "alice".into(), repo: "my-project".into(), issue_number: 1 };
        let name = key.volume_name("foundry-issue");
        assert_eq!(name, "foundry-issue__alice__my-project__1");
        assert!(!name.contains("alice-my")); // would be ambiguous with single hyphen
    }

    #[test]
    fn issue_phase_display() {
        assert_eq!(IssuePhase::Planning.to_string(), "planning");
        assert_eq!(IssuePhase::Implementing.to_string(), "implementing");
        assert_eq!(IssuePhase::InReview.to_string(), "in-review");
        assert_eq!(IssuePhase::Done.to_string(), "done");
    }

    #[test]
    fn issue_session_volume_name_delegates_to_key() {
        let session = IssueSession {
            key: IssueKey { owner: "bob".into(), repo: "repo".into(), issue_number: 7 },
            phase: IssuePhase::Planning,
            pr_number: None,
            container_running: false,
        };
        assert_eq!(session.volume_name("foundry-issue"), "foundry-issue__bob__repo__7");
    }
}
