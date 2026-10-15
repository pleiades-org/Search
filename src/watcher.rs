use crate::{arena::IndexArena, engine::SharedIndex, walker::collect_paths_with_cancel};
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const DEBOUNCE: Duration = Duration::from_millis(50);
const MAX_COALESCE: Duration = Duration::from_millis(250);

enum Signal {
    Dirty,
    Shutdown,
}

pub struct WatchWorker {
    watcher: Option<RecommendedWatcher>,
    sender: SyncSender<Signal>,
    stopped: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

pub(crate) fn spawn_watcher(root: &Path, shared: Arc<SharedIndex>) -> io::Result<WatchWorker> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let callback_sender = sender.clone();
    let callback_state = shared.clone();
    let cache = crate::util::cache_directory();
    let cache = std::fs::canonicalize(&cache).unwrap_or(cache);
    let separate_cache_directory = cache != root;
    let mut watcher = RecommendedWatcher::new(
        move |event: notify::Result<notify::Event>| {
            match event {
                Ok(event)
                    if !event.paths.is_empty()
                        && event.paths.iter().all(|path| {
                            (separate_cache_directory && path.starts_with(&cache))
                                || crate::util::is_index_cache_file(path, &cache)
                        }) =>
                {
                    return;
                }
                Ok(event) if matches!(event.kind, EventKind::Access(_)) => return,
                Err(error) => {
                    callback_state.diagnose(format!("watch error; reconciling root: {error}"))
                }
                Ok(_) => {}
            }
            // One pending dirty notification is enough: the worker performs a full policy-aware scan.
            match callback_sender.try_send(Signal::Dirty) {
                Ok(())
                | Err(mpsc::TrySendError::Full(_))
                | Err(mpsc::TrySendError::Disconnected(_)) => {}
            }
        },
        Config::default(),
    )
    .map_err(io::Error::other)?;
    watcher
        .watch(root, RecursiveMode::Recursive)
        .map_err(io::Error::other)?;
    let stopped = Arc::new(AtomicBool::new(false));
    let worker_stop = stopped.clone();
    let root = root.to_path_buf();
    let thread = thread::Builder::new()
        .name("codesearch-index".into())
        .spawn(move || update_loop(root, shared, receiver, &worker_stop))?;
    Ok(WatchWorker {
        watcher: Some(watcher),
        sender,
        stopped,
        thread: Some(thread),
    })
}

fn update_loop(
    root: PathBuf,
    shared: Arc<SharedIndex>,
    receiver: Receiver<Signal>,
    stopped: &AtomicBool,
) {
    while !stopped.load(Ordering::Acquire) {
        match receiver.recv() {
            Ok(Signal::Dirty) => {}
            Ok(Signal::Shutdown) | Err(_) => break,
        }
        if !coalesce(&receiver, stopped) {
            break;
        }
        let cancelled = || stopped.load(Ordering::Acquire);
        match collect_paths_with_cancel(&root, &cancelled) {
            Ok(Some(paths)) if !cancelled() => {
                shared.publish(IndexArena::from_paths(root.clone(), paths), false)
            }
            Ok(_) => {}
            Err(error) => shared.diagnose(format!("watch reconciliation failed: {error}")),
        }
    }
}

fn coalesce(receiver: &Receiver<Signal>, stopped: &AtomicBool) -> bool {
    let started = Instant::now();
    while !stopped.load(Ordering::Acquire) && started.elapsed() < MAX_COALESCE {
        match receiver.recv_timeout(DEBOUNCE) {
            Ok(Signal::Dirty) => {}
            Ok(Signal::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => return false,
            Err(mpsc::RecvTimeoutError::Timeout) => return true,
        }
    }
    !stopped.load(Ordering::Acquire)
}

impl Drop for WatchWorker {
    fn drop(&mut self) {
        self.watcher.take();
        self.stopped.store(true, Ordering::Release);
        match self.sender.try_send(Signal::Shutdown) {
            Ok(())
            | Err(mpsc::TrySendError::Full(_))
            | Err(mpsc::TrySendError::Disconnected(_)) => {}
        }
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            eprintln!("index worker panicked during shutdown");
        }
    }
}
