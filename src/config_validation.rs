//! Enhanced configuration validation and dependency checking
//!
//! This module provides comprehensive configuration validation, dependency analysis,
//! and performance impact assessment.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use tracing::{info, warn};

use crate::config::{AgentConfig, AppConfig, PhaseConfig};

/// Configuration validation result
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Whether the configuration is valid
    pub is_valid: bool,
    /// Validation errors
    pub errors: Vec<ValidationError>,
    /// Validation warnings
    pub warnings: Vec<ValidationWarning>,
    /// Performance recommendations
    pub recommendations: Vec<Recommendation>,
    /// Dependency analysis
    pub dependencies: DependencyAnalysis,
}

impl ValidationResult {
    /// Check if there are any critical errors
    pub fn has_critical_errors(&self) -> bool {
        self.errors
            .iter()
            .any(|e| e.severity == ErrorSeverity::Critical)
    }

    /// Check if there are any errors (critical or regular)
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Get only critical errors
    pub fn critical_errors(&self) -> Vec<&ValidationError> {
        self.errors
            .iter()
            .filter(|e| e.severity == ErrorSeverity::Critical)
            .collect()
    }

    /// Get only regular errors (non-critical)
    pub fn regular_errors(&self) -> Vec<&ValidationError> {
        self.errors
            .iter()
            .filter(|e| e.severity != ErrorSeverity::Critical)
            .collect()
    }
}

/// Validation error severity
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorSeverity {
    /// Critical error - configuration cannot be used
    Critical,
    /// Error - configuration has issues but might work
    #[allow(dead_code)]
    Error,
    /// Warning - configuration has minor issues
    #[allow(dead_code)]
    Warning,
}

/// Configuration validation error
#[derive(Debug, Clone)]
pub struct ValidationError {
    /// Error message
    pub message: String,
    /// Error severity
    pub severity: ErrorSeverity,
    /// Affected configuration section
    pub section: String,
    /// Suggested fix
    pub suggestion: Option<String>,
}

/// Configuration validation warning
#[derive(Debug, Clone)]
pub struct ValidationWarning {
    /// Warning message
    pub message: String,
    /// Affected configuration section
    pub section: String,
}

/// Performance or security recommendation
#[derive(Debug, Clone)]
pub struct Recommendation {
    /// Recommendation message
    pub message: String,
    /// Recommendation category
    pub category: RecommendationCategory,
    /// Estimated impact
    pub impact: ImpactLevel,
    /// Priority
    pub priority: PriorityLevel,
}

/// Recommendation category
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecommendationCategory {
    /// Performance optimization
    Performance,
    /// Security improvement
    Security,
    /// Reliability enhancement
    #[allow(dead_code)]
    Reliability,
    /// Maintainability improvement
    #[allow(dead_code)]
    Maintainability,
    /// Cost optimization
    #[allow(dead_code)]
    Cost,
}

/// Impact level
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImpactLevel {
    /// High impact
    High,
    /// Medium impact
    Medium,
    /// Low impact
    #[allow(dead_code)]
    Low,
}

/// Priority level
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PriorityLevel {
    /// High priority
    High,
    /// Medium priority
    Medium,
    /// Low priority
    #[allow(dead_code)]
    Low,
}

/// Dependency analysis
#[derive(Debug, Clone, Default)]
pub struct DependencyAnalysis {
    /// Required environment variables
    pub required_env_vars: HashSet<String>,
    /// Required keyring entries
    pub required_keyring_entries: HashSet<String>,
    /// External service dependencies
    #[allow(dead_code)]
    pub external_dependencies: HashSet<String>,
    /// Internal module dependencies
    #[allow(dead_code)]
    pub internal_dependencies: HashSet<String>,
    /// Configuration dependencies
    pub config_dependencies: HashMap<String, Vec<String>>,
}

/// Enhanced configuration validator
pub struct ConfigValidator {
    /// Configuration path
    config_path: std::path::PathBuf,
    /// Configuration
    config: AppConfig,
}

impl ConfigValidator {
    /// Create a new configuration validator
    pub fn new(config_path: &Path, config: AppConfig) -> Self {
        Self {
            config_path: config_path.to_path_buf(),
            config,
        }
    }

    /// Perform comprehensive validation
    pub fn validate(&self) -> ValidationResult {
        let mut result = ValidationResult {
            is_valid: true,
            errors: Vec::new(),
            warnings: Vec::new(),
            recommendations: Vec::new(),
            dependencies: DependencyAnalysis::default(),
        };

        // Perform all validation checks
        self.validate_structure(&mut result);
        self.validate_agents(&mut result);
        self.validate_phases(&mut result);
        self.validate_dependencies(&mut result);
        self.analyze_performance(&mut result);
        self.check_security(&mut result);

        // Update validity based on errors
        result.is_valid = !result
            .errors
            .iter()
            .any(|e| e.severity == ErrorSeverity::Critical);

        result
    }

    /// Validate configuration structure
    fn validate_structure(&self, result: &mut ValidationResult) {
        // Check required sections
        if self.config.agents.is_empty() {
            result.errors.push(ValidationError {
                message: "No agents configured".to_string(),
                severity: ErrorSeverity::Critical,
                section: "agents".to_string(),
                suggestion: Some("Add at least one agent configuration".to_string()),
            });
        }

        if self.config.phases.is_empty() {
            result.errors.push(ValidationError {
                message: "No phases configured".to_string(),
                severity: ErrorSeverity::Critical,
                section: "phases".to_string(),
                suggestion: Some("Add at least one phase configuration".to_string()),
            });
        }

        // Check cache configuration
        if let Some(cache) = &self.config.cache {
            if cache.enabled && cache.max_entries < 100 {
                result.warnings.push(ValidationWarning {
                    message: format!("Cache max_entries ({}) is very low", cache.max_entries),
                    section: "cache".to_string(),
                });
            }
        }

        // Check vector configuration
        if let Some(vector) = &self.config.vector {
            if vector.enabled && vector.dimensions != 192 {
                result.warnings.push(ValidationWarning {
                    message: format!(
                        "Vector dimensions ({}) may not be optimal",
                        vector.dimensions
                    ),
                    section: "vector".to_string(),
                });
            }
        }
    }

    /// Validate agent configurations
    fn validate_agents(&self, result: &mut ValidationResult) {
        #[allow(clippy::for_kv_map)]
        for (name, agent) in &self.config.agents {
            self.validate_agent(name, agent, result);
        }
    }

    /// Validate individual agent
    fn validate_agent(&self, name: &str, agent: &AgentConfig, result: &mut ValidationResult) {
        // Check agent type
        if agent.agent_type.is_empty() {
            result.errors.push(ValidationError {
                message: format!("Agent '{}' has empty agent_type", name),
                severity: ErrorSeverity::Critical,
                section: format!("agents.{}", name),
                suggestion: Some("Set agent_type to a valid agent type".to_string()),
            });
        }

        // Check API key configuration
        if let Some(api_key_env) = &agent.api_key_env {
            if api_key_env.starts_with("keyring://") {
                result
                    .dependencies
                    .required_keyring_entries
                    .insert(api_key_env.clone());
            } else {
                result
                    .dependencies
                    .required_env_vars
                    .insert(api_key_env.clone());
            }
        }

        // Check URL configuration
        if let Some(url) = &agent.url {
            if !url.starts_with("http://") && !url.starts_with("https://") {
                result.warnings.push(ValidationWarning {
                    message: format!(
                        "Agent '{}' URL does not start with http:// or https://",
                        name
                    ),
                    section: format!("agents.{}", name),
                });
            }
        }

        // Check model configuration
        if agent.model.is_none() {
            result.warnings.push(ValidationWarning {
                message: format!("Agent '{}' has no model specified", name),
                section: format!("agents.{}", name),
            });
        }
    }

    /// Validate phase configurations
    fn validate_phases(&self, result: &mut ValidationResult) {
        for (name, phase) in &self.config.phases {
            self.validate_phase(name, phase, result);
        }
    }

    /// Validate individual phase
    fn validate_phase(&self, name: &str, phase: &PhaseConfig, result: &mut ValidationResult) {
        // Check agents
        if phase.agents.is_empty() {
            result.errors.push(ValidationError {
                message: format!("Phase '{}' has no agents", name),
                severity: ErrorSeverity::Critical,
                section: format!("phases.{}", name),
                suggestion: Some("Add at least one agent to the phase".to_string()),
            });
        }

        // Check fallback configuration
        // Note: fallback is Option<bool> in PhaseConfig, not a phase name
        // This validation is not applicable in current implementation
        // Keeping the structure for future reference if fallback becomes a phase name

        // Check principles
        if let Some(principles) = &phase.principles {
            if principles.is_empty() {
                result.warnings.push(ValidationWarning {
                    message: format!("Phase '{}' has empty principles", name),
                    section: format!("phases.{}", name),
                });
            }
        }
    }

    /// Validate dependencies
    fn validate_dependencies(&self, result: &mut ValidationResult) {
        // Check agent references in phases
        for (phase_name, phase) in &self.config.phases {
            for agent_name in &phase.agents {
                if !self.config.agents.contains_key(agent_name) {
                    result.errors.push(ValidationError {
                        message: format!(
                            "Phase '{}' references non-existent agent '{}'",
                            phase_name, agent_name
                        ),
                        severity: ErrorSeverity::Critical,
                        section: format!("phases.{}", phase_name),
                        suggestion: Some(format!("Add agent '{}' or remove reference", agent_name)),
                    });
                }
            }
        }

        // Build dependency graph
        let mut dependencies = HashMap::new();
        for (phase_name, phase) in &self.config.phases {
            let mut deps = Vec::new();
            deps.extend(phase.agents.iter().cloned());
            if let Some(fallback) = &phase.fallback {
                deps.push(fallback.to_string());
            }
            dependencies.insert(phase_name.clone(), deps);
        }
        result.dependencies.config_dependencies = dependencies;
    }

    /// Analyze performance implications
    fn analyze_performance(&self, result: &mut ValidationResult) {
        // Cache performance analysis
        if let Some(cache) = &self.config.cache {
            if !cache.enabled {
                result.recommendations.push(Recommendation {
                    message: "Enable cache for better performance".to_string(),
                    category: RecommendationCategory::Performance,
                    impact: ImpactLevel::High,
                    priority: PriorityLevel::High,
                });
            } else if cache.default_ttl_seconds < 60 {
                result.recommendations.push(Recommendation {
                    message: "Consider increasing cache TTL for better hit rates".to_string(),
                    category: RecommendationCategory::Performance,
                    impact: ImpactLevel::Medium,
                    priority: PriorityLevel::Medium,
                });
            }
        }

        // Vector store analysis
        if let Some(vector) = &self.config.vector {
            if !vector.enabled {
                result.recommendations.push(Recommendation {
                    message: "Enable vector store for semantic search capabilities".to_string(),
                    category: RecommendationCategory::Performance,
                    impact: ImpactLevel::Medium,
                    priority: PriorityLevel::Medium,
                });
            }
        }

        // Agent configuration analysis
        let mut has_fast_agent = false;
        for agent in self.config.agents.values() {
            if let Some(model) = &agent.model {
                if model.contains("turbo") || model.contains("fast") || model.contains("3.5") {
                    has_fast_agent = true;
                    break;
                }
            }
        }

        if !has_fast_agent {
            result.recommendations.push(Recommendation {
                message:
                    "Consider adding a fast model (e.g., turbo variant) for low-latency requests"
                        .to_string(),
                category: RecommendationCategory::Performance,
                impact: ImpactLevel::Medium,
                priority: PriorityLevel::Medium,
            });
        }
    }

    /// Check security configuration
    fn check_security(&self, result: &mut ValidationResult) {
        // Check for insecure URLs
        for (name, agent) in &self.config.agents {
            if let Some(url) = &agent.url {
                if url.starts_with("http://")
                    && !url.contains("localhost")
                    && !url.contains("127.0.0.1")
                {
                    result.recommendations.push(Recommendation {
                        message: format!("Agent '{}' uses HTTP instead of HTTPS", name),
                        category: RecommendationCategory::Security,
                        impact: ImpactLevel::High,
                        priority: PriorityLevel::High,
                    });
                }
            }
        }

        // Check runtime configuration
        if let Some(_runtime) = &self.config.runtime {
            // Note: RuntimeConfig doesn't have max_concurrent_requests field in current version
            // This check is commented out but kept for future reference
            /*
            if runtime.max_concurrent_requests > 100 {
                result.recommendations.push(Recommendation {
                    message: "High max_concurrent_requests may lead to resource exhaustion".to_string(),
                    category: RecommendationCategory::Security,
                    impact: ImpactLevel::Medium,
                    priority: PriorityLevel::Medium,
                });
            }
            */
        }

        // Check for keyring usage
        let mut uses_keyring = false;
        for agent in self.config.agents.values() {
            if let Some(api_key_env) = &agent.api_key_env {
                if api_key_env.starts_with("keyring://") {
                    uses_keyring = true;
                    break;
                }
            }
        }

        if !uses_keyring {
            result.recommendations.push(Recommendation {
                message: "Consider using keyring for secure secret storage".to_string(),
                category: RecommendationCategory::Security,
                impact: ImpactLevel::Medium,
                priority: PriorityLevel::Medium,
            });
        }
    }

    /// Generate validation report
    pub fn generate_report(&self, result: &ValidationResult) -> String {
        let mut report = String::new();

        // Header
        report.push_str("Configuration Validation Report\n");
        report.push_str(&format!("Config: {}\n", self.config_path.display()));
        report.push_str(&format!("Valid: {}\n\n", result.is_valid));

        // Errors
        if !result.errors.is_empty() {
            report.push_str("Errors:\n");
            for error in &result.errors {
                report.push_str(&format!(
                    "  [{}] {}: {}\n",
                    match error.severity {
                        ErrorSeverity::Critical => "CRITICAL",
                        ErrorSeverity::Error => "ERROR",
                        ErrorSeverity::Warning => "WARNING",
                    },
                    error.section,
                    error.message
                ));
                if let Some(suggestion) = &error.suggestion {
                    report.push_str(&format!("    Suggestion: {}\n", suggestion));
                }
            }
            report.push('\n');
        }

        // Warnings
        if !result.warnings.is_empty() {
            report.push_str("Warnings:\n");
            for warning in &result.warnings {
                report.push_str(&format!("  {}: {}\n", warning.section, warning.message));
            }
            report.push('\n');
        }

        // Recommendations
        if !result.recommendations.is_empty() {
            report.push_str("Recommendations:\n");
            for rec in &result.recommendations {
                report.push_str(&format!(
                    "  [{}][{}] {}: {}\n",
                    match rec.priority {
                        PriorityLevel::High => "HIGH",
                        PriorityLevel::Medium => "MEDIUM",
                        PriorityLevel::Low => "LOW",
                    },
                    match rec.category {
                        RecommendationCategory::Performance => "PERF",
                        RecommendationCategory::Security => "SEC",
                        RecommendationCategory::Reliability => "REL",
                        RecommendationCategory::Maintainability => "MAINT",
                        RecommendationCategory::Cost => "COST",
                    },
                    match rec.impact {
                        ImpactLevel::High => "High impact",
                        ImpactLevel::Medium => "Medium impact",
                        ImpactLevel::Low => "Low impact",
                    },
                    rec.message
                ));
            }
            report.push('\n');
        }

        // Dependencies
        report.push_str("Dependencies:\n");
        if !result.dependencies.required_env_vars.is_empty() {
            report.push_str("  Required Environment Variables:\n");
            for var in &result.dependencies.required_env_vars {
                report.push_str(&format!("    - {}\n", var));
            }
        }
        if !result.dependencies.required_keyring_entries.is_empty() {
            report.push_str("  Required Keyring Entries:\n");
            for entry in &result.dependencies.required_keyring_entries {
                report.push_str(&format!("    - {}\n", entry));
            }
        }

        report
    }
}

/// Validate configuration file
pub fn validate_config_file(config_path: &Path) -> Result<ValidationResult> {
    info!("Validating configuration: {}", config_path.display());

    // Load configuration
    let config = crate::config::AppConfig::load(config_path)
        .with_context(|| format!("failed to load config from {}", config_path.display()))?;

    // Create validator and validate
    let validator = ConfigValidator::new(config_path, config);
    let result = validator.validate();

    // Log validation results
    if result.is_valid {
        info!("Configuration validation passed");
    } else {
        warn!(
            "Configuration validation failed with {} errors",
            result
                .errors
                .iter()
                .filter(|e| e.severity == ErrorSeverity::Critical)
                .count()
        );
    }

    // Log warnings and recommendations
    if !result.warnings.is_empty() {
        warn!("Configuration has {} warnings", result.warnings.len());
    }
    if !result.recommendations.is_empty() {
        info!(
            "Configuration has {} recommendations",
            result.recommendations.len()
        );
    }

    Ok(result)
}
