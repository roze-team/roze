use std::{future::Future, sync::Arc};

use tokio::sync::{mpsc, Mutex};

#[derive(Debug)]
pub struct WorkerPool<J> {
    sender: mpsc::Sender<J>,
    workers: Vec<tokio::task::JoinHandle<()>>,
}

impl<J> WorkerPool<J>
where
    J: Send + 'static,
{
    pub fn spawn<F, Fut>(workers: usize, capacity: usize, handler: F) -> Self
    where
        F: Fn(J) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel(capacity.max(1));
        let receiver = Arc::new(Mutex::new(receiver));
        let handler = Arc::new(handler);
        let mut handles = Vec::new();

        for _ in 0..workers.max(1) {
            let receiver = receiver.clone();
            let handler = handler.clone();
            handles.push(tokio::spawn(async move {
                loop {
                    let job = {
                        let mut locked = receiver.lock().await;
                        locked.recv().await
                    };
                    match job {
                        Some(job) => handler(job).await,
                        None => break,
                    }
                }
            }));
        }

        Self {
            sender,
            workers: handles,
        }
    }

    pub async fn submit(&self, job: J) -> Result<(), mpsc::error::SendError<J>> {
        self.sender.send(job).await
    }

    pub async fn shutdown(self) {
        drop(self.sender);
        for handle in self.workers {
            let _ = handle.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[tokio::test]
    async fn processes_jobs() {
        let hits = Arc::new(AtomicUsize::new(0));
        let pool = WorkerPool::spawn(2, 8, {
            let hits = hits.clone();
            move |_job: usize| {
                let hits = hits.clone();
                async move {
                    let _ = hits.fetch_add(1, Ordering::SeqCst);
                }
            }
        });

        pool.submit(1).await.expect("submit");
        pool.submit(2).await.expect("submit");
        pool.submit(3).await.expect("submit");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        pool.shutdown().await;
        assert_eq!(hits.load(Ordering::SeqCst), 3);
    }
}
