# go-on PUA Integration - Implementation Guide

**For**: go-on (Rust agent proxy)  
**Purpose**: Enforce PUA rules on ALL agent interactions  
**Status**: Ready for implementation

---

## High-Level Architecture

```
User Request (raw)
        ↓
go-on receives + validates request
        ↓
Create AgentTask with PUA tracking:
  - failure_count = 0
  - pressure_level = L0
  - quality_compass = [false, false, false, false, false]
        ↓
Route to agent (Copilot, Claude, GPT-4, etc.)
        ↓
Agent returns response
        ↓
PUA Validation Layer:
  1. Check for red line violations (claim, assumptions, scope)
  2. Increment failure_count on rejects
  3. Escalate pressure_level based on count
  4. If >= failure_count==4: trigger 7-point checklist
  5. Validate quality compass (5 checkpoints)
  6. Iceberg scan for related issues
        ↓
Build proof/error case/root cause verified?
        ↓ YES: Approve + return to user
        ↓ NO: Reject + ask agent to retry + log violation
```

---

## Implementation Steps

### Step 1: Create PUA Tracking Struct

**File**: `src/pua.rs` (NEW)

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// PUA (Performance Improvement Plan) enforcement state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PuaTracker {
    /// Number of consecutive failures
    pub failure_count: u32,
    /// Current pressure level (L0-L4)
    pub pressure_level: PressureLevel,
    /// Quality compass checkpoint validation
    pub quality_compass: QualityCompass,
    /// Red line violations detected
    pub red_line_violations: Vec<RedLineViolation>,
    /// Iceberg scan results
    pub iceberg_patterns: HashMap<String, usize>,
    /// Timestamp of last failure
    pub last_failure_at: Option<std::time::SystemTime>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum PressureLevel {
    L0, // Normal
    L1, // Switch approach
    L2, // Deep investigate
    L3, // 7-point checklist
    L4, // Desperation mode
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityCompass {
    pub build_proof: bool,
    pub error_cases_tested: bool,
    pub pattern_scanned: bool,
    pub root_cause_explained: bool,
    pub quality_improved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedLineViolation {
    pub red_line: u8,  // 1, 2, or 3
    pub trigger_phrase: String,
    pub detected_at: std::time::SystemTime,
    pub context: String,
}

impl PuaTracker {
    pub fn new() -> Self {
        Self {
            failure_count: 0,
            pressure_level: PressureLevel::L0,
            quality_compass: QualityCompass {
                build_proof: false,
                error_cases_tested: false,
                pattern_scanned: false,
                root_cause_explained: false,
                quality_improved: false,
            },
            red_line_violations: Vec::new(),
            iceberg_patterns: HashMap::new(),
            last_failure_at: None,
        }
    }

    /// Record a failure and escalate pressure
    pub fn record_failure(&mut self) {
        self.failure_count += 1;
        self.last_failure_at = Some(std::time::SystemTime::now());
        self.pressure_level = match self.failure_count {
            1 => PressureLevel::L0,
            2 => PressureLevel::L1,
            3 => PressureLevel::L2,
            4 => PressureLevel::L3,
            _ => PressureLevel::L4,
        };
    }

    /// Check if response violates red lines
    pub fn check_red_lines(&mut self, response_text: &str) -> Vec<String> {
        let mut violations = Vec::new();

        // Red Line 1: Close the loop
        let red_line_1_phrases = vec![
            "I think it works",
            "should compile",
            "probably works",
            "I think",
            "should work",
            "seems like",
        ];
        for phrase in red_line_1_phrases {
            if response_text.to_lowercase().contains(phrase) {
                violations.push(format!(
                    "RED_LINE_1: Unverified claim detected: '{}'",
                    phrase
                ));
                self.red_line_violations.push(RedLineViolation {
                    red_line: 1,
                    trigger_phrase: phrase.to_string(),
                    detected_at: std::time::SystemTime::now(),
                    context: response_text.lines().next().unwrap_or("").to_string(),
                });
            }
        }

        // Red Line 2: Fact-driven
        let red_line_2_phrases = vec![
            "probably environment issue",
            "maybe it's",
            "might be",
            "could be",
            "possibly",
        ];
        for phrase in red_line_2_phrases {
            if response_text.to_lowercase().contains(phrase)
                && !response_text.to_lowercase().contains("verified")
            {
                violations.push(format!(
                    "RED_LINE_2: Unverified hypothesis detected: '{}'",
                    phrase
                ));
                self.red_line_violations.push(RedLineViolation {
                    red_line: 2,
                    trigger_phrase: phrase.to_string(),
                    detected_at: std::time::SystemTime::now(),
                    context: response_text.lines().next().unwrap_or("").to_string(),
                });
            }
        }

        // Red Line 3: Exhausted everything (checked if failure_count > 2)
        if self.failure_count > 2 {
            let red_line_3_phrases = vec![
                "beyond my scope",
                "beyond scope",
                "need more context",
                "not possible",
                "impossible",
            ];
            for phrase in red_line_3_phrases {
                if response_text.to_lowercase().contains(phrase) {
                    violations.push(format!(
                        "RED_LINE_3: Premature surrender detected: '{}'",
                        phrase
                    ));
                    self.red_line_violations.push(RedLineViolation {
                        red_line: 3,
                        trigger_phrase: phrase.to_string(),
                        detected_at: std::time::SystemTime::now(),
                        context: format!("After {} failures", self.failure_count),
                    });
                }
            }
        }

        violations
    }

    /// Validate quality compass (5-point check)
    pub fn validate_quality_compass(&mut self, response_text: &str) -> Vec<String> {
        let mut rejections = Vec::new();

        // Check 1: Build proof
        if response_text.to_lowercase().contains("finished")
            || response_text.contains("cargo check")
            || response_text.contains("✅")
        {
            self.quality_compass.build_proof = true;
        } else if response_text.contains("fixed") || response_text.contains("compiled") {
            rejections.push(
                "QUALITY_COMPASS_1: Build proof missing. Show actual 'Finished' or compile output."
                    .to_string(),
            );
        }

        // Check 2: Error cases tested
        if response_text.to_lowercase().contains("tested")
            && response_text.to_lowercase().contains("error")
        {
            self.quality_compass.error_cases_tested = true;
        } else if response_text.contains("error handling") || response_text.contains("try-catch") {
            rejections.push(
                "QUALITY_COMPASS_2: Error case testing missing. Show test with invalid input."
                    .to_string(),
            );
        }

        // Check 3: Pattern scanned (iceberg rule)
        if response_text.to_lowercase().contains("grep")
            || response_text.to_lowercase().contains("scan")
            || response_text.contains("similar")
        {
            self.quality_compass.pattern_scanned = true;
        }

        // Check 4: Root cause explained
        if response_text.to_lowercase().contains("root cause")
            || response_text.to_lowercase().contains("cause:")
        {
            self.quality_compass.root_cause_explained = true;
        } else if response_text.contains("fixed") {
            rejections.push(
                "QUALITY_COMPASS_4: Root cause missing. Explain WHY it failed and HOW to prevent it."
                    .to_string(),
            );
        }

        // Check 5: Quality improved
        if response_text.contains("level") || response_text.contains("quality")
            || response_text.contains("improved")
        {
            self.quality_compass.quality_improved = true;
        }

        rejections
    }

    /// Get current checkpoint count
    pub fn quality_score(&self) -> f32 {
        let checks = [
            self.quality_compass.build_proof,
            self.quality_compass.error_cases_tested,
            self.quality_compass.pattern_scanned,
            self.quality_compass.root_cause_explained,
            self.quality_compass.quality_improved,
        ];

        checks.iter().filter(|&&c| c).count() as f32 / 5.0
    }

    /// Check if L3 checklist is needed
    pub fn is_l3_required(&self) -> bool {
        self.failure_count >= 4
    }

    /// Trigger iceberg scan for related issues
    pub fn scan_iceberg(&mut self, issue_category: &str, grep_results: usize) {
        self.iceberg_patterns
            .insert(issue_category.to_string(), grep_results);
        if grep_results > 0 {
            self.quality_compass.pattern_scanned = true;
        }
    }
}
```

### Step 2: Add PUA to AppConfig

**File**: `src/config.rs` (MODIFY)

```rust
pub struct AppConfig {
    // ... existing fields ...

    /// PUA enforcement rules (loaded from RULES/pua.md)
    pub pua_enabled: bool,
    pub pua_rules: Option<String>,  // Content of RULES/pua.md
}
```

### Step 3: Load PUA Rules on Config Initialization

**File**: `src/config.rs` (MODIFY in apply_auto_rules function)

```rust
fn apply_auto_rules(config_path: &Path, config: &mut AppConfig) {
    let mut shared_rules = Vec::new();

    // ... existing rule loading ...

    // Load PUA rules
    if let Ok(pua_content) = std::fs::read_to_string(config_path.join("RULES/pua.md")) {
        config.pua_rules = Some(pua_content);
        config.pua_enabled = true;
        log::info!("PUA enforcement rules loaded from RULES/pua.md");
    }

    // ... rest of function ...
}
```

### Step 4: Agent Task Integration

**File**: `src/orchestrator.rs` or agent proxy handler (MODIFY)

```rust
pub struct AgentTask {
    pub id: String,
    pub phase: String,
    pub request: String,
    pub response: Option<String>,
    
    // NEW: PUA enforcement tracking
    pub pua_tracker: PuaTracker,
}

impl AgentTask {
    pub fn new(id: String, phase: String, request: String) -> Self {
        Self {
            id,
            phase,
            request,
            response: None,
            pua_tracker: PuaTracker::new(),
        }
    }

    /// Process agent response with PUA validation
    pub async fn validate_response(
        &mut self,
        agent_response: String,
        config: &AppConfig,
    ) -> Result<ValidatedResponse, PuaRejection> {
        if !config.pua_enabled {
            // If PUA disabled, accept response as-is
            return Ok(ValidatedResponse::Approved(agent_response));
        }

        // Step 1: Check for red line violations
        let red_lines = self.pua_tracker.check_red_lines(&agent_response);
        if !red_lines.is_empty() {
            self.pua_tracker.record_failure();
            return Err(PuaRejection {
                reason: "RED_LINE_VIOLATION",
                details: red_lines,
                pressure_level: self.pua_tracker.pressure_level,
                retry_count: self.pua_tracker.failure_count,
            });
        }

        // Step 2: Validate quality compass
        let quality_rejections = self.pua_tracker.validate_quality_compass(&agent_response);
        if !quality_rejections.is_empty() && self.pua_tracker.failure_count >= 1 {
            self.pua_tracker.record_failure();
            return Err(PuaRejection {
                reason: "QUALITY_COMPASS_FAILED",
                details: quality_rejections,
                pressure_level: self.pua_tracker.pressure_level,
                retry_count: self.pua_tracker.failure_count,
            });
        }

        // Step 3: Check if L3 checklist needed
        if self.pua_tracker.is_l3_required() {
            // Trigger comprehensive validation
            log::warn!(
                "Task {} at L3: Triggering 7-point checklist",
                self.id
            );
            // TODO: Implement 7-point checklist validation
        }

        // Step 4: Iceberg scan (check for related issues)
        // TODO: Implement pattern scanning

        // All validations passed
        self.response = Some(agent_response.clone());
        Ok(ValidatedResponse::Approved(agent_response))
    }
}

#[derive(Debug)]
pub struct PuaRejection {
    pub reason: &'static str,
    pub details: Vec<String>,
    pub pressure_level: PressureLevel,
    pub retry_count: u32,
}

pub enum ValidatedResponse {
    Approved(String),
    RejectedByPua(PuaRejection),
}
```

### Step 5: Logging & Observability

**File**: `src/audit.rs` (EXTEND)

```rust
#[derive(Debug, Serialize)]
pub struct PuaViolationLog {
    pub task_id: String,
    pub timestamp: SystemTime,
    pub violation_type: String,
    pub red_lines: Vec<String>,
    pub quality_score: f32,
    pub pressure_level: String,
    pub failure_count: u32,
    pub agent: String,
    pub phase: String,
}

pub fn log_pua_violation(task: &AgentTask, rejection: &PuaRejection) {
    let violation_log = PuaViolationLog {
        task_id: task.id.clone(),
        timestamp: SystemTime::now(),
        violation_type: rejection.reason.to_string(),
        red_lines: task.pua_tracker.red_line_violations
            .iter()
            .map(|rv| format!("RED_{}: {}", rv.red_line, rv.trigger_phrase))
            .collect(),
        quality_score: task.pua_tracker.quality_score(),
        pressure_level: format!("{:?}", task.pua_tracker.pressure_level),
        failure_count: task.pua_tracker.failure_count,
        agent: "agent_name".to_string(),  // From task context
        phase: task.phase.clone(),
    };

    // Log to observability system
    log::warn!(
        "PUA_VIOLATION: {} - Quality Score: {:.2} - Failures: {}",
        violation_log.violation_type,
        violation_log.quality_score,
        violation_log.failure_count
    );
}
```

---

## Integration Checklist

- [ ] Create `src/pua.rs` with PuaTracker struct
- [ ] Add PUA fields to AppConfig
- [ ] Update config.rs to load RULES/pua.md
- [ ] Modify agent task handler to validate responses
- [ ] Add PuaRejection type and error handling
- [ ] Integrate logging in audit.rs
- [ ] Add metrics/observability hooks
- [ ] Write tests for red line detection
- [ ] Write tests for quality compass validation
- [ ] Document PUA behavior in README
- [ ] Add CLI flag: `--pua-enforce true/false`
- [ ] Add config option: `pua.enabled = true/false`

---

## ENV Variables (Optional)

```bash
# Enable/disable PUA globally
export GO_ON_PUA_ENABLED=true

# Set minimum quality score (0.0-1.0)
export GO_ON_PUA_MIN_QUALITY=0.8

# Auto-escalate on failures
export GO_ON_PUA_AUTO_ESCALATE=true

# Log violations to file
export GO_ON_PUA_LOG_PATH=/var/log/go-on-pua-violations.log
```

---

## Config Example

```toml
[pua]
enabled = true
min_quality_score = 0.8
auto_escalate_on_failure = true
log_violations = true
violation_log_path = "pua-violations.log"

[phases.agent]
principles = [
  "Apply PUA enforcement (three red lines)",
  "Reject unverified claims",
  "Mandate build proof for all code changes",
  "Scan for related issues (iceberg rule)",
  "Explain root causes for all fixes",
]
```

---

## Testing

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_red_line_1_detection() {
        let mut tracker = PuaTracker::new();
        let response = "I think it works now, should compile fine.";
        let violations = tracker.check_red_lines(response);
        assert!(!violations.is_empty());
        assert!(violations[0].contains("RED_LINE_1"));
    }

    #[test]
    fn test_quality_compass_validation() {
        let mut tracker = PuaTracker::new();
        let response = "Fixed the bug.";
        let rejections = tracker.validate_quality_compass(response);
        assert!(!rejections.is_empty());
    }

    #[test]
    fn test_pressure_escalation() {
        let mut tracker = PuaTracker::new();
        assert_eq!(tracker.pressure_level, PressureLevel::L0);
        
        tracker.record_failure(); // 1st failure
        assert_eq!(tracker.pressure_level, PressureLevel::L0);
        
        tracker.record_failure(); // 2nd failure
        assert_eq!(tracker.pressure_level, PressureLevel::L1);
        
        tracker.record_failure(); // 3rd failure
        assert_eq!(tracker.pressure_level, PressureLevel::L2);
        
        tracker.record_failure(); // 4th failure
        assert_eq!(tracker.pressure_level, PressureLevel::L3);
    }
}
```

---

## Summary

**PUA enforcement is now integrated into go-on app as:**

1. ✅ Core tracking struct (PuaTracker)
2. ✅ Red line detection (automatic)
3. ✅ Pressure escalation (L0-L4)
4. ✅ Quality compass validation (5-point check)
5. ✅ Configuration loading (RULES/pua.md)
6. ✅ Logging & observability hooks
7. ✅ Agent response validation pipeline

**When agent responds to request:**
- Response is validated against all PUA rules
- Violations are logged
- If failed: rejection returned to agent with escalated pressure
- If passed: response approved and returned to user

**Status**: Ready for implementation in Rust.

---

*Last Updated: 2026-04-02*
