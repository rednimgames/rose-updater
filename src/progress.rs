use fltk::app;

use std::sync::{
    atomic::{self, AtomicU64},
    Arc,
};

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(usize)]
pub enum ProgressStage {
    None,
    Start,
    FetchingMetadata,
    UpdatingUpdater,
    CheckingFiles,
    DownloadingUpdates,
    Play,
}

impl From<usize> for ProgressStage {
    fn from(value: usize) -> Self {
        match value {
            1 => ProgressStage::Start,
            2 => ProgressStage::FetchingMetadata,
            3 => ProgressStage::UpdatingUpdater,
            4 => ProgressStage::CheckingFiles,
            5 => ProgressStage::DownloadingUpdates,
            6 => ProgressStage::Play,
            _ => ProgressStage::None,
        }
    }
}

#[derive(Clone)]
pub struct ProgressState {
    current_progress: Arc<AtomicU64>,
    max_progress: Arc<AtomicU64>,
    stage: Arc<AtomicU64>,
}

impl Default for ProgressState {
    fn default() -> Self {
        Self {
            current_progress: Arc::new(AtomicU64::new(0)),
            max_progress: Arc::new(AtomicU64::new(0)),
            stage: Arc::new(AtomicU64::new(ProgressStage::Start as u64)),
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

    pub fn set_current_progress(&self, val: u64) {
        self.current_progress.store(val, atomic::Ordering::Relaxed);
        app::awake();
    }

    pub fn increment_progress(&self, val: u64) {
        self.current_progress
            .fetch_add(val, atomic::Ordering::Relaxed);
        app::awake();
    }

    pub fn set_stage(&self, val: ProgressStage) {
        self.stage.store(val as u64, atomic::Ordering::Relaxed);
        app::awake();
    }

    pub fn current_stage(&self) -> ProgressStage {
        ProgressStage::from(self.stage.load(atomic::Ordering::Relaxed) as usize)
    }
}
