//! Headless mode: run the update/verify work with no window and report progress
//! as newline-delimited JSON (NDJSON) on stdout. Each line is one `Event`. This
//! is a cross-repo wire contract — the exact JSON strings are locked by tests.

use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

use serde::Serialize;

use rose_update::progress::{ProgressStage, ProgressState};

use crate::{run_verify_process, update_process, Args, UpdateProcessResult};

#[derive(Serialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
enum Event {
    Progress {
        stage: &'static str,
        current: u64,
        max: u64,
    },
    Done {
        #[serde(skip_serializing_if = "Option::is_none")]
        repaired: Option<usize>,
    },
    UpdaterUpdated,
    Error {
        code: Option<u16>,
        message: String,
        details: String,
    },
}

/// Kebab-case wire name for each stage. Exhaustive so a new `ProgressStage`
/// variant is a compile error until it gets a string here.
fn stage_str(stage: ProgressStage) -> &'static str {
    match stage {
        ProgressStage::None => "none",
        ProgressStage::Start => "start",
        ProgressStage::FetchingMetadata => "fetching-metadata",
        ProgressStage::UpdatingUpdater => "updating-updater",
        ProgressStage::CheckingFiles => "checking-files",
        ProgressStage::DownloadingUpdates => "downloading-updates",
        ProgressStage::VerifyingFiles => "verifying-files",
        ProgressStage::Play => "play",
    }
}

fn emit(event: &Event) {
    if let Ok(line) = serde_json::to_string(event) {
        println!("{line}");
        let _ = std::io::stdout().flush();
    }
}

/// Best-effort extraction of the numeric code from a "[ROSE-NNN]" prefix
/// anywhere in the error chain.
fn parse_error_code(text: &str) -> Option<u16> {
    let start = text.find("[ROSE-")? + "[ROSE-".len();
    let rest = &text[start..];
    let end = rest.find(']')?;
    rest[..end].parse().ok()
}

/// Pure change-detector for the progress poller: emits a `Progress` event only
/// when the (current, max, stage) snapshot differs from the last one seen.
struct EmitTracker {
    last: Option<(u64, u64, ProgressStage)>,
}

impl EmitTracker {
    fn observe(&mut self, current: u64, max: u64, stage: ProgressStage) -> Option<Event> {
        let snapshot = (current, max, stage);
        if self.last == Some(snapshot) {
            return None;
        }
        self.last = Some(snapshot);
        Some(Event::Progress {
            stage: stage_str(stage),
            current,
            max,
        })
    }
}

pub(crate) async fn run_headless(args: Args) -> ExitCode {
    // No notifier is set, so ProgressState is pure atomics polled below.
    let progress_state = ProgressState::default();

    // Immediate handshake: also doubles as the "headless supported" signal.
    emit(&Event::Progress {
        stage: "start",
        current: 0,
        max: 0,
    });

    // Emitter: poll the atomics at 10 Hz, emit only on change. Sub-100ms
    // transient stages may be skipped (harmless for a progress UI).
    let emitter = {
        let progress_state = progress_state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            let mut tracker = EmitTracker { last: None };
            loop {
                interval.tick().await;
                if let Some(event) = tracker.observe(
                    progress_state.current_progress(),
                    progress_state.max_progress(),
                    progress_state.current_stage(),
                ) {
                    emit(&event);
                }
            }
        })
    };

    let result = if args.verify {
        run_verify_process(
            &args.url,
            &args.manifest_name,
            &args.output,
            progress_state.clone(),
        )
        .await
        .map(Outcome::Verified)
    } else {
        update_process(&args, progress_state.clone())
            .await
            .map(Outcome::Update)
    };

    emitter.abort();

    // One final snapshot so consumers see the terminal progress values.
    emit(&Event::Progress {
        stage: stage_str(progress_state.current_stage()),
        current: progress_state.current_progress(),
        max: progress_state.max_progress(),
    });

    match result {
        Ok(Outcome::Update(UpdateProcessResult::ApplicationUpdated)) => {
            emit(&Event::Done { repaired: None });
            ExitCode::SUCCESS
        }
        Ok(Outcome::Verified(repaired)) => {
            emit(&Event::Done {
                repaired: Some(repaired),
            });
            ExitCode::SUCCESS
        }
        Ok(Outcome::Update(UpdateProcessResult::UpdaterUpdated)) => {
            emit(&Event::UpdaterUpdated);
            // Caller respawns us; a detached self-respawn would orphan its pipe.
            ExitCode::from(10)
        }
        Err(err) => {
            let details = format!("{err:#}");
            let code = parse_error_code(&details);
            emit(&Event::Error {
                code,
                message: err.to_string(),
                details,
            });
            ExitCode::FAILURE
        }
    }
}

enum Outcome {
    Update(UpdateProcessResult),
    Verified(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_wire_format() {
        // Exact wire strings — this is the cross-repo contract.
        assert_eq!(
            serde_json::to_string(&Event::Progress {
                stage: "checking-files",
                current: 3,
                max: 10,
            })
            .unwrap(),
            r#"{"event":"progress","stage":"checking-files","current":3,"max":10}"#
        );
        assert_eq!(
            serde_json::to_string(&Event::Done { repaired: None }).unwrap(),
            r#"{"event":"done"}"#
        );
        assert_eq!(
            serde_json::to_string(&Event::Done { repaired: Some(2) }).unwrap(),
            r#"{"event":"done","repaired":2}"#
        );
        assert_eq!(
            serde_json::to_string(&Event::UpdaterUpdated).unwrap(),
            r#"{"event":"updater-updated"}"#
        );
        assert_eq!(
            serde_json::to_string(&Event::Error {
                code: Some(101),
                message: "boom".into(),
                details: "chain".into(),
            })
            .unwrap(),
            r#"{"event":"error","code":101,"message":"boom","details":"chain"}"#
        );
        assert_eq!(
            serde_json::to_string(&Event::Error {
                code: None,
                message: "boom".into(),
                details: "chain".into(),
            })
            .unwrap(),
            r#"{"event":"error","code":null,"message":"boom","details":"chain"}"#
        );
    }

    #[test]
    fn stage_strings_are_kebab_case() {
        assert_eq!(stage_str(ProgressStage::None), "none");
        assert_eq!(stage_str(ProgressStage::Start), "start");
        assert_eq!(stage_str(ProgressStage::FetchingMetadata), "fetching-metadata");
        assert_eq!(stage_str(ProgressStage::UpdatingUpdater), "updating-updater");
        assert_eq!(stage_str(ProgressStage::CheckingFiles), "checking-files");
        assert_eq!(
            stage_str(ProgressStage::DownloadingUpdates),
            "downloading-updates"
        );
        assert_eq!(stage_str(ProgressStage::VerifyingFiles), "verifying-files");
        assert_eq!(stage_str(ProgressStage::Play), "play");
    }

    #[test]
    fn emit_tracker_emits_on_change_only() {
        let mut tracker = EmitTracker { last: None };
        // First observation always emits.
        assert!(tracker.observe(0, 0, ProgressStage::Start).is_some());
        // Identical repeat: no emit.
        assert!(tracker.observe(0, 0, ProgressStage::Start).is_none());
        // Stage-only change: emit.
        assert!(tracker.observe(0, 0, ProgressStage::CheckingFiles).is_some());
        // Number-only change: emit.
        assert!(tracker.observe(5, 0, ProgressStage::CheckingFiles).is_some());
        assert!(tracker.observe(5, 10, ProgressStage::CheckingFiles).is_some());
    }

    #[test]
    fn parses_rose_error_code() {
        assert_eq!(parse_error_code("[ROSE-101] something failed"), Some(101));
        assert_eq!(parse_error_code("wrapper: [ROSE-203] nested"), Some(203));
        assert_eq!(parse_error_code("no code here"), None);
    }
}
