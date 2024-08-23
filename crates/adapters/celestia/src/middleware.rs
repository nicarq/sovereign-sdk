use std::task::{Context, Poll};

use futures::future::BoxFuture;

#[derive(Clone)]
pub struct TimingLayer;

impl<S> tower::Layer<S> for TimingLayer {
    type Service = TimingMiddleware<S>;

    fn layer(&self, service: S) -> Self::Service {
        TimingMiddleware { inner: service }
    }
}

#[derive(Debug, Clone)]
pub struct TimingMiddleware<S> {
    inner: S,
}

impl<S, Request> tower::Service<Request> for TimingMiddleware<S>
where
    S: tower::Service<Request>,
    S::Future: Send + 'static,
    Request: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request) -> Self::Future {
        let start = tokio::time::Instant::now();

        let fut = self.inner.call(req);

        // Create a future that will log the duration once the inner service future completes
        Box::pin(async move {
            let result = fut.await;

            tracing::trace!(
                time_ms = start.elapsed().as_millis(),
                success = result.is_ok(),
                "Request call completed"
            );

            result
        })
    }
}
