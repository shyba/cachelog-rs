#![cfg(not(feature = "loom"))]

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub struct BackgroundFlushConfig {
    pub trigger_dirty: usize,
    pub flush_limit: usize,
    pub check_interval: Duration,
}

impl BackgroundFlushConfig {
    pub const fn new(trigger_dirty: usize, flush_limit: usize, check_interval: Duration) -> Self {
        Self {
            trigger_dirty,
            flush_limit,
            check_interval,
        }
    }

    pub fn for_batch_size(batch_size: usize) -> Self {
        let trigger_dirty = batch_size.max(1);
        let flush_limit = 512;
        let check_interval = if trigger_dirty >= 256 {
            Duration::from_secs(10)
        } else {
            Duration::from_millis(100)
        };
        Self::new(trigger_dirty, flush_limit, check_interval)
    }
}

impl Default for BackgroundFlushConfig {
    fn default() -> Self {
        Self {
            trigger_dirty: 1024,
            flush_limit: 512,
            check_interval: Duration::from_millis(100),
        }
    }
}

const WAKE_IDLE: u8 = 0;
const WAKE_PENDING: u8 = 1;
const WAKE_ACTIVE: u8 = 2;

#[derive(Default)]
struct WorkerControl {
    wake_pending: bool,
    force_flush: bool,
    shutdown: bool,
    pending_sync: u64,
    completed_sync: u64,
    last_sync_result: Option<Result<(), String>>,
}

pub(crate) struct BackgroundFlushService {
    pub(crate) dirty_len: Arc<dyn Fn() -> usize + Send + Sync>,
    pub(crate) flush: Arc<dyn Fn(usize) -> Result<(), String> + Send + Sync>,
}

struct WorkerLoop {
    control: Arc<(Mutex<WorkerControl>, Condvar)>,
    running: Arc<AtomicBool>,
    wake_state: Arc<AtomicU8>,
    trigger_dirty: Arc<AtomicUsize>,
    flush_limit: Arc<AtomicUsize>,
    backlog_hint: Arc<AtomicUsize>,
    dirty_len: Arc<dyn Fn() -> usize + Send + Sync>,
    flush: Arc<dyn Fn(usize) -> Result<(), String> + Send + Sync>,
    check_interval: Duration,
}

pub struct BackgroundFlushHandle {
    control: Arc<(Mutex<WorkerControl>, Condvar)>,
    join: Mutex<Option<JoinHandle<Result<(), String>>>>,
    running: Arc<AtomicBool>,
    wake_state: Arc<AtomicU8>,
    trigger_dirty: Arc<AtomicUsize>,
    flush_limit: Arc<AtomicUsize>,
    backlog_hint: Arc<AtomicUsize>,
    dirty_len: Arc<dyn Fn() -> usize + Send + Sync>,
}

impl BackgroundFlushHandle {
    fn shutdown_error() -> String {
        "Background flush service is shutting down".to_string()
    }

    pub(crate) fn start_with_service(
        config: BackgroundFlushConfig,
        service: BackgroundFlushService,
    ) -> Self {
        let control = Arc::new((Mutex::new(WorkerControl::default()), Condvar::new()));
        let running = Arc::new(AtomicBool::new(true));
        let wake_state = Arc::new(AtomicU8::new(WAKE_IDLE));
        let trigger_dirty = Arc::new(AtomicUsize::new(config.trigger_dirty.max(1)));
        let flush_limit = Arc::new(AtomicUsize::new(config.flush_limit.max(1)));
        let BackgroundFlushService { dirty_len, flush } = service;
        let backlog_hint = Arc::new(AtomicUsize::new(dirty_len()));
        let check_interval = config.check_interval;

        let worker = WorkerLoop {
            control: control.clone(),
            running: running.clone(),
            wake_state: wake_state.clone(),
            trigger_dirty: trigger_dirty.clone(),
            flush_limit: flush_limit.clone(),
            backlog_hint: backlog_hint.clone(),
            dirty_len: dirty_len.clone(),
            flush,
            check_interval,
        };

        let join = thread::spawn(move || worker_loop(worker));

        Self {
            control,
            join: Mutex::new(Some(join)),
            running,
            wake_state,
            trigger_dirty,
            flush_limit,
            backlog_hint,
            dirty_len,
        }
    }

    pub fn note_writes(&self, writes_added: usize) -> Result<(), String> {
        if writes_added == 0 {
            return Ok(());
        }
        if !self.running.load(Ordering::Acquire) {
            return Err(Self::shutdown_error());
        }
        let trigger = self.trigger_dirty.load(Ordering::Acquire).max(1);
        let dirty_len = self
            .backlog_hint
            .fetch_add(writes_added, Ordering::Relaxed)
            .saturating_add(writes_added);
        if dirty_len < trigger {
            return Ok(());
        }
        if self
            .wake_state
            .compare_exchange(WAKE_IDLE, WAKE_PENDING, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(());
        }
        let (lock, cvar) = &*self.control;
        let mut state = lock
            .lock()
            .map_err(|_| "Failed to lock background flush state".to_string())?;
        if state.shutdown {
            self.wake_state.store(WAKE_IDLE, Ordering::Release);
            return Err(Self::shutdown_error());
        }
        state.wake_pending = true;
        cvar.notify_one();
        Ok(())
    }

    /// Ask the background worker to drain pending dirty entries even if the
    /// current backlog is below the automatic wake threshold.
    pub fn request_flush(&self) -> Result<(), String> {
        if !self.running.load(Ordering::Acquire) {
            return Err(Self::shutdown_error());
        }
        let (lock, cvar) = &*self.control;
        let mut state = lock
            .lock()
            .map_err(|_| "Failed to lock background flush state".to_string())?;
        if state.shutdown {
            return Err(Self::shutdown_error());
        }
        state.force_flush = true;
        cvar.notify_one();
        Ok(())
    }

    /// Force a flush cycle and wait until the worker has completed it.
    ///
    /// This returns the worker's last flush result, including persist errors
    /// and shutdown failures.
    pub fn flush_sync(&self) -> Result<(), String> {
        let (lock, cvar) = &*self.control;
        let mut state = lock
            .lock()
            .map_err(|_| "Failed to lock background flush state".to_string())?;
        if state.shutdown || !self.running.load(Ordering::Acquire) {
            return state
                .last_sync_result
                .clone()
                .unwrap_or_else(|| Err(Self::shutdown_error()));
        }
        state.pending_sync = state.pending_sync.wrapping_add(1);
        let target = state.pending_sync;
        state.force_flush = true;
        cvar.notify_one();
        while state.completed_sync < target && !state.shutdown {
            state = cvar
                .wait(state)
                .map_err(|_| "Condvar wait failed".to_string())?;
        }
        state
            .last_sync_result
            .clone()
            .unwrap_or_else(|| Err(Self::shutdown_error()))
    }

    /// Run a scan against a persisted-consistent view.
    ///
    /// If dirty entries are pending, this synchronously flushes them first.
    /// Persist errors and shutdown are surfaced unchanged to the caller.
    pub fn with_persisted_scan<R>(
        &self,
        scan: impl FnOnce() -> Result<R, String>,
    ) -> Result<R, String> {
        if (self.dirty_len)() == 0 {
            return scan();
        }
        self.flush_sync()?;
        scan()
    }

    pub fn reconfigure(&self, trigger_dirty: usize, flush_limit: usize) -> Result<(), String> {
        let new_trigger = trigger_dirty.max(1);
        let old_trigger = self.trigger_dirty.swap(new_trigger, Ordering::AcqRel);
        self.flush_limit
            .store(flush_limit.max(1), Ordering::Release);
        if !self.running.load(Ordering::Acquire) {
            return Err(Self::shutdown_error());
        }

        let current_dirty = self
            .backlog_hint
            .load(Ordering::Acquire)
            .max((self.dirty_len)());
        if new_trigger < old_trigger && current_dirty >= new_trigger {
            self.request_flush()?;
        }
        Ok(())
    }

    pub fn reconfigure_for_batch_size(&self, batch_size: usize) -> Result<(), String> {
        let config = BackgroundFlushConfig::for_batch_size(batch_size);
        self.reconfigure(config.trigger_dirty, config.flush_limit)
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Stop the background worker and surface any final flush error.
    ///
    /// All cloned handles observe shutdown after this returns and subsequent
    /// flush requests fail fast instead of blocking.
    pub fn shutdown(&self) -> Result<(), String> {
        self.running.store(false, Ordering::Release);
        let (lock, cvar) = &*self.control;
        if let Ok(mut state) = lock.lock() {
            state.shutdown = true;
            state.completed_sync = state.pending_sync;
            state.last_sync_result = Some(Err(Self::shutdown_error()));
            cvar.notify_one();
            cvar.notify_all();
        }
        if let Ok(mut join) = self.join.lock()
            && let Some(handle) = join.take()
        {
            return handle
                .join()
                .map_err(|_| "Background flush worker panicked".to_string())?;
        }
        Ok(())
    }
}

impl Drop for BackgroundFlushHandle {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn drain_limit(flush_limit: usize) -> usize {
    flush_limit.max(1)
}

fn worker_loop(worker: WorkerLoop) -> Result<(), String> {
    let WorkerLoop {
        control,
        running,
        wake_state,
        trigger_dirty,
        flush_limit,
        backlog_hint,
        dirty_len,
        flush,
        check_interval,
    } = worker;
    let (lock, cvar) = &*control;
    let mut state = match lock.lock() {
        Ok(state) => state,
        Err(_) => return Err("Failed to lock background flush state".to_string()),
    };

    loop {
        let trigger = trigger_dirty.load(Ordering::Acquire).max(1);
        while !state.shutdown
            && !state.force_flush
            && !state.wake_pending
            && state.pending_sync == state.completed_sync
            && backlog_hint.load(Ordering::Acquire) < trigger
        {
            if wake_state
                .compare_exchange(WAKE_ACTIVE, WAKE_IDLE, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                break;
            }
            let waited = match cvar.wait_timeout(state, check_interval) {
                Ok(waited) => waited,
                Err(_) => return Err("Failed to lock background flush state".to_string()),
            };
            state = waited.0;
            if waited.1.timed_out() {
                backlog_hint.store(dirty_len(), Ordering::Release);
            }
        }

        if state.shutdown {
            drop(state);
            let limit = drain_limit(flush_limit.load(Ordering::Acquire));
            let result = flush(limit);
            backlog_hint.store(dirty_len(), Ordering::Release);
            wake_state.store(WAKE_IDLE, Ordering::Release);
            running.store(false, Ordering::Release);
            return result;
        }

        wake_state.store(WAKE_ACTIVE, Ordering::Release);
        let target_sync = state.pending_sync;
        let needs_flush = state.force_flush
            || state.wake_pending
            || target_sync > state.completed_sync
            || backlog_hint.load(Ordering::Acquire) >= trigger;
        state.force_flush = false;
        state.wake_pending = false;
        drop(state);

        let result = if needs_flush {
            let limit = drain_limit(flush_limit.load(Ordering::Acquire));
            let result = flush(limit);
            backlog_hint.store(dirty_len(), Ordering::Release);
            result
        } else {
            Ok(())
        };

        state = match lock.lock() {
            Ok(state) => state,
            Err(_) => return Err("Failed to lock background flush state".to_string()),
        };
        if result.is_err() {
            state.shutdown = true;
            state.completed_sync = state.pending_sync;
            state.last_sync_result = Some(result.clone());
            wake_state.store(WAKE_IDLE, Ordering::Release);
            running.store(false, Ordering::Release);
            cvar.notify_all();
            return result;
        }
        if target_sync > state.completed_sync {
            state.completed_sync = target_sync;
            state.last_sync_result = Some(result.clone());
            cvar.notify_all();
        }
    }
}
