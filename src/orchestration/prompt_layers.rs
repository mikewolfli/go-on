//! BLUE35 S6: Prompt 8-Layer Architecture (ARCH-03)
//!
//! Defines a layered prompt processing pipeline:
//! L1: System Prompt Assembly
//! L2: Role & Identity Injection
//! L3: Context Window Optimization
//! L4: Task Decomposition Guidance
//! L5: Safety & Constraint Enforcement
//! L6: Output Format Specification
//! L7: Chain-of-Thought Triggers
//! L8: Meta-Cognitive Instructions

use serde::{Deserialize, Serialize};

/// The 8 prompt layers
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PromptLayer {
    L1SystemPrompt,
    L2RoleIdentity,
    L3ContextWindow,
    L4TaskDecomposition,
    L5SafetyConstraints,
    L6OutputFormat,
    L7ChainOfThought,
    L8MetaCognitive,
}

impl PromptLayer {
    pub fn all() -> Vec<PromptLayer> {
        vec![
            PromptLayer::L1SystemPrompt,
            PromptLayer::L2RoleIdentity,
            PromptLayer::L3ContextWindow,
            PromptLayer::L4TaskDecomposition,
            PromptLayer::L5SafetyConstraints,
            PromptLayer::L6OutputFormat,
            PromptLayer::L7ChainOfThought,
            PromptLayer::L8MetaCognitive,
        ]
    }

    pub fn name(&self) -> &'static str {
        match self {
            PromptLayer::L1SystemPrompt => "system_prompt",
            PromptLayer::L2RoleIdentity => "role_identity",
            PromptLayer::L3ContextWindow => "context_window",
            PromptLayer::L4TaskDecomposition => "task_decomposition",
            PromptLayer::L5SafetyConstraints => "safety_constraints",
            PromptLayer::L6OutputFormat => "output_format",
            PromptLayer::L7ChainOfThought => "chain_of_thought",
            PromptLayer::L8MetaCognitive => "meta_cognitive",
        }
    }
}

/// A prompt segment tagged with its layer origin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptSegment {
    pub layer: PromptLayer,
    pub content: String,
    pub priority: u32,
}

/// The assembled multi-layer prompt
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayeredPrompt {
    pub segments: Vec<PromptSegment>,
    pub assembled: String,
    pub token_estimate: usize,
}

/// Prompt assembler that merges segments from all layers
pub struct PromptAssembler;

impl PromptAssembler {
    pub fn assemble(segments: Vec<PromptSegment>) -> LayeredPrompt {
        let mut sorted = segments;
        sorted.sort_by_key(|s| s.priority);

        let assembled = sorted
            .iter()
            .map(|s| s.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");

        let token_estimate = assembled.len() / 4;

        LayeredPrompt {
            segments: sorted,
            assembled,
            token_estimate,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_layers_have_names() {
        for layer in PromptLayer::all() {
            assert!(!layer.name().is_empty());
        }
    }

    #[test]
    fn test_assemble_orders_by_priority() {
        let segments = vec![
            PromptSegment {
                layer: PromptLayer::L5SafetyConstraints,
                content: "safety first".to_string(),
                priority: 10,
            },
            PromptSegment {
                layer: PromptLayer::L1SystemPrompt,
                content: "system instruction".to_string(),
                priority: 1,
            },
        ];
        let prompt = PromptAssembler::assemble(segments);
        assert!(prompt.assembled.starts_with("system instruction"));
        assert!(prompt.assembled.contains("safety first"));
    }

    #[test]
    fn test_token_estimate() {
        let segments = vec![PromptSegment {
            layer: PromptLayer::L1SystemPrompt,
            content: "hello world".to_string(),
            priority: 0,
        }];
        let prompt = PromptAssembler::assemble(segments);
        assert!(prompt.token_estimate > 0);
    }
}
