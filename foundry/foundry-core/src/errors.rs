use thiserror::Error;

#[derive(Debug, Error)]
pub enum EventSourceError {
    #[error("Webhook signature verification failed")]
    InvalidSignature,
    #[error("Failed to parse webhook payload: {0}")]
    ParseError(String),
    #[error("HTTP server error: {0}")]
    Http(String),
    #[error("Channel closed — dispatcher shut down")]
    ChannelClosed,
}

#[derive(Debug, Error)]
pub enum ContainerError {
    #[error("Docker API error: {0}")]
    Api(String),
    #[error("Container {container_id} timed out")]
    Timeout { container_id: String },
    #[error("Failed to create volume '{name}': {reason}")]
    VolumeCreate { name: String, reason: String },
    #[error("Failed to write to volume '{volume}' at '{path}': {reason}")]
    VolumeWrite { volume: String, path: String, reason: String },
    #[error("Container exited with non-zero code {exit_code}")]
    NonZeroExit { exit_code: i64 },
}

#[derive(Debug, Error)]
pub enum SessionStoreError {
    #[error("Serialization error: {0}")]
    Internal(String),
}

#[derive(Debug, Error)]
pub enum CodeHostError {
    #[error("HTTP request failed: {0}")]
    Http(String),
    #[error("API rate limited; retry after {retry_after_secs:?} seconds")]
    RateLimited { retry_after_secs: Option<u64> },
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Unauthorized — check GITEA_ACCESS_TOKEN")]
    Unauthorized,
    #[error("Unexpected response: {0}")]
    UnexpectedResponse(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_source_error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<EventSourceError>();
    }

    #[test]
    fn container_error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ContainerError>();
    }

    #[test]
    fn session_store_error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SessionStoreError>();
    }

    #[test]
    fn code_host_error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CodeHostError>();
    }

    #[test]
    fn container_error_timeout_display() {
        let e = ContainerError::Timeout { container_id: "abc".into() };
        assert!(e.to_string().contains("abc"));
    }
}
