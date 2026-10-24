//! The store on a thread of its own.
//!
//! rusqlite blocks the thread it runs on, so nothing on the UI thread touches
//! the store. Every call is a closure sent down a channel to one dedicated
//! thread that runs them in order; an answer comes back on a oneshot the
//! calling Task awaits. In order matters: a play begun and then finished must
//! reach SQLite in that order, which a pool of blocking threads would not
//! promise.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

use aoide_store::Store;
use tokio::sync::oneshot;

use crate::playback::recorder::PlayRecorder;

/// What the store thread owns: the store, and the one piece of state that
/// belongs next to it, the play being recorded.
pub struct Worker {
    pub store: Arc<Store>,
    pub recorder: PlayRecorder,
}

type Job = Box<dyn FnOnce(&mut Worker) + Send>;

/// The way to the store thread. Cheap to clone; the thread ends when the last
/// handle is dropped.
#[derive(Clone)]
pub struct StoreHandle {
    jobs: mpsc::Sender<Job>,
    store: Arc<Store>,
}

impl std::fmt::Debug for StoreHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreHandle").finish_non_exhaustive()
    }
}

impl StoreHandle {
    /// Opens the store at `path` (creating its directory) and starts its thread.
    pub fn open(path: &Path, device_id: &str) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let store = Store::open(path, device_id).map_err(|e| e.to_string())?;
        Self::start(store)
    }

    /// A store in memory, for tests.
    pub fn in_memory(device_id: &str) -> Result<Self, String> {
        Self::start(Store::in_memory(device_id).map_err(|e| e.to_string())?)
    }

    fn start(store: Store) -> Result<Self, String> {
        let store = Arc::new(store);
        let (jobs, inbox) = mpsc::channel::<Job>();
        let mut worker = Worker {
            store: Arc::clone(&store),
            recorder: PlayRecorder::default(),
        };
        thread::Builder::new()
            .name("aoide-store".into())
            .spawn(move || {
                while let Ok(job) = inbox.recv() {
                    job(&mut worker);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { jobs, store })
    }

    /// The store itself, for the sync engine, which runs its own transactions
    /// inside its future on a runtime worker thread, never the UI thread.
    pub fn shared(&self) -> Arc<Store> {
        Arc::clone(&self.store)
    }

    /// Runs `job` on the store thread and returns what it returns. `None` only
    /// if the thread has gone, which happens when the app is closing.
    pub async fn call<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut Worker) -> T + Send + 'static,
    ) -> Option<T> {
        let (reply, answer) = oneshot::channel();
        let sent = self.jobs.send(Box::new(move |worker: &mut Worker| {
            let _ = reply.send(job(worker));
        }));
        if sent.is_err() {
            return None;
        }
        answer.await.ok()
    }

    /// Runs `job` on the store thread without waiting for it.
    pub fn send(&self, job: impl FnOnce(&mut Worker) + Send + 'static) {
        let _ = self.jobs.send(Box::new(job));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn calls_run_on_the_store_thread_in_order() {
        let handle = StoreHandle::in_memory("device-1").unwrap();
        let name = handle
            .call(|_| thread::current().name().map(str::to_owned))
            .await
            .flatten();
        assert_eq!(name.as_deref(), Some("aoide-store"));

        let created = handle
            .call(|worker| worker.store.create_playlist("Late", None, None, 1).unwrap())
            .await
            .unwrap();
        handle.send(move |worker| {
            worker
                .store
                .rename_playlist(&created.id, "Later", 2)
                .unwrap();
        });
        let names = handle
            .call(|worker| {
                worker
                    .store
                    .playlists()
                    .unwrap()
                    .into_iter()
                    .map(|p| p.name)
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap();
        assert_eq!(names, ["Later"]);
    }
}
