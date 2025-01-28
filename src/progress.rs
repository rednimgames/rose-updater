use fltk::app;

use std::sync::{
    atomic::{self, AtomicU64},
    Arc,
};

#[derive(Clone)]
pub struct ProgressState {
    current_progress: Arc<AtomicU64>,
    max_progress: Arc<AtomicU64>,
}

impl Default for ProgressState {
    fn default() -> Self {
        Self {
            current_progress: Arc::new(AtomicU64::new(0)),
            max_progress: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl ProgressState {
    pub fn max_progress(&self) -> u64 {
        self.max_progress.load(atomic::Ordering::Relaxed)
    }

    pub fn current_progress(&self) -> u64 {
        self.current_progress.load(atomic::Ordering::Relaxed)
    }

    pub fn set_max_progress(&self, val: u64) {
        self.max_progress.store(val, atomic::Ordering::Relaxed);
        app::awake();
    }

    pub fn increment_progress(&self, val: u64) {
        self.current_progress
            .fetch_add(val, atomic::Ordering::Relaxed);
        app::awake();
    }
}
