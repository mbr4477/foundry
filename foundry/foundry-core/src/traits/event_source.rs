use crate::errors::EventSourceError;
use crate::events::Event;
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[async_trait]
pub trait EventSource: Send + Sync + 'static {
    async fn run(
        &self,
        tx: mpsc::Sender<Event>,
        cancel: CancellationToken,
    ) -> Result<(), EventSourceError>;
}
