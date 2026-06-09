//! GAP-B53-56: Cross-session metacognitive learning persistence.
//!
//! Provides persistent storage for metacognitive learnings so that
//! corrective actions, observations, and reflection reports survive
//! process restarts and span multiple sessions.

use crate::intelligence::metacognitive::{
    CorrectiveAction, ExecutionObservation, MetacognitiveConfig, MetacognitiveController,
    ReflectionReport,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Serialisable snapshot of metacognitive state for disk persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetacognitiveSnapshot {
    pub observations: Vec<ExecutionObservation>,
    pub actions: Vec<CorrectiveAction>,
    pub reports: Vec<ReflectionReport>,
    pub config: MetacognitiveConfig,
    pub saved_at_ms: u64,
}

/// Handles persisting and restoring metacognitive state across sessions.
pub struct MetacognitivePersistence {
    /// Directory where snapshot files are stored.
    storage_dir: PathBuf,
}

impl MetacognitivePersistence {
    /// Create a new persistence handler rooted at `storage_dir`.
    /// The directory is created if it does not exist.
    pub fn new(storage_dir: PathBuf) -> std::io::Result<Self> {
        if !storage_dir.exists() {
            fs::create_dir_all(&storage_dir)?;
        }
        Ok(Self { storage_dir })
    }

    /// Path to the metacognitive snapshot file.
    fn snapshot_path(&self) -> PathBuf {
        self.storage_dir.join("metacognitive_snapshot.json")
    }

    /// Save the current metacognitive controller state to disk.
    pub fn save(&self, controller: &MetacognitiveController) -> std::io::Result<()> {
        let config = MetacognitiveConfig::default();
        let observations = controller.list_observations(false);
        let actions = controller.list_actions(None);
        let reports = controller.list_reports();

        let snapshot = MetacognitiveSnapshot {
            observations,
            actions,
            reports,
            config,
            saved_at_ms: crate::intelligence::now_ms(),
        };

        let json = serde_json::to_string_pretty(&snapshot).map_err(std::io::Error::other)?;

        // Write atomically via a temp file, then rename.
        let tmp_path = self.storage_dir.join("metacognitive_snapshot.tmp");
        fs::write(&tmp_path, &json)?;
        fs::rename(&tmp_path, self.snapshot_path())?;

        Ok(())
    }

    /// Load and restore metacognitive state from disk.
    /// Returns `None` if no snapshot exists or it is corrupt.
    pub fn load(&self) -> Option<MetacognitiveSnapshot> {
        let path = self.snapshot_path();
        if !path.exists() {
            return None;
        }

        let content = fs::read_to_string(&path).ok()?;
        serde_json::from_str::<MetacognitiveSnapshot>(&content).ok()
    }

    /// Restore the state from a snapshot into a new MetacognitiveController.
    /// Observations, actions, and reports are replayed into the controller.
    pub fn restore_into_controller(
        &self,
        controller: &MetacognitiveController,
    ) -> std::io::Result<usize> {
        let snapshot = match self.load() {
            Some(s) => s,
            None => return Ok(0),
        };

        let mut restored_count = 0;

        // Replay observations.
        for obs in &snapshot.observations {
            let result = controller.record_observation(
                &obs.task_id,
                &obs.agent,
                &obs.observation_type,
                &obs.severity,
                &obs.description,
            );
            if let Ok(id) = result {
                if obs.is_resolved {
                    let _ = controller.resolve_observation(&id);
                }
                restored_count += 1;
            }
        }

        Ok(restored_count)
    }

    /// Check if a saved snapshot exists on disk.
    pub fn has_saved_state(&self) -> bool {
        self.snapshot_path().exists()
    }

    /// Remove the saved snapshot from disk.
    #[allow(dead_code)] // F-GAP-49: reserved for metacognitive snapshot cleanup
    pub fn clear(&self) -> std::io::Result<()> {
        let path = self.snapshot_path();
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_controller() -> MetacognitiveController {
        MetacognitiveController::new(MetacognitiveConfig::default())
    }

    #[test]
    fn test_persistence_save_and_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let persistence = MetacognitivePersistence::new(tmp.path().to_path_buf()).unwrap();

        let controller = make_controller();
        controller
            .record_observation("task-1", "agent-a", "error", "high", "Timeout")
            .unwrap();
        controller
            .record_observation("task-2", "agent-b", "latency", "medium", "Slow")
            .unwrap();

        persistence.save(&controller).unwrap();
        assert!(persistence.has_saved_state());

        let snapshot = persistence.load().unwrap();
        assert_eq!(snapshot.observations.len(), 2);
        assert_eq!(snapshot.observations[0].task_id, "task-1");
        assert_eq!(snapshot.observations[1].task_id, "task-2");
    }

    #[test]
    fn test_persistence_restore_into_controller() {
        let tmp = TempDir::new().unwrap();
        let persistence = MetacognitivePersistence::new(tmp.path().to_path_buf()).unwrap();

        let controller = make_controller();
        controller
            .record_observation("task-x", "agent-x", "error", "high", "X")
            .unwrap();

        persistence.save(&controller).unwrap();

        let restored = make_controller();
        let count = persistence.restore_into_controller(&restored).unwrap();
        assert!(count > 0, "Should restore at least one observation");
    }

    #[test]
    fn test_persistence_no_saved_state() {
        let tmp = TempDir::new().unwrap();
        let persistence = MetacognitivePersistence::new(tmp.path().to_path_buf()).unwrap();
        assert!(!persistence.has_saved_state());
        assert!(persistence.load().is_none());
    }

    #[test]
    fn test_persistence_clear() {
        let tmp = TempDir::new().unwrap();
        let persistence = MetacognitivePersistence::new(tmp.path().to_path_buf()).unwrap();
        let controller = make_controller();
        controller
            .record_observation("task-1", "a", "error", "low", "E1")
            .unwrap();
        persistence.save(&controller).unwrap();
        assert!(persistence.has_saved_state());
        persistence.clear().unwrap();
        assert!(!persistence.has_saved_state());
    }
}
