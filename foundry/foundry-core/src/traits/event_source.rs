use crate::errors::EventSourceError;
use crate::events::Event;
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[async_trait]
pub trait EventSource: Send + Sync + 'static {
    /// Start producing events into `tx` until `cancel` is triggered.
    async fn run(
        &self,
        tx: mpsc::Sender<Event>,
        cancel: CancellationToken,
    ) -> Result<(), EventSourceError>;
}
