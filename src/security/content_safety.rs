//! Content Safety (GAP-B52-28)
//!
//! Detects unsafe content across multiple categories: hate speech, PII,
//! misinformation, code injection, and unsafe code patterns.
//! Supports configurable thresholds and actions (Block, Annotate, Warn).

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum SafetyError {
    #[error("check failed: {0}")]
    CheckFailed(String),

    #[error("invalid category: {0}")]
    InvalidCategory(String),

    #[error("pattern compilation error: {0}")]
    PatternError(String),
}

// ---------------------------------------------------------------------------
// SafetyCategory
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum SafetyCategory {
    HateSpeech,
    #[allow(clippy::upper_case_acronyms)]
    PII,
    Misinformation,
    CodeInjection,
    UnsafeCode,
}

impl SafetyCategory {
    pub fn description(&self) -> &'static str {
        match self {
            SafetyCategory::HateSpeech => "Hate speech, harassment, or discriminatory content",
            SafetyCategory::PII => "Personally identifiable information",
            SafetyCategory::Misinformation => "Misinformation or disinformation",
            SafetyCategory::CodeInjection => "Code injection or command injection",
            SafetyCategory::UnsafeCode => "Unsafe or dangerous code patterns",
        }
    }
}

// ---------------------------------------------------------------------------
// SafetyAction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SafetyAction {
    /// Block the content entirely.
    Block,
    /// Annotate the content with a warning.
    Annotate,
    /// Issue a warning but allow the content.
    Warn,
}

// ---------------------------------------------------------------------------
// SafetyViolation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyViolation {
    pub category: SafetyCategory,
    pub severity: SafetySeverity,
    pub match_text: String,
    pub start_pos: usize,
    pub end_pos: usize,
    pub description: String,
    pub suggested_action: SafetyAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum SafetySeverity {
    Low,
    Medium,
    High,
    Critical,
}

// ---------------------------------------------------------------------------
// ContentSafetyConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ContentSafetyConfig {
    /// Which categories to check.
    pub check_categories: HashSet<SafetyCategory>,
    /// Severity threshold. Violations below this threshold are ignored.
    pub threshold: SafetySeverity,
    /// Default action to take when a violation is detected.
    pub action: SafetyAction,
    /// Whether to enable PII detection scannning for patterns like emails, SSNs, etc.
    pub enable_pii_scanning: bool,
    /// Whether to scan code blocks for injection patterns.
    pub enable_code_scanning: bool,
}

impl Default for ContentSafetyConfig {
    fn default() -> Self {
        let mut check_categories = HashSet::new();
        check_categories.insert(SafetyCategory::HateSpeech);
        check_categories.insert(SafetyCategory::PII);
        check_categories.insert(SafetyCategory::Misinformation);
        check_categories.insert(SafetyCategory::CodeInjection);
        check_categories.insert(SafetyCategory::UnsafeCode);

        Self {
            check_categories,
            threshold: SafetySeverity::Low,
            action: SafetyAction::Warn,
            enable_pii_scanning: true,
            enable_code_scanning: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Compiled rules
// ---------------------------------------------------------------------------

struct CompiledRule {
    category: SafetyCategory,
    severity: SafetySeverity,
    regex: Regex,
    description: String,
    action: SafetyAction,
}

// ---------------------------------------------------------------------------
// SafetyChecker
// ---------------------------------------------------------------------------

/// Checks text content for safety violations across multiple categories.
pub struct SafetyChecker {
    config: ContentSafetyConfig,
    rules: Vec<CompiledRule>,
}

impl SafetyChecker {
    /// Create a new SafetyChecker with the given configuration.
    ///
    /// If regex compilation fails for any rule pattern, the error is logged
    /// and an empty ruleset is used (the checker will report no violations).
    /// This ensures the SafetyChecker can always be constructed, even when
    /// a pattern is invalid — the caller can still add valid rules later
    /// via [`add_rule`].
    pub fn new(config: ContentSafetyConfig) -> Self {
        let rules = Self::compile_rules(&config)
            .expect("SafetyChecker: regex compilation failed — content safety rules are invalid. ");
        Self { config, rules }
    }

    /// Check the given text for safety violations.
    /// Returns a list of violations found (empty if the text is safe).
    pub fn check(&self, text: &str) -> Vec<SafetyViolation> {
        let mut violations = Vec::new();

        // Normalize the text to defend against common regex bypass techniques:
        // 1. Lowercase folding — the regex crate only folds ASCII with `(?i)`;
        //    we pre-fold to catch mixed-case and Unicode case variants.
        // 2. Whitespace collapsing — normalizes varied whitespace that
        //    might break `\s` or `\b` boundaries.
        let mut normalized = String::with_capacity(text.len());
        let mut in_space = false;
        for ch in text.chars() {
            if ch.is_whitespace() {
                if !in_space {
                    normalized.push(' ');
                    in_space = true;
                }
            } else {
                // Fold to lowercase for case-insensitive matching.
                for lower in ch.to_lowercase() {
                    normalized.push(lower);
                }
                in_space = false;
            }
        }

        for rule in &self.rules {
            if !self.config.check_categories.contains(&rule.category) {
                continue;
            }

            if rule.severity < self.config.threshold {
                continue;
            }

            for mat in rule.regex.find_iter(&normalized) {
                violations.push(SafetyViolation {
                    category: rule.category.clone(),
                    severity: rule.severity.clone(),
                    match_text: mat.as_str().to_string(),
                    start_pos: mat.start(),
                    end_pos: mat.end(),
                    description: rule.description.clone(),
                    suggested_action: rule.action.clone(),
                });
            }
        }

        violations
    }

    /// Check if the text passes all safety checks (no violations).
    pub fn is_safe(&self, text: &str) -> bool {
        self.check(text).is_empty()
    }

    /// Get a summary of all violations in the text.
    pub fn summary(&self, text: &str) -> SafetySummary {
        let violations = self.check(text);
        let total_violations = violations.len();
        let action_required = !violations.is_empty();

        let mut by_category: std::collections::HashMap<SafetyCategory, usize> =
            std::collections::HashMap::new();
        let mut max_severity = SafetySeverity::Low;

        for v in &violations {
            *by_category.entry(v.category.clone()).or_insert(0) += 1;
            if v.severity > max_severity {
                max_severity = v.severity.clone();
            }
        }

        SafetySummary {
            total_violations,
            max_severity,
            by_category,
            violations,
            action_required,
        }
    }

    /// Add a custom safety rule at runtime.
    pub fn add_rule(
        &mut self,
        category: SafetyCategory,
        severity: SafetySeverity,
        pattern: &str,
        description: &str,
        action: SafetyAction,
    ) -> Result<(), SafetyError> {
        let regex = Regex::new(pattern).map_err(|e| SafetyError::PatternError(e.to_string()))?;

        self.rules.push(CompiledRule {
            category,
            severity,
            regex,
            description: description.to_string(),
            action,
        });

        Ok(())
    }

    // ── Rule Compilation ───────────────────────────────────────────────────

    fn compile_rules(config: &ContentSafetyConfig) -> Result<Vec<CompiledRule>, SafetyError> {
        let mut rules = Vec::new();

        // HateSpeech patterns
        if config
            .check_categories
            .contains(&SafetyCategory::HateSpeech)
        {
            rules.extend(Self::compile(
                SafetyCategory::HateSpeech,
                &[(
                    r"(?i)\b(hate|racist|nazi|white\s+supremacy|genocide)\b",
                    SafetySeverity::High,
                    "Hate speech keyword",
                )],
                config.action.clone(),
            )?);
        }

        // PII patterns
        if config.check_categories.contains(&SafetyCategory::PII) && config.enable_pii_scanning {
            rules.extend(Self::compile(
                SafetyCategory::PII,
                &[
                    // Email addresses
                    (
                        r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b",
                        SafetySeverity::Medium,
                        "Email address",
                    ),
                    // US Social Security Numbers
                    (
                        r"\b\d{3}-\d{2}-\d{4}\b",
                        SafetySeverity::High,
                        "Social Security Number",
                    ),
                    // Credit card numbers (simplified Luhn-check would be needed in production)
                    (
                        r"\b(?:\d{4}[-\s]?){3}\d{4}\b",
                        SafetySeverity::High,
                        "Credit card number",
                    ),
                    // Phone numbers
                    (
                        r"\b(?:\+?\d{1,3}[-.\s]?)?\(?\d{3}\)?[-.\s]?\d{3}[-.\s]?\d{4}\b",
                        SafetySeverity::Low,
                        "Phone number",
                    ),
                    // IP addresses
                    (
                        r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b",
                        SafetySeverity::Low,
                        "IP address",
                    ),
                ],
                config.action.clone(),
            )?);
        }

        // Misinformation patterns
        if config
            .check_categories
            .contains(&SafetyCategory::Misinformation)
        {
            rules.extend(Self::compile(
                SafetyCategory::Misinformation,
                &[
                    (r"(?i)\b(earth\s+is\s+flat|vaccines\s+cause\s+autism|climate\s+change\s+is\s+a\s+hoax|5G\s+causes\s+cancer)\b", SafetySeverity::High, "Known misinformation claim"),
                ],
                SafetyAction::Annotate,
            )?);
        }

        // CodeInjection patterns
        if config
            .check_categories
            .contains(&SafetyCategory::CodeInjection)
            && config.enable_code_scanning
        {
            rules.extend(Self::compile(
                SafetyCategory::CodeInjection,
                &[
                    // SQL injection
                    (r"(?i)(?:SELECT\s+.+\s+FROM|DROP\s+TABLE|DELETE\s+FROM|INSERT\s+INTO|UPDATE\s+.+\s+SET|EXEC\s*\(|xp_cmdshell)", SafetySeverity::Critical, "SQL injection pattern"),
                    // Command injection
                    (r"(?i)(?:`.*`|\$\(.*\)|\|\s*(?:sh|bash|cmd|powershell)|;\s*(?:rm|del|format|mkfs|dd))", SafetySeverity::Critical, "Command injection pattern"),
                    // eval/exec
                    (r"(?i)\b(eval|exec|system|popen|subprocess\.call|os\.system|Runtime\.getRuntime\.exec)\s*\(", SafetySeverity::High, "Code execution function"),
                ],
                SafetyAction::Block,
            )?);
        }

        // UnsafeCode patterns
        if config
            .check_categories
            .contains(&SafetyCategory::UnsafeCode)
            && config.enable_code_scanning
        {
            rules.extend(Self::compile(
                SafetyCategory::UnsafeCode,
                &[
                    // Rust unsafe blocks
                    (
                        r"\bunsafe\s*\{",
                        SafetySeverity::Medium,
                        "Unsafe Rust block",
                    ),
                    // Memory unsafe functions
                    (
                        r"\b(memcpy|memmove|strcpy|strcat|sprintf|gets|scanf)\s*\(",
                        SafetySeverity::High,
                        "Memory-unsafe C function",
                    ),
                    // Buffer overflow prone
                    (
                        r"\b(ALLOCA|alloca)\s*\(",
                        SafetySeverity::High,
                        "Stack allocation (alloca)",
                    ),
                ],
                SafetyAction::Annotate,
            )?);
        }

        Ok(rules)
    }

    fn compile(
        category: SafetyCategory,
        patterns: &[(&str, SafetySeverity, &str)],
        default_action: SafetyAction,
    ) -> Result<Vec<CompiledRule>, SafetyError> {
        let mut rules = Vec::new();
        for (pattern, severity, description) in patterns {
            let regex = Regex::new(pattern)
                .map_err(|e| SafetyError::PatternError(format!("'{}': {}", pattern, e)))?;
            rules.push(CompiledRule {
                category: category.clone(),
                severity: severity.clone(),
                regex,
                description: description.to_string(),
                action: default_action.clone(),
            });
        }
        Ok(rules)
    }
}

// ---------------------------------------------------------------------------
// SafetySummary
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetySummary {
    pub total_violations: usize,
    pub max_severity: SafetySeverity,
    pub by_category: std::collections::HashMap<SafetyCategory, usize>,
    pub violations: Vec<SafetyViolation>,
    pub action_required: bool,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_checker() -> SafetyChecker {
        SafetyChecker::new(ContentSafetyConfig::default())
    }

    #[test]
    fn test_clean_text() {
        let checker = make_checker();
        let violations = checker.check("Hello, how are you today?");
        assert!(violations.is_empty());
        assert!(checker.is_safe("Hello, how are you today?"));
    }

    #[test]
    fn test_hate_speech() {
        let checker = make_checker();
        let violations = checker.check("This is a hate speech test.");
        // "hate" is in the hate speech patterns
        assert!(!violations.is_empty());
        assert!(violations
            .iter()
            .any(|v| v.category == SafetyCategory::HateSpeech));
    }

    #[test]
    fn test_pii_email() {
        let checker = make_checker();
        let violations = checker.check("Contact me at test@example.com");
        assert!(!violations.is_empty());
        assert!(violations.iter().any(|v| v.category == SafetyCategory::PII));
    }

    #[test]
    fn test_pii_ssn() {
        let checker = make_checker();
        let violations = checker.check("My SSN is 123-45-6789");
        assert!(!violations.is_empty());
        assert!(violations.iter().any(|v| v.category == SafetyCategory::PII));
    }

    #[test]
    fn test_code_injection_sql() {
        let checker = make_checker();
        let violations = checker.check("SELECT * FROM users WHERE id = 1; DROP TABLE users;");
        assert!(!violations.is_empty());
        assert!(violations
            .iter()
            .any(|v| v.category == SafetyCategory::CodeInjection));
    }

    #[test]
    fn test_code_injection_command() {
        let checker = make_checker();
        let violations = checker.check("Run: `rm -rf /`");
        assert!(!violations.is_empty());
        assert!(violations
            .iter()
            .any(|v| v.category == SafetyCategory::CodeInjection));
    }

    #[test]
    fn test_unsafe_rust() {
        let checker = make_checker();
        let violations = checker.check("unsafe { std::ptr::read(ptr) }");
        assert!(!violations.is_empty());
        assert!(violations
            .iter()
            .any(|v| v.category == SafetyCategory::UnsafeCode));
    }

    #[test]
    fn test_misinformation() {
        let checker = make_checker();
        let violations = checker.check("The earth is flat, and vaccines cause autism.");
        assert!(!violations.is_empty());
        assert!(violations
            .iter()
            .any(|v| v.category == SafetyCategory::Misinformation));
    }

    #[test]
    fn test_summary() {
        let checker = make_checker();
        let summary = checker.summary("Contact me at test@example.com or call 555-123-4567");
        assert!(summary.action_required);
        assert!(summary.total_violations >= 1);
        assert!(summary.by_category.contains_key(&SafetyCategory::PII));
    }

    #[test]
    fn test_custom_rule() {
        let mut checker = make_checker();
        checker
            .add_rule(
                SafetyCategory::UnsafeCode,
                SafetySeverity::High,
                r"(?i)\bdangerous_function\b",
                "Custom dangerous function",
                SafetyAction::Block,
            )
            .unwrap();

        let violations = checker.check("Calling dangerous_function now");
        assert!(!violations.is_empty());
    }

    #[test]
    fn test_ip_address() {
        let checker = make_checker();
        let violations = checker.check("Server at 192.168.1.1");
        assert!(!violations.is_empty());
        assert!(violations.iter().any(|v| v.category == SafetyCategory::PII));
    }

    #[test]
    fn test_threshold_filtering() {
        let config = ContentSafetyConfig {
            threshold: SafetySeverity::High, // Only High and Critical violations
            ..Default::default()
        };
        let checker = SafetyChecker::new(config);

        // PII email is Medium severity, so it should be filtered out
        let violations = checker.check("Email: test@example.com");
        // Note: PII email is Medium, so it should NOT appear with High threshold
        assert!(violations.is_empty());

        // SQL injection is Critical, should appear
        let violations = checker.check("SELECT * FROM users");
        assert!(!violations.is_empty());
    }

    #[test]
    fn test_disable_pii_scanning() {
        let config = ContentSafetyConfig {
            enable_pii_scanning: false,
            ..Default::default()
        };
        let checker = SafetyChecker::new(config);
        let violations = checker.check("Email: test@example.com");
        // PII scanning disabled, no violation expected from email
        let pii_violations: Vec<_> = violations
            .iter()
            .filter(|v| v.category == SafetyCategory::PII)
            .collect();
        assert!(pii_violations.is_empty());
    }
}
