use std::collections::HashMap;
use std::sync::OnceLock;

use crate::intelligence::metacognitive::{MetacognitiveConfig, MetacognitiveController};
use crate::intelligence::self_model::{SelfModelConfig, SelfModelCore};
use crate::intelligence::world_model::{EntityType, WorldModel, WorldModelConfig};

pub(crate) struct ExecutionPreCheck {
    pub should_degrade: bool,
    pub reason: Option<String>,
}

static META_CTRL: OnceLock<MetacognitiveController> = OnceLock::new();
static WORLD_MODEL: OnceLock<WorldModel> = OnceLock::new();
static SELF_MODEL: OnceLock<SelfModelCore> = OnceLock::new();

fn metacognitive() -> &'static MetacognitiveController {
    META_CTRL.get_or_init(|| MetacognitiveController::new(MetacognitiveConfig::default()))
}

fn world_model() -> &'static WorldModel {
    WORLD_MODEL.get_or_init(|| WorldModel::new(WorldModelConfig::default()))
}

fn self_model() -> &'static SelfModelCore {
    SELF_MODEL.get_or_init(|| SelfModelCore::new(SelfModelConfig::default()))
}

pub(crate) fn should_degrade(limitations_count: usize) -> bool {
    limitations_count > 2000
}

pub(crate) fn pre_check(task_id: &str, agent: &str) -> ExecutionPreCheck {
    let world = world_model();
    let self_profile = self_model().profile();

    let should_degrade = should_degrade(self_profile.limitations_count);
    let reason = if should_degrade {
        Some("self_model_limitations_overflow".to_string())
    } else {
        None
    };

    let mut payload = HashMap::new();
    payload.insert("task_id".to_string(), task_id.to_string());
    payload.insert("agent".to_string(), agent.to_string());
    payload.insert("phase".to_string(), "pre_check".to_string());
    let _ = world.record_event("autonomy_precheck", "execution_intelligence", payload);

    ExecutionPreCheck {
        should_degrade,
        reason,
    }
}

pub(crate) fn post_check(task_id: &str, agent: &str, success: bool, summary: &str) {
    let world = world_model();
    let _ = world.register_entity(&format!("autonomy-task-{}", task_id), EntityType::System);

    let mut payload = HashMap::new();
    payload.insert("task_id".to_string(), task_id.to_string());
    payload.insert("agent".to_string(), agent.to_string());
    payload.insert("success".to_string(), success.to_string());
    payload.insert("summary".to_string(), summary.to_string());
    let _ = world.record_event("autonomy_postcheck", "execution_intelligence", payload);

    if !success {
        if let Ok(_id) =
            metacognitive().record_observation(task_id, agent, "tool_execution", "high", summary)
        {
            let _ = metacognitive().autoreflect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::should_degrade;

    #[test]
    fn degrade_threshold_is_applied() {
        assert!(!should_degrade(2000));
        assert!(should_degrade(2001));
    }
}
