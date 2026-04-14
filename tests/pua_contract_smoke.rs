#[path = "../src/governance/pua.rs"]
mod pua;
#[path = "../src/orchestration/roles.rs"]
mod roles;

use pua::{quality_compass, PuaEnforcementPlan};

fn parse_escalation_level(level: &str) -> u8 {
    level
        .strip_prefix('L')
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(u8::MAX)
}

#[test]
fn pua_default_plan_has_sane_escalation_level() {
    let plan = PuaEnforcementPlan::default();
    let escalation_level = parse_escalation_level(&plan.escalation_level);

    assert!(
        escalation_level <= 5,
        "escalation_level 超出预期范围: {}",
        plan.escalation_level
    );
    assert!(
        !plan.red_lines.is_empty(),
        "default PUA plan red_lines 不得为空"
    );
}

#[test]
fn quality_compass_returns_non_empty_checks() {
    let checks = quality_compass();

    assert!(!checks.is_empty(), "quality_compass() 不得为空");
    assert!(checks.len() >= 3, "quality_compass() 至少应有 3 项检查");
}

#[test]
fn pua_default_plan_keeps_quality_compass_contract() {
    let plan = PuaEnforcementPlan::default();

    assert_eq!(plan.quality_compass, quality_compass());
    assert!(plan
        .quality_compass
        .iter()
        .all(|item| !item.trim().is_empty()));
}
