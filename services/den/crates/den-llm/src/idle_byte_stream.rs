use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures::Stream;
use tokio::time::Sleep;

use den_core::{DenError, ProviderFailure, ProviderFailureKind};

/// Fails the stream when no upstream bytes arrive within `idle_limit`.
pub fn byte_stream_with_idle_timeout<S>(
    inner: S,
    idle_limit: Duration,
) -> impl Stream<Item = Result<Bytes, DenError>> + Send + Unpin
where
    S: Stream<Item = Result<Bytes, DenError>> + Send + Unpin + 'static,
{
    IdleTimeoutByteStream {
        inner: Box::pin(inner),
        idle: Box::pin(tokio::time::sleep(idle_limit)),
        idle_limit,
        finished: false,
    }
}

struct IdleTimeoutByteStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, DenError>> + Send + Unpin>>,
    idle: Pin<Box<Sleep>>,
    idle_limit: Duration,
    finished: bool,
}

impl IdleTimeoutByteStream {
    fn reset_idle(&mut self) {
        self.idle = Box::pin(tokio::time::sleep(self.idle_limit));
    }

    fn idle_elapsed_error(&self) -> DenError {
        DenError::Provider(ProviderFailure::new(ProviderFailureKind::IdleTimeout))
    }
}

impl Stream for IdleTimeoutByteStream {
    type Item = Result<Bytes, DenError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.finished {
            return Poll::Ready(None);
        }

        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(item)) => {
                self.reset_idle();
                Poll::Ready(Some(item))
            }
            Poll::Ready(None) => {
                self.finished = true;
                Poll::Ready(None)
            }
            Poll::Pending => match self.idle.as_mut().poll(cx) {
                Poll::Ready(()) => {
                    self.finished = true;
                    tracing::warn!(
                        idle_timeout_secs = self.idle_limit.as_secs(),
                        "LLM byte stream idle timeout"
                    );
                    Poll::Ready(Some(Err(self.idle_elapsed_error())))
                }
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::pin::pin;

    use futures::StreamExt;
    use tokio::time::{sleep, Duration};

    use super::*;

    fn empty_pending_stream() -> impl Stream<Item = Result<Bytes, DenError>> + Send + Unpin {
        futures::stream::pending()
    }

    #[tokio::test]
    async fn idle_timeout_fires_when_upstream_stalls() {
        let stream =
            byte_stream_with_idle_timeout(empty_pending_stream(), Duration::from_millis(50));
        let mut stream = pin!(stream);
        let item = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("should not hang forever")
            .expect("should emit timeout error")
            .expect_err("should be an error");
        assert!(matches!(
            item,
            DenError::Provider(failure) if failure.kind() == ProviderFailureKind::IdleTimeout
        ));
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn idle_timer_resets_after_bytes() {
        let stream = byte_stream_with_idle_timeout(
            futures::stream::iter(vec![Ok(Bytes::from("chunk"))]).chain(futures::stream::pending()),
            Duration::from_millis(80),
        );
        let mut stream = pin!(stream);
        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first, Bytes::from("chunk"));
        sleep(Duration::from_millis(30)).await;
        let second = tokio::time::timeout(Duration::from_millis(200), stream.next())
            .await
            .expect("second chunk should not wait full idle window")
            .expect("stream should end or error");
        assert!(second.is_err(), "expected idle timeout after stall");
    }
}
