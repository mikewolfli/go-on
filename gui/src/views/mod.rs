use std::sync::mpsc;

/// Send a message over a SyncSender with a single try_send attempt.
/// If the channel is full the message is silently dropped — the next poll cycle
/// will fetch fresh state from the backend.
///
/// No retry loop with thread::sleep is used to avoid blocking the UI thread.
pub(crate) fn send_with_retry(tx: &mpsc::SyncSender<String>, msg: String) {
    if tx.try_send(msg).is_err() {
        eprintln!("WARN: channel full — message dropped");
    }
}

pub mod about;
pub mod autotune;
pub mod chat;
pub mod config_editor;
pub mod monitor;
pub mod prompts;
pub mod providers;
pub mod risk_decision;
pub mod security;
pub mod security_prefs;
pub mod settings;
pub mod setup;
pub mod skills;
pub mod ui_state;
pub mod workflow;
