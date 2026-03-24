use async_trait::async_trait;
use chrono::{DateTime, Utc};
use foundry_core::{
    errors::CodeHostError,
    traits::code_host::{CodeHost, HostComment, HostIssue, HostReview},
    types::{IssueKey, ReviewState},
};
use serde::Deserialize;

pub struct GiteaCodeHost {
    base_url: String,
    token: String,
    #[allow(dead_code)]
    bot_username: String,
    client: reqwest::Client,
}

impl GiteaCodeHost {
    pub fn new(base_url: String, token: String, bot_username: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            bot_username,
            client: reqwest::Client::new(),
        }
    }
}

// Internal raw response structs

#[derive(Debug, Deserialize)]
struct GiteaUserRaw {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GiteaRepoRefRaw {
    owner: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct GiteaIssueRaw {
    number: u64,
    repository: Option<GiteaRepoRefRaw>,
}

#[derive(Debug, Deserialize)]
struct GiteaCommentRaw {
    id: u64,
    user: GiteaUserRaw,
    body: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct GiteaReviewRaw {
    id: u64,
    user: GiteaUserRaw,
    state: String,
    submitted_at: DateTime<Utc>,
}

fn map_review_state(state: &str) -> ReviewState {
    match state {
        "APPROVED" => ReviewState::Approved,
        "REQUEST_CHANGES" => ReviewState::ChangesRequested,
        _ => ReviewState::Comment,
    }
}

#[async_trait]
impl CodeHost for GiteaCodeHost {
    async fn list_assigned_issues(
        &self,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<HostIssue>, CodeHostError> {
        let url = format!("{}/api/v1/repos/issues/search", self.base_url);
        let mut query = vec![
            ("type", "issues".to_string()),
            ("assigned", "true".to_string()),
            ("state", "open".to_string()),
        ];
        if let Some(since_dt) = since {
            query.push(("since", since_dt.to_rfc3339()));
        }

        let resp = self
            .client
            .get(&url)
            .query(&query)
            .header("Authorization", format!("token {}", self.token))
            .send()
            .await
            .map_err(|e| CodeHostError::Http(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(CodeHostError::Unauthorized);
        }
        if !resp.status().is_success() {
            return Err(CodeHostError::Http(format!("HTTP {}", resp.status())));
        }

        let issues: Vec<GiteaIssueRaw> = resp
            .json()
            .await
            .map_err(|e| CodeHostError::UnexpectedResponse(e.to_string()))?;

        let result = issues
            .into_iter()
            .filter_map(|issue| {
                let repo = issue.repository?;
                Some(HostIssue {
                    key: IssueKey {
                        owner: repo.owner,
                        repo: repo.name,
                        issue_number: issue.number,
                    },
                })
            })
            .collect();

        Ok(result)
    }

    async fn list_issue_comments(
        &self,
        key: &IssueKey,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<HostComment>, CodeHostError> {
        let url = format!(
            "{}/api/v1/repos/{}/{}/issues/{}/comments",
            self.base_url, key.owner, key.repo, key.issue_number
        );
        let mut query = vec![];
        if let Some(since_dt) = since {
            query.push(("since", since_dt.to_rfc3339()));
        }

        let resp = self
            .client
            .get(&url)
            .query(&query)
            .header("Authorization", format!("token {}", self.token))
            .send()
            .await
            .map_err(|e| CodeHostError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(CodeHostError::Http(format!("HTTP {}", resp.status())));
        }

        let comments: Vec<GiteaCommentRaw> = resp
            .json()
            .await
            .map_err(|e| CodeHostError::UnexpectedResponse(e.to_string()))?;

        Ok(comments
            .into_iter()
            .map(|c| HostComment {
                id: c.id,
                author: c.user.login,
                body: c.body,
                created_at: c.created_at,
            })
            .collect())
    }

    async fn list_pr_reviews(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<Vec<HostReview>, CodeHostError> {
        let url = format!(
            "{}/api/v1/repos/{}/{}/pulls/{}/reviews",
            self.base_url, owner, repo, pr_number
        );

        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("token {}", self.token))
            .send()
            .await
            .map_err(|e| CodeHostError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(CodeHostError::Http(format!("HTTP {}", resp.status())));
        }

        let reviews: Vec<GiteaReviewRaw> = resp
            .json()
            .await
            .map_err(|e| CodeHostError::UnexpectedResponse(e.to_string()))?;

        Ok(reviews
            .into_iter()
            .map(|r| HostReview {
                id: r.id,
                reviewer: r.user.login,
                state: map_review_state(&r.state),
                submitted_at: r.submitted_at,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;

    #[tokio::test]
    async fn list_assigned_issues_parses_response() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock(
                "GET",
                "/api/v1/repos/issues/search?type=issues&assigned=true&state=open",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"[{
                "number": 1,
                "title": "Fix bug",
                "body": "desc",
                "state": "open",
                "updated_at": "2026-01-01T00:00:00Z",
                "assignees": [{"login": "foundry-bot"}],
                "user": {"login": "alice"},
                "repository": {"owner": "alice", "name": "proj"}
            }]"#,
            )
            .create_async()
            .await;

        let host = GiteaCodeHost::new(server.url(), "test".into(), "foundry-bot".into());
        let issues = host.list_assigned_issues(None).await.unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].key.issue_number, 1);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn list_issue_comments_parses_response() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("GET", "/api/v1/repos/alice/proj/issues/1/comments")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"[{
                "id": 10,
                "user": {"login": "alice"},
                "body": "/approve",
                "created_at": "2026-01-01T00:00:00Z"
            }]"#,
            )
            .create_async()
            .await;

        let host = GiteaCodeHost::new(server.url(), "test".into(), "foundry-bot".into());
        let key = IssueKey {
            owner: "alice".into(),
            repo: "proj".into(),
            issue_number: 1,
        };
        let comments = host.list_issue_comments(&key, None).await.unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].body, "/approve");
        mock.assert_async().await;
    }
}
