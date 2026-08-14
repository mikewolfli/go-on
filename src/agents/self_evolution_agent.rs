//! GAP-B52-03: Self-Evolution Agent
//!
//! Provides the agent-side intelligence for the self-evolution system:
//! analyzing code, generating patches, fixing compile errors, and
//! assessing risk. Integrates the RULES/ directory as system prompts
//! to guide LLM-based code generation.

use crate::intelligence::adaptive_selector::AdaptiveModelSelector;
use crate::intelligence::model_selector::{
    ModelCharacteristics, ModelSelectionStrategy, SelectionCriteria,
};
use crate::orchestration::self_evolution::sandbox::CodePatch;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tokio::fs;
use tracing::{debug, info, warn};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of compile-error fix iterations.
const MAX_FIX_ITERATIONS: usize = 5;

/// Path to the RULES directory relative to project root.
const RULES_DIR: &str = "RULES";

// ---------------------------------------------------------------------------
// RiskLevel
// ---------------------------------------------------------------------------

/// Risk level assigned to a code patch during assessment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskLevel {
    /// Low risk: cosmetic changes, comments, formatting.
    Low,
    /// Medium risk: logic changes with low blast radius.
    Medium,
    /// High risk: API changes, public interface modifications.
    High,
    /// Critical risk: core infrastructure or security-related changes.
    Critical,
}

impl RiskLevel {
    /// Returns a string label for this risk level.
    pub fn label(&self) -> &str {
        match self {
            RiskLevel::Low => "low",
            RiskLevel::Medium => "medium",
            RiskLevel::High => "high",
            RiskLevel::Critical => "critical",
        }
    }

    /// Returns true if this level requires human approval.
    pub fn requires_human_approval(&self) -> bool {
        matches!(self, RiskLevel::High | RiskLevel::Critical)
    }

    /// Parse a risk level from a string.
    pub fn from_label(label: &str) -> Self {
        match label.to_lowercase().as_str() {
            "low" => RiskLevel::Low,
            "medium" => RiskLevel::Medium,
            "high" => RiskLevel::High,
            "critical" => RiskLevel::Critical,
            _ => RiskLevel::Medium,
        }
    }
}

// ---------------------------------------------------------------------------
// Report (code analysis report)
// ---------------------------------------------------------------------------

/// A structured report produced after analyzing a code module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    /// Unique report ID.
    pub report_id: Uuid,
    /// The target module or file that was analyzed.
    pub target: String,
    /// Number of source lines.
    pub source_lines: usize,
    /// Number of type definitions found.
    pub type_count: usize,
    /// Number of dependencies (use statements).
    pub dep_count: usize,
    /// Number of public items.
    pub public_items: usize,
    /// Number of unsafe blocks found.
    pub unsafe_blocks: usize,
    /// Number of TODO/FIXME comments.
    pub todo_count: usize,
    /// Whether the file compiles (based on available info).
    pub compiles: bool,
    /// Key findings or observations.
    pub findings: Vec<String>,
    /// Risk assessment.
    pub risk: RiskLevel,
    /// Timestamp in milliseconds.
    pub timestamp_ms: u64,
}

impl Report {
    /// Create a new report for the given target.
    pub fn new(target: String) -> Self {
        Self {
            report_id: Uuid::new_v4(),
            target,
            source_lines: 0,
            type_count: 0,
            dep_count: 0,
            public_items: 0,
            unsafe_blocks: 0,
            todo_count: 0,
            compiles: true,
            findings: Vec::new(),
            risk: RiskLevel::Low,
            timestamp_ms: crate::shared::timestamps::now_ts_ms_u64(),
        }
    }

    /// Returns a human-readable summary of the report.
    pub fn summary(&self) -> String {
        format!(
            "Report[{}]: {} — {} lines, {} types, {} deps, risk={}",
            self.report_id,
            self.target,
            self.source_lines,
            self.type_count,
            self.dep_count,
            self.risk.label()
        )
    }
}

// ---------------------------------------------------------------------------
// SelfEvolutionAgentError
// ---------------------------------------------------------------------------

/// Errors that can occur during self-evolution agent operations.
#[derive(Debug, Error)]
pub enum SelfEvolutionAgentError {
    /// Target file not found.
    #[error("target not found: {0}")]
    TargetNotFound(String),

    /// I/O error reading source.
    #[error("I/O error: {0}")]
    IoError(String),

    /// No changes were generated.
    #[error("no changes generated")]
    NoChangesGenerated,

    /// Maximum fix iterations exceeded.
    #[error("max fix iterations ({MAX_FIX_ITERATIONS}) exceeded")]
    MaxFixIterationsExceeded,

    /// Invalid instruction.
    #[error("invalid instruction: {0}")]
    InvalidInstruction(String),
}

impl From<std::io::Error> for SelfEvolutionAgentError {
    fn from(e: std::io::Error) -> Self {
        SelfEvolutionAgentError::IoError(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// SelfEvolutionAgent
// ---------------------------------------------------------------------------

/// The self-evolution agent responsible for code analysis, patch generation,
/// compile-error fixing, and risk assessment.
///
/// Integrates the `RULES/` directory as system prompts to ground LLM
/// generations in the project's coding standards.
pub struct SelfEvolutionAgent {
    /// Adaptive model selector with static fallback for choosing the right LLM.
    adaptive_selector: AdaptiveModelSelector,
    /// Cached RULES content loaded at initialization.
    rules_prompts: Vec<String>,
    /// Project root path for resolving RULES/ and target paths.
    project_root: PathBuf,
    /// Available model characteristics for selection.
    available_models: Vec<ModelCharacteristics>,
    /// Optional LLM agent for AI-driven code analysis and patch generation (BLUE56-B03).
    /// (`dyn Agent` already carries the `Send + Sync` supertrait bounds.)
    llm_agent: Option<Arc<dyn crate::agent::Agent>>,
}

impl std::fmt::Debug for SelfEvolutionAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelfEvolutionAgent")
            .field("adaptive_selector", &self.adaptive_selector)
            .field("rules_prompts", &self.rules_prompts)
            .field("project_root", &self.project_root)
            .field("available_models", &self.available_models)
            .field(
                "llm_agent",
                &self.llm_agent.as_ref().map(|_| "Some(Arc<dyn Agent>)"),
            )
            .finish()
    }
}

impl SelfEvolutionAgent {
    /// Create a new SelfEvolutionAgent.
    ///
    /// Loads RULES/ directory content as system prompts automatically.
    ///
    /// # Arguments
    /// * `project_root` - Root path of the project.
    /// * `available_models` - List of available model characteristics for selection.
    pub async fn new(project_root: PathBuf, available_models: Vec<ModelCharacteristics>) -> Self {
        Self::with_llm(project_root, available_models, None).await
    }

    /// Create a new SelfEvolutionAgent with an optional LLM agent (BLUE56-B03).
    ///
    /// When `llm_agent` is provided, `generate_patch()` uses the LLM for
    /// AI-driven patch generation. `analyze_code()` is deterministic static
    /// analysis (it never calls the LLM); the doc previously claimed
    /// otherwise (principle §18 — fixed).
    pub async fn with_llm(
        project_root: PathBuf,
        available_models: Vec<ModelCharacteristics>,
        llm_agent: Option<Arc<dyn crate::agent::Agent>>,
    ) -> Self {
        let rules_prompts = Self::load_rules(&project_root).await;

        info!(
            rules_count = rules_prompts.len(),
            models = available_models.len(),
            "self-evolution agent initialized"
        );

        Self {
            adaptive_selector: AdaptiveModelSelector::with_static_strategy(
                ModelSelectionStrategy::Balanced,
            ),
            rules_prompts,
            project_root,
            available_models,
            llm_agent,
        }
    }

    /// Analyze a code target (file or module) and produce a structured report.
    ///
    /// Reads the source code, counts types/dependencies/public items/unsafe blocks,
    /// and generates findings.
    pub async fn analyze_code(&self, target: &str) -> Result<Report, SelfEvolutionAgentError> {
        let target_path = self.resolve_path(target)?;

        if !target_path.exists() {
            return Err(SelfEvolutionAgentError::TargetNotFound(target.to_string()));
        }

        let content = fs::read_to_string(&target_path).await?;
        let mut report = Report::new(target.to_string());

        // Count source lines
        report.source_lines = content.lines().count();

        // Count type definitions
        report.type_count = content
            .lines()
            .filter(|l| {
                let t = l.trim();
                t.starts_with("struct ")
                    || t.starts_with("enum ")
                    || t.starts_with("trait ")
                    || t.starts_with("type ")
                    || t.starts_with("union ")
                    || t.starts_with("pub struct ")
                    || t.starts_with("pub enum ")
                    || t.starts_with("pub trait ")
                    || t.starts_with("pub type ")
                    || t.starts_with("pub union ")
            })
            .count();

        // Count dependency (use) statements
        report.dep_count = content
            .lines()
            .filter(|l| l.trim().starts_with("use ") || l.trim().starts_with("pub use "))
            .count();

        // Count public items
        report.public_items = content
            .lines()
            .filter(|l| {
                let t = l.trim();
                t.starts_with("pub ") && !t.starts_with("pub use ")
            })
            .count();

        // Count unsafe blocks
        report.unsafe_blocks = content.matches("unsafe {").count();

        // Count TODO/FIXME comments
        report.todo_count = content
            .lines()
            .filter(|l| {
                let t = l.trim();
                t.contains("TODO") || t.contains("FIXME") || t.contains("HACK")
            })
            .count();

        // Generate findings based on analysis
        if report.unsafe_blocks > 0 {
            report.findings.push(format!(
                "{} unsafe blocks found — review for safety",
                report.unsafe_blocks
            ));
        }
        if report.todo_count > 0 {
            report.findings.push(format!(
                "{} TODO/FIXME markers — consider addressing",
                report.todo_count
            ));
        }
        if report.source_lines > 500 {
            report.findings.push(format!(
                "Large file ({} lines) — consider splitting",
                report.source_lines
            ));
        }
        if report.type_count == 0 && report.target.ends_with(".rs") {
            report
                .findings
                .push("No type definitions found in Rust source".to_string());
        }

        // Assess risk based on findings
        report.risk = if report.unsafe_blocks > 5 || report.target.contains("security") {
            RiskLevel::High
        } else if report.unsafe_blocks > 0 || report.target.contains("core") {
            RiskLevel::Medium
        } else {
            RiskLevel::Low
        };

        debug!(target = %target, lines = report.source_lines, "code analysis complete");
        Ok(report)
    }

    /// Generate a code patch based on an analysis report and instruction.
    ///
    /// Uses the RULES/ content as system prompts to guide patch generation
    /// toward the project's coding standards.
    ///
    /// # Arguments
    /// * `analysis` - The analysis report produced by `analyze_code`.
    /// * `instruction` - Natural language instruction describing what to change.
    pub async fn generate_patch(
        &self,
        analysis: &Report,
        instruction: &str,
    ) -> Result<CodePatch, SelfEvolutionAgentError> {
        if instruction.trim().is_empty() {
            return Err(SelfEvolutionAgentError::InvalidInstruction(
                "instruction cannot be empty".to_string(),
            ));
        }

        // Read the target file's current content
        let target_path = self.resolve_path(&analysis.target)?;
        let content = fs::read_to_string(&target_path).await?;
        let original_lines: Vec<(usize, String)> = content
            .lines()
            .enumerate()
            .map(|(i, l)| (i + 1, l.to_string()))
            .collect();

        // Build the system context from RULES
        let system_context = self.build_system_context(analysis, instruction);

        // Use the model selector to pick the best model for code generation.
        // The selection is advisory (observability only): synthesis is heuristic
        // unless an LLM agent is injected. Production passes
        // `available_models: Vec::new()`, so `select_model` is always None there;
        // the former "model-aware synthesis" branch duplicated the fallback
        // verbatim and has been removed.
        let selected_model = self.select_model("code_generation");
        info!(
            target = %analysis.target,
            instruction = %instruction,
            model = ?selected_model,
            "generating patch using selected model"
        );

        // Use LLM agent when available (BLUE56-B03), otherwise fall back to heuristic
        let patched_lines = if let Some(ref agent) = self.llm_agent {
            info!(
                target = %analysis.target,
                instruction = %instruction,
                "using LLM agent for patch generation"
            );
            let llm_instruction = format!(
                "Generate a code patch for the file '{}'.\n\nInstruction: {}\n\nSystem context:\n{}\n\nCurrent code:\n```\n{}\n```\n\nReturn ONLY the new file content, wrapped in a code block.",
                analysis.target,
                instruction,
                system_context.join("\n"),
                content
            );
            let messages = vec![
                crate::agent::Message {
                    role: "system".to_string(),
                    content: "You are a code evolution agent. Generate precise code patches."
                        .to_string(),
                },
                crate::agent::Message {
                    role: "user".to_string(),
                    content: llm_instruction,
                },
            ];
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let sender = crate::agent::StreamingSender::from(tx);
            if let Err(e) = agent.chat(messages, None, None, sender).await {
                warn!("LLM agent patch generation failed: {e}, falling back to heuristic");
                self.synthesize_patch_lines(&content, instruction)
            } else {
                // Bounded collection: the patch output flows into file
                // edits, so an unbounded model stream must not balloon it.
                let llm_output =
                    crate::acp::helpers::conversation::drain_channel_capped(&mut rx).await;
                if llm_output.trim().is_empty() {
                    self.synthesize_patch_lines(&content, instruction)
                } else {
                    // Parse the LLM output to extract patched lines
                    // Try to extract content from markdown code fences first
                    let extracted = self.extract_code_from_markdown(&llm_output);
                    let source = extracted.as_deref().unwrap_or(&llm_output);
                    let patched: Vec<(usize, String)> = source
                        .lines()
                        .enumerate()
                        .map(|(i, l)| (i + 1, l.to_string()))
                        .collect();
                    if patched.is_empty() {
                        self.synthesize_patch_lines(&content, instruction)
                    } else {
                        patched
                    }
                }
            }
        } else {
            // No LLM agent injected — heuristic synthesis. `selected_model` is
            // advisory only (logged above); it does not change the synthesis
            // path (previously a verbatim duplicate branch claimed otherwise).
            self.synthesize_patch_lines(&content, instruction)
        };

        if patched_lines.is_empty() {
            return Err(SelfEvolutionAgentError::NoChangesGenerated);
        }

        let patch = CodePatch::new(
            analysis.target.clone(),
            original_lines,
            patched_lines,
            format!(
                "Self-evolution: {}\n\nSystem context:\n{}\n\nAnalysis: {}",
                instruction,
                system_context.join("\n"),
                analysis.summary()
            ),
        );

        debug!(
            target = %patch.target_file,
            patch_id = ?patch.patch_id,
            "patch generated"
        );

        Ok(patch)
    }

    /// Attempt to fix compile errors by refining an existing patch.
    ///
    /// Retries up to `MAX_FIX_ITERATIONS` (5) times, each time incorporating
    /// the compiler error output to produce a corrected patch.
    ///
    /// # Arguments
    /// * `errors` - Compile error lines from the build output.
    /// * `current_patch` - The patch that introduced the errors.
    pub async fn fix_compile_errors(
        &self,
        errors: &[String],
        current_patch: &CodePatch,
    ) -> Result<CodePatch, SelfEvolutionAgentError> {
        if errors.is_empty() {
            return Ok(current_patch.clone());
        }

        info!(
            target = %current_patch.target_file,
            error_count = errors.len(),
            "attempting to fix compile errors"
        );

        let mut patch = current_patch.clone();
        let mut iteration = 0;

        while iteration < MAX_FIX_ITERATIONS {
            iteration += 1;

            // Read the current state of the target file
            let target_path = self.resolve_path(&patch.target_file)?;
            let content = match fs::read_to_string(&target_path).await {
                Ok(c) => c,
                Err(_) => break,
            };

            let original_lines: Vec<(usize, String)> = content
                .lines()
                .enumerate()
                .map(|(i, l)| (i + 1, l.to_string()))
                .collect();

            // Analyze errors to determine which lines to fix
            let (fixed_lines, fixes_applied) = self.resolve_errors(&content, errors);

            if fixes_applied == 0 {
                debug!(
                    iteration = iteration,
                    "no automatic fixes possible for remaining errors"
                );
                break;
            }

            patch = CodePatch::new(
                patch.target_file.clone(),
                original_lines,
                fixed_lines,
                format!(
                    "Fix iteration {}: {} errors addressed. Previous reasoning: {}",
                    iteration,
                    errors.len(),
                    patch.reasoning
                ),
            );

            debug!(
                target = %patch.target_file,
                iteration = iteration,
                fixes = fixes_applied,
                "compile error fix iteration"
            );
        }

        if iteration >= MAX_FIX_ITERATIONS {
            warn!(
                target = %patch.target_file,
                "max fix iterations reached with remaining errors"
            );
            // Return the best-effort patch rather than failing
        }

        Ok(patch)
    }

    /// Assess the risk level of applying a given code patch.
    ///
    /// Considers the target file path, the type of changes, and the
    /// number of lines modified.
    pub fn assess_risk(&self, patch: &CodePatch) -> RiskLevel {
        let target = &patch.target_file;

        // Critical paths
        if target.contains("security")
            || target.contains("auth")
            || target.contains("crypto")
            || target.contains("encryption")
        {
            return RiskLevel::Critical;
        }

        // High-risk paths
        if target.contains("core/")
            || target.contains("governance/")
            || target.contains("protocol/")
            || target.contains("orchestration/")
            || target == "src/lib.rs"
            || target == "src/main.rs"
        {
            return RiskLevel::High;
        }

        // Check the magnitude of changes
        let total_changes = patch.original_lines.len() + patch.patched_lines.len();
        if total_changes > 100 {
            return RiskLevel::High;
        }
        if total_changes > 30 {
            return RiskLevel::Medium;
        }

        // Check for risky patterns in the patch
        let patch_text = patch.diff.to_lowercase();
        if patch_text.contains("unsafe")
            || patch_text.contains("#[allow")
            || patch_text.contains("transmute")
        {
            return RiskLevel::Medium;
        }

        RiskLevel::Low
    }

    /// Get the loaded RULES prompts.
    #[inline]
    pub fn rules_prompts(&self) -> &[String] {
        &self.rules_prompts
    }

    /// Select the best model for a given task type.
    ///
    /// Uses the adaptive UCB selector when sufficient data is available,
    /// falling back to the balanced static strategy during cold start.
    pub fn select_model(&self, task_type: &str) -> Option<String> {
        let criteria = match task_type {
            "code_generation" => SelectionCriteria::code_generation(),
            "analysis" => SelectionCriteria::minimal(),
            "fix_errors" => SelectionCriteria::complex(),
            _ => SelectionCriteria::fast_response(),
        };

        self.adaptive_selector
            .select_with_static_fallback(&criteria, &self.available_models, None)
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Resolve a target path relative to the project root.
    fn resolve_path(&self, target: &str) -> Result<PathBuf, SelfEvolutionAgentError> {
        let path = if Path::new(target).is_absolute() {
            PathBuf::from(target)
        } else {
            self.project_root.join(target)
        };

        // Canonicalize if possible
        match path.canonicalize() {
            Ok(p) => Ok(p),
            Err(_) => Ok(path),
        }
    }

    /// Load all rule files from the RULES/ directory as system prompts.
    async fn load_rules(project_root: &Path) -> Vec<String> {
        let rules_dir = project_root.join(RULES_DIR);
        let mut prompts = Vec::new();

        if !rules_dir.exists() {
            warn!("RULES directory not found at {:?}", rules_dir);
            return prompts;
        }

        let mut entries = match fs::read_dir(&rules_dir).await {
            Ok(e) => e,
            Err(_) => return prompts,
        };

        let mut file_names = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap_or(None) {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if ext == "md" || ext == "txt" {
                        file_names.push(path);
                    }
                }
            }
        }

        // Sort for deterministic ordering
        file_names.sort();

        for path in file_names {
            match fs::read_to_string(&path).await {
                Ok(content) => {
                    if !content.trim().is_empty() {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        prompts.push(format!("--- {} ---\n{}", name, content));
                        debug!("Loaded RULES prompt from {:?}", path);
                    }
                }
                Err(e) => {
                    warn!("Failed to read rule file {:?}: {}", path, e);
                }
            }
        }

        info!(
            count = prompts.len(),
            "RULES prompts loaded for system context"
        );

        prompts
    }

    /// Build the system context string from RULES, analysis, and instruction.
    fn build_system_context(&self, analysis: &Report, instruction: &str) -> Vec<String> {
        let mut context = Vec::new();

        context.push("=== PROJECT RULES ===".to_string());
        context.extend(self.rules_prompts.clone());

        context.push("\n=== CURRENT ANALYSIS ===".to_string());
        context.push(analysis.summary());

        context.push("\n=== INSTRUCTION ===".to_string());
        context.push(instruction.to_string());

        context.push("\n=== RISK ASSESSMENT ===".to_string());
        context.push(format!("Current risk level: {}", analysis.risk.label()));

        context
    }

    /// Extract code from markdown code fences (triple backticks).
    /// Returns `None` if no fences are found.
    fn extract_code_from_markdown(&self, output: &str) -> Option<String> {
        let mut in_fence = false;
        let mut code_lines: Vec<&str> = Vec::new();
        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("```") {
                if in_fence {
                    // End of fence — return collected lines
                    return Some(code_lines.join("\n"));
                } else {
                    // Start of fence — begin collecting
                    in_fence = true;
                    code_lines.clear();
                }
            } else if in_fence {
                code_lines.push(line);
            }
        }
        // If we ended while still in a fence, return what we have
        if !code_lines.is_empty() {
            return Some(code_lines.join("\n"));
        }
        None
    }

    /// Synthesize patch lines from content and instruction.
    ///
    /// Tries, in order:
    /// 1. Unified diff format (`@@ -line,count +line,count @@` hunks)
    /// 2. Markdown fenced code blocks with inline file references
    /// 3. Inline `path:line:content` patterns
    /// 4. Content-aware keyword heuristic (the original fallback)
    fn synthesize_patch_lines(&self, content: &str, instruction: &str) -> Vec<(usize, String)> {
        // ── Phase 1: Try unified diff format ───────────────────────────
        // If the instruction contains `@@ -line,count +line,count @@` hunks,
        // extract the new ("+") lines as patches.
        if instruction.contains("@@ -") && instruction.contains(" @@") {
            if let Some(patched) = self.parse_unified_diff_patch(content, instruction) {
                return patched;
            }
        }

        // ── Phase 2: Try markdown code blocks with file references ─────
        // Look for a code fence whose language hint (e.g. "rust:src/lib.rs")
        // or preceding text mentions a file path.
        if let Some(patched) = self.parse_fenced_code_patch(content, instruction) {
            return patched;
        }

        // ── Phase 3: Try inline path:line:content patterns ─────────────
        // Look for lines like `filename.rs:42:+ new code` or `path/to/file:15: content`.
        if let Some(patched) = self.parse_inline_path_patch(content, instruction) {
            return patched;
        }

        // ── Phase 4: Keyword-based heuristic (original fallback) ───────
        self.synthesize_keyword_heuristic(content, instruction)
    }

    /// Parse a unified diff embedded in the instruction.
    /// Returns `None` if the instruction does not contain a valid diff.
    fn parse_unified_diff_patch(
        &self,
        _content: &str,
        instruction: &str,
    ) -> Option<Vec<(usize, String)>> {
        let lines: Vec<&str> = instruction.lines().collect();
        let mut patched: Vec<(usize, String)> = Vec::new();
        let mut base_line: Option<usize> = None;

        // Prefix detection must use the RAW line, not `line.trim()`: the
        // leading space of a context line IS its marker, and trimming it made
        // context lines unrecognizable (and silently let "-" removals advance
        // the new-file counter).
        for line in &lines {
            // Unified diff hunk header: @@ -old_line,old_count +new_line,new_count @@
            if line.starts_with("@@ -") && line.contains(" @@") {
                // Parse the target line number from the "+new_line,new_count" part
                let hunk_parts: Vec<&str> = line.split_whitespace().collect();
                if hunk_parts.len() >= 2 {
                    // hunk_parts[1] is "-old_line,old_count", hunk_parts[2] is "+new_line,new_count"
                    let target = hunk_parts
                        .iter()
                        .find(|p| p.starts_with('+'))
                        .unwrap_or(&"+0");
                    let target = target
                        .trim_start_matches('+')
                        .split(',')
                        .next()
                        .unwrap_or("0");
                    base_line = target.parse::<usize>().ok().filter(|ln| *ln > 0);
                }
                continue;
            }

            // Addition lines ("+") produce patches and advance the counter.
            if let Some(rest) = line.strip_prefix('+') {
                let content_line = rest.trim_end().to_string();
                if let Some(bl) = base_line.as_mut() {
                    patched.push((*bl, content_line));
                    *bl = bl.saturating_add(1);
                }
            } else if line.starts_with(' ') {
                // Context lines advance the new-file counter. Removal lines
                // ("-") exist only in the OLD file, so they must NOT advance
                // it — advancing would skew every patched line number after
                // the removal. (The previous code trimmed the leading space,
                // so context lines were never recognized and removals advanced
                // the counter instead.)
                if let Some(bl) = base_line.as_mut() {
                    *bl = bl.saturating_add(1);
                }
            }
        }

        if patched.is_empty() {
            None
        } else {
            Some(patched)
        }
    }

    /// Parse a markdown fenced code block preceded by a file-path reference.
    /// Looks for lines like `path/to/file.rs:`, `--- path/to/file.rs ---`,
    /// or `file.rs` immediately before a code fence.
    fn parse_fenced_code_patch(
        &self,
        _content: &str,
        instruction: &str,
    ) -> Option<Vec<(usize, String)>> {
        // Collect the content of every fenced code block in the instruction
        let mut in_fence = false;
        let mut fence_lines: Vec<String> = Vec::new();

        for line in instruction.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("```") {
                if in_fence {
                    // End of fence: produce patch lines from fence_lines.
                    // If we found useful content, return it — otherwise clear and keep looking.
                    let patched: Vec<(usize, String)> = fence_lines
                        .iter()
                        .enumerate()
                        .map(|(i, l)| (i + 1, l.clone()))
                        .collect();
                    if !patched.is_empty() {
                        return Some(patched);
                    }
                    fence_lines.clear();
                    in_fence = false;
                } else {
                    // Start of fence — reset
                    in_fence = true;
                    fence_lines.clear();
                }
            } else if in_fence {
                fence_lines.push(line.to_string());
            }
        }

        // Handle unclosed fence (common in truncated output)
        if !fence_lines.is_empty() {
            let patched: Vec<(usize, String)> = fence_lines
                .iter()
                .enumerate()
                .map(|(i, l)| (i + 1, l.clone()))
                .collect();
            if !patched.is_empty() {
                return Some(patched);
            }
        }

        None
    }

    /// Parse inline `path:line:content` or `path line: content` patterns
    /// where a filename and line number precede a change.
    ///
    /// Matches patterns like:
    /// - `src/main.rs:42:+ println!("hello");`
    /// - `src/lib.rs:15:  let x = 1;`
    /// - `file.rs:32:- old_line`
    fn parse_inline_path_patch(
        &self,
        _content: &str,
        instruction: &str,
    ) -> Option<Vec<(usize, String)>> {
        let mut patched: Vec<(usize, String)> = Vec::new();

        // Look for lines matching: filepath:line_number:+ content or filepath:line_number: content
        for line in instruction.lines() {
            let trimmed = line.trim();
            // Match pattern: path/to/file.ext:12345:+rest or path/to/file.ext:12345:rest
            if let Some(colon_pos) = trimmed.rfind(':') {
                // Check if the character before the colon is a digit (end of line number)
                if colon_pos > 0 {
                    let before_colon = &trimmed[..colon_pos];
                    let after_colon = &trimmed[colon_pos + 1..];
                    // Find the colon that separates path from line number
                    if let Some(path_colon) = before_colon.rfind(':') {
                        let line_num_str = &before_colon[path_colon + 1..];
                        if let Ok(ln) = line_num_str.parse::<usize>() {
                            // Reject line 0: apply_to_file indexes with `ln - 1`,
                            // which would underflow (panic in debug).
                            if ln == 0 {
                                continue;
                            }
                            // Check that the path-like part before the line number looks like a file
                            let path_part = &before_colon[..path_colon];
                            if path_part.contains('.')
                                || path_part.contains('/')
                                || path_part.contains('\\')
                            {
                                let content_part = if after_colon.starts_with('+')
                                    || after_colon.starts_with('-')
                                {
                                    after_colon[1..].trim().to_string()
                                } else {
                                    after_colon.to_string()
                                };
                                if !content_part.is_empty() {
                                    patched.push((ln, content_part));
                                }
                            }
                        }
                    }
                }
            }
        }

        if patched.is_empty() {
            None
        } else {
            Some(patched)
        }
    }

    /// Content-aware keyword heuristic: find functional lines (non-comment, non-empty)
    /// that semantically match the instruction's intent.
    fn synthesize_keyword_heuristic(
        &self,
        content: &str,
        instruction: &str,
    ) -> Vec<(usize, String)> {
        let ins_lower = instruction.to_lowercase();
        let mut patched = Vec::new();

        let keywords: Vec<&str> = ins_lower
            .split_whitespace()
            .filter(|w| w.len() > 3)
            .collect();

        if keywords.is_empty() {
            return patched;
        }

        // Detect the kind of change requested
        let is_add = ins_lower.contains("add") || ins_lower.contains("insert");
        let is_remove = ins_lower.contains("remove") || ins_lower.contains("delete");
        let is_fix = ins_lower.contains("fix") || ins_lower.contains("correct");

        // Collect the lines once — `content.lines().nth(i)` inside the loop
        // rescans from the start of the string on every call (O(n²)).
        let lines: Vec<&str> = content.lines().collect();

        for (i, line) in lines.iter().enumerate() {
            let line_lower = line.to_lowercase();
            let trimmed = line.trim();

            // Skip pure comments and empty lines
            if trimmed.is_empty() || trimmed.starts_with("//") {
                continue;
            }

            // Score how well this line matches the instruction
            let keyword_match = keywords.iter().filter(|k| line_lower.contains(*k)).count();
            if keyword_match == 0 {
                continue;
            }

            if is_remove {
                // Skip lines that match removal keywords
                continue;
            }

            // For fix/add operations, include matching lines and context
            let include_surrounding = is_fix || is_add;
            if include_surrounding {
                // Include the matching line plus adjacent context
                patched.push((i + 1, (*line).to_string()));
                // Also include the next line for context if it's not already included
                if let Some(next) = lines.get(i + 1) {
                    let next_trimmed = next.trim();
                    if !next_trimmed.is_empty() && !next_trimmed.starts_with("//") {
                        patched.push((i + 2, (*next).to_string()));
                    }
                }
            } else {
                patched.push((i + 1, (*line).to_string()));
            }
        }

        // Deduplicate by line number
        patched.sort_by_key(|(num, _)| *num);
        patched.dedup_by_key(|(num, _)| *num);

        patched
    }

    /// Try to automatically resolve compile errors by fixing obvious issues.
    /// Returns (fixed_lines, count_of_fixes_applied).
    fn resolve_errors(&self, content: &str, errors: &[String]) -> (Vec<(usize, String)>, usize) {
        let mut fixes = 0usize;
        let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();

        for error in errors {
            let err_lower = error.to_lowercase();

            // Extract line number from error message (format: "error: ... --> file.rs:42:1")
            let line_num = self.extract_line_number(error);

            if let Some(ln) = line_num {
                if ln > 0 && ln <= lines.len() {
                    let line = &lines[ln - 1];
                    let trimmed = line.trim();

                    // Handle "unused variable" — add underscore prefix
                    if err_lower.contains("unused variable") || err_lower.contains("unused import")
                    {
                        if trimmed.starts_with("let ") && !trimmed.starts_with("let _") {
                            let parts: Vec<&str> = trimmed.splitn(3, ' ').collect();
                            if parts.len() >= 2 {
                                lines[ln - 1] = format!(
                                    "    let _{}{}",
                                    &parts[1],
                                    if parts.len() > 2 {
                                        let rest = parts[2..].join(" ");
                                        format!(" {}", rest)
                                    } else {
                                        String::new()
                                    }
                                );
                                fixes += 1;
                            }
                        } else if line.contains("use ") || line.contains("extern crate") {
                            // Comment out unused imports
                            if !line.trim_start().starts_with("//") {
                                lines[ln - 1] = format!("// {}", line);
                                fixes += 1;
                            }
                        }
                    }
                    // Handle "missing semicolon"
                    else if err_lower.contains("expected `;`")
                        || err_lower.contains("missing `;`")
                    {
                        if !trimmed.ends_with(';')
                            && !trimmed.ends_with('}')
                            && !trimmed.ends_with('{')
                            && !trimmed.starts_with("//")
                        {
                            lines[ln - 1] = format!("{};", line);
                            fixes += 1;
                        }
                    }
                    // Handle "dead code"
                    else if err_lower.contains("dead code")
                        && !trimmed.starts_with("#[allow(dead_code)]")
                        && !trimmed.starts_with("#[allow")
                    {
                        let indent = line
                            .chars()
                            .take_while(|c| c.is_whitespace())
                            .collect::<String>();
                        let prev_idx = ln.saturating_sub(2);
                        if prev_idx > 0 && prev_idx < lines.len() {
                            lines.insert(prev_idx, format!("{}#[allow(dead_code)]", indent));
                            fixes += 1;
                        }
                    }
                }
            }
        }

        let result: Vec<(usize, String)> = lines
            .into_iter()
            .enumerate()
            .map(|(i, l)| (i + 1, l))
            .collect();

        (result, fixes)
    }

    /// Extract the line number from a compiler error message.
    fn extract_line_number(&self, error: &str) -> Option<usize> {
        // Pattern: " --> file.rs:42:1" or "file.rs:42:1:"
        for pattern in &[" --> ", ":"] {
            if let Some(pos) = error.rfind(pattern) {
                let after = &error[pos + pattern.len()..];
                let parts: Vec<&str> = after.split(':').collect();
                if parts.len() >= 2 {
                    // Skip the file part (first), take the line number
                    let line_part = if parts[0].contains('/')
                        || parts[0].contains('\\')
                        || parts[0].contains('.')
                    {
                        if parts.len() >= 2 {
                            parts[1]
                        } else {
                            continue;
                        }
                    } else {
                        parts[0]
                    };
                    if let Ok(ln) = line_part.parse::<usize>() {
                        return Some(ln);
                    }
                }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Create a test agent in a sync context.
    /// Uses `block_on` since `SelfEvolutionAgent::new` is async and
    /// this factory is called from `#[test]` (non-async) contexts.
    fn create_test_agent() -> (SelfEvolutionAgent, TempDir) {
        let tmp_dir = TempDir::new().expect("should create temp dir for test agent");
        let project_root = tmp_dir.path().to_path_buf();
        let models = vec![ModelCharacteristics {
            id: "test-model".to_string(),
            cost_per_request_cents: 1,
            latency_ms: 100,
            capability_tier: 3,
            supports_vision: false,
            supports_function_calling: true,
            excels_at_code: true,
            context_window: 8192,
        }];

        let rt =
            tokio::runtime::Runtime::new().expect("should create tokio runtime for test agent");
        let agent = rt.block_on(SelfEvolutionAgent::new(project_root.clone(), models));
        (agent, tmp_dir)
    }

    /// Create a test agent in an async context (for `#[tokio::test]`).
    /// Avoids nested runtime creation that would panic in async contexts.
    async fn create_test_agent_async() -> (SelfEvolutionAgent, TempDir) {
        let tmp_dir = TempDir::new().expect("should create temp dir for async test agent");
        let project_root = tmp_dir.path().to_path_buf();
        let models = vec![ModelCharacteristics {
            id: "test-model".to_string(),
            cost_per_request_cents: 1,
            latency_ms: 100,
            capability_tier: 3,
            supports_vision: false,
            supports_function_calling: true,
            excels_at_code: true,
            context_window: 8192,
        }];

        let agent = SelfEvolutionAgent::new(project_root.clone(), models).await;
        (agent, tmp_dir)
    }

    #[tokio::test]
    async fn test_risk_level_labels() {
        assert_eq!(RiskLevel::Low.label(), "low");
        assert_eq!(RiskLevel::High.label(), "high");
        assert_eq!(RiskLevel::Critical.label(), "critical");
    }

    #[test]
    fn test_risk_level_human_approval() {
        assert!(!RiskLevel::Low.requires_human_approval());
        assert!(!RiskLevel::Medium.requires_human_approval());
        assert!(RiskLevel::High.requires_human_approval());
        assert!(RiskLevel::Critical.requires_human_approval());
    }

    #[test]
    fn test_risk_level_parse() {
        assert_eq!(RiskLevel::from_label("high"), RiskLevel::High);
        assert_eq!(RiskLevel::from_label("CRITICAL"), RiskLevel::Critical);
        assert_eq!(RiskLevel::from_label("unknown"), RiskLevel::Medium);
    }

    #[tokio::test]
    async fn test_analyze_code_file_not_found() {
        let (agent, _tmp_dir) = create_test_agent_async().await;
        let result = agent.analyze_code("nonexistent.rs").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_analyze_code_rust_file() {
        let (agent, tmp_dir) = create_test_agent_async().await;
        let test_file = tmp_dir.path().join("test.rs");
        fs::write(
            &test_file,
            "pub struct Foo {}\nuse std::collections::HashMap;\nfn bar() { unsafe {} }\n// TODO: fix\n",
        )
        .await
        .expect("should write test file");

        let report = agent
            .analyze_code("test.rs")
            .await
            .expect("should analyze test file");
        assert_eq!(report.source_lines, 4);
        assert!(report.type_count >= 1);
        assert!(report.dep_count >= 1);
        assert!(report.unsafe_blocks >= 1);
        assert!(report.todo_count >= 1);
    }

    #[tokio::test]
    async fn test_generate_patch_empty_instruction() {
        let (agent, _tmp_dir) = create_test_agent_async().await;
        let report = Report::new("test.rs".to_string());
        let result = agent.generate_patch(&report, "").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_assess_risk_critical_paths() {
        let (agent, _tmp_dir) = create_test_agent_async().await;
        let patch = CodePatch::new(
            "src/security/auth.rs".to_string(),
            vec![],
            vec![],
            "test".to_string(),
        );
        assert_eq!(agent.assess_risk(&patch), RiskLevel::Critical);
    }

    #[tokio::test]
    async fn test_assess_risk_high_paths() {
        let (agent, _tmp_dir) = create_test_agent_async().await;
        let patch = CodePatch::new(
            "src/core/mod.rs".to_string(),
            vec![],
            vec![],
            "test".to_string(),
        );
        assert_eq!(agent.assess_risk(&patch), RiskLevel::High);
    }

    #[tokio::test]
    async fn test_assess_risk_low_paths() {
        let (agent, _tmp_dir) = create_test_agent_async().await;
        let patch = CodePatch::new(
            "src/agents/test.rs".to_string(),
            vec![],
            vec![],
            "test".to_string(),
        );
        assert_eq!(agent.assess_risk(&patch), RiskLevel::Low);
    }

    #[tokio::test]
    async fn test_assess_risk_medium_for_unsafe() {
        let (agent, _tmp_dir) = create_test_agent_async().await;
        // Create a patch where the diff contains "unsafe" by providing original
        // and patched lines that produce a diff hunk with the unsafe keyword.
        let patch = CodePatch::new(
            "src/test.rs".to_string(),
            vec![(1, "fn foo() {}".to_string())],
            vec![(1, "unsafe fn foo() {}".to_string())],
            "added unsafe".to_string(),
        );
        assert!(
            patch.diff.contains("unsafe"),
            "diff should contain 'unsafe' keyword: {} \n {}",
            patch.diff,
            "CodePatch should produce diff with unsafe in it"
        );
        assert_eq!(agent.assess_risk(&patch), RiskLevel::Medium);
    }

    #[test]
    fn test_report_new() {
        let report = Report::new("src/main.rs".to_string());
        assert_eq!(report.target, "src/main.rs");
        assert_eq!(report.risk, RiskLevel::Low);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn test_report_summary() {
        let mut report = Report::new("src/lib.rs".to_string());
        report.source_lines = 100;
        report.type_count = 5;
        assert!(report.summary().contains("src/lib.rs"));
        assert!(report.summary().contains("100"));
    }

    #[test]
    fn test_extract_line_number() {
        let (agent, _tmp_dir) = create_test_agent();
        let err = "error[E0308]: type mismatch\n --> src/main.rs:42:1\n";
        assert_eq!(agent.extract_line_number(err), Some(42));
    }

    #[test]
    fn test_extract_line_number_no_match() {
        let (agent, _tmp_dir) = create_test_agent();
        assert_eq!(agent.extract_line_number("unknown error"), None);
    }

    #[tokio::test]
    async fn test_load_rules_empty_dir() {
        let tmp_dir = TempDir::new().expect("should create temp dir for load_rules test");
        let rules = SelfEvolutionAgent::load_rules(tmp_dir.path()).await;
        assert!(rules.is_empty());
    }

    #[test]
    fn test_self_evolution_agent_select_model() {
        let (agent, _tmp_dir) = create_test_agent();
        let model = agent.select_model("code_generation");
        assert!(model.is_some());
        assert_eq!(
            model.expect("selected model should be present"),
            "test-model"
        );
    }

    #[test]
    fn test_resolve_errors_unused_variable() {
        let (agent, _tmp_dir) = create_test_agent();
        let content = "fn main() {\n    let x = 42;\n}";
        // Use a Rust compiler-style error message with proper line number format
        let errors = vec!["warning: unused variable `x`\n --> src/main.rs:2:1".to_string()];
        let (lines, fixes) = agent.resolve_errors(content, &errors);
        assert!(fixes > 0);
        assert!(lines.iter().any(|(_, l)| l.contains("_x")));
    }

    #[test]
    fn test_resolve_errors_missing_semicolon() {
        let (agent, _tmp_dir) = create_test_agent();
        let content = "fn main() {\n    let x = 42\n}";
        // Use a Rust compiler-style error message with proper line number format
        let errors = vec!["error: expected `;`\n --> src/main.rs:2:1".to_string()];
        let (lines, fixes) = agent.resolve_errors(content, &errors);
        assert!(fixes > 0);
        assert!(lines.iter().any(|(_, l)| l.ends_with(';')));
    }

    #[test]
    fn test_parse_unified_diff_rejects_zero_line() {
        // Regression: `@@ -0,0 +0,1 @@` produced a 0-based line number that
        // underflowed `ln - 1` in apply_to_file (panic in debug builds). The
        // parser must drop zero target lines.
        let (agent, _tmp_dir) = create_test_agent();
        let instruction = "@@ -0,0 +0,1 @@\n+let x = 1;\n";
        let patched = agent.parse_unified_diff_patch("", instruction);
        assert!(patched.is_none() || patched.as_ref().unwrap().iter().all(|(ln, _)| *ln >= 1));
    }

    #[test]
    fn test_parse_unified_diff_removal_lines_do_not_advance_new_file_line() {
        // Regression: removal ("-") lines exist only in the OLD file, so they
        // must not advance the new-file line counter. The previous code
        // advanced on removals (and left a no-op `if` block whose comment
        // claimed the opposite), skewing every patched line number after a
        // removal.
        let (agent, _tmp_dir) = create_test_agent();
        // Hunk: old lines 3-5 removed, new file context starts at line 3.
        let instruction = "@@ -3,5 +3,2 @@\n-fn removed() {}\n fn kept() {}\n+fn added() {}\n";
        let patched = agent
            .parse_unified_diff_patch("", instruction)
            .expect("diff should parse");
        // Patches are produced for ADDITION lines only; the context line at
        // new-file line 3 advances the counter (the removal does not), so the
        // addition lands at new-file line 4.
        assert_eq!(patched, vec![(4, "fn added() {}".to_string())]);
    }

    #[test]
    fn test_parse_inline_path_patch_rejects_zero_line() {
        let (agent, _tmp_dir) = create_test_agent();
        let instruction = "src/lib.rs:0:+ let x = 1;\n";
        let patched = agent.parse_inline_path_patch("", instruction);
        assert!(patched.is_none() || patched.as_ref().unwrap().iter().all(|(ln, _)| *ln >= 1));
    }
}
