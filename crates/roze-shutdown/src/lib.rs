use tokio::sync::watch;
use tokio::time::{timeout, Duration};

#[derive(Clone)]
pub struct ShutdownHandle {
    tx: watch::Sender<bool>,
}

#[derive(Clone)]
pub struct ShutdownListener {
    rx: watch::Receiver<bool>,
}

pub fn channel() -> (ShutdownHandle, ShutdownListener) {
    let (tx, rx) = watch::channel(false);
    (ShutdownHandle { tx }, ShutdownListener { rx })
}

impl ShutdownHandle {
    pub fn trigger(&self) {
        let _ = self.tx.send(true);
    }
}

impl ShutdownListener {
    pub async fn wait(mut self) {
        if self.is_triggered() {
            return;
        }
        while self.rx.changed().await.is_ok() {
            if self.is_triggered() {
                return;
            }
        }
    }

    pub fn is_triggered(&self) -> bool {
        *self.rx.borrow()
    }
}

pub async fn listen_for_ctrl_c() {
    let _ = tokio::signal::ctrl_c().await;
}

pub async fn wait_for_shutdown(listener: ShutdownListener) {
    listener.wait().await;
}

pub async fn wait_for_ctrl_c_or_timeout(duration: Duration) -> bool {
    timeout(duration, tokio::signal::ctrl_c()).await.is_err()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn trigger_wakes_listener_state() {
        let (handle, listener) = channel();
        assert!(!listener.is_triggered());
        handle.trigger();
        assert!(listener.is_triggered());
    }

    #[tokio::test]
    async fn wait_returns_when_already_triggered() {
        let (handle, listener) = channel();
        handle.trigger();

        timeout(Duration::from_millis(10), listener.wait())
            .await
            .expect("wait should return immediately");
    }
}
