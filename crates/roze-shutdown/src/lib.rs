use tokio::sync::watch;

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
        let _ = self.rx.changed().await;
    }

    pub fn is_triggered(&self) -> bool {
        *self.rx.borrow()
    }
}

pub async fn listen_for_ctrl_c() {
    let _ = tokio::signal::ctrl_c().await;
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
}
