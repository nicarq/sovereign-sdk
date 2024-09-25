use futures::future::BoxFuture;
use futures::FutureExt;
use tokio::sync::watch;

/// A notification from [`DropNotifier`].
pub type DropNotification = BoxFuture<'static, ()>;

/// Embed this in any `struct` for which you want to be notified when it is
/// dropped.
///
/// See [`DropNotifier::build`].
pub struct DropNotifier {
    notify: watch::Sender<()>,
}

impl DropNotifier {
    /// Builds a [`DropNotifier`] and a [`std::future::Future`] that will
    /// resolve when the [`DropNotifier`] is dropped.
    pub fn build() -> (Self, DropNotification) {
        let (notify, mut notified) = watch::channel(());

        // The initial value inside the channel should not trigger a
        // notification.
        notified.mark_unchanged();
        let fut = async move {
            notified.changed().await.ok();
        };

        (Self { notify }, fut.boxed())
    }
}

impl Drop for DropNotifier {
    fn drop(&mut self) {
        self.notify.send(()).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_drop_notifier() {
        let (notifier, notified) = DropNotifier::build();

        drop(notifier);
        notified.await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn no_drop_no_notification() {
        let (_notifier, notified) = DropNotifier::build();

        assert_eq!(notified.now_or_never(), None);
    }
}
