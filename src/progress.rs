use std::sync::{
    atomic::{self, AtomicU64},
    Arc, OnceLock,
};

/// Process-global notifier fired after every progress mutation. The GUI binary
/// wires this to `fltk::app::awake`; headless mode leaves it unset (pure atomics).
static NOTIFIER: OnceLock<fn()> = OnceLock::new();

pub fn set_notifier(f: fn()) {
    let _ = NOTIFIER.set(f);
}

fn notify() {
    if let Some(f) = NOTIFIER.get() {
        f();
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(usize)]
pub enum ProgressStage {
    None,
    Start,
    FetchingMetadata,
    UpdatingUpdater,
    CheckingFiles,
    DownloadingUpdates,
    VerifyingFiles,
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
            6 => ProgressStage::VerifyingFiles,
            7 => ProgressStage::Play,
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
        notify();
    }

    pub fn set_current_progress(&self, val: u64) {
        self.current_progress.store(val, atomic::Ordering::Relaxed);
        notify();
    }

    pub fn increment_progress(&self, val: u64) {
        self.current_progress
            .fetch_add(val, atomic::Ordering::Relaxed);
        notify();
    }

    pub fn set_stage(&self, val: ProgressStage) {
        self.stage.store(val as u64, atomic::Ordering::Relaxed);
        notify();
    }

    pub fn current_stage(&self) -> ProgressStage {
        ProgressStage::from(self.stage.load(atomic::Ordering::Relaxed) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setters_dont_panic_without_notifier() {
        // No notifier need be set for these to be safe (pure atomics).
        let state = ProgressState::default();
        state.set_max_progress(10);
        state.set_current_progress(5);
        state.increment_progress(1);
        state.set_stage(ProgressStage::CheckingFiles);
        assert_eq!(state.current_progress(), 6);
        assert_eq!(state.current_stage(), ProgressStage::CheckingFiles);
    }

    static NOTIFY_COUNT: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn notifier_fires_when_set() {
        // set_notifier is only called here in the lib test binary, so this
        // OnceLock::set wins. The notifier bumps a global counter.
        set_notifier(|| {
            NOTIFY_COUNT.fetch_add(1, atomic::Ordering::Relaxed);
        });
        let before = NOTIFY_COUNT.load(atomic::Ordering::Relaxed);
        ProgressState::default().set_stage(ProgressStage::Play);
        assert!(NOTIFY_COUNT.load(atomic::Ordering::Relaxed) > before);
    }
}
