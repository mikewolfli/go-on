//! BLUE35 S6: Prompt Layered Architecture (ARCH-03)
//!
//! Defines a layered prompt processing pipeline. Only the layers that are
//! actually constructed in production remain:
//! L1: System Prompt Assembly
//! L2: Role & Identity Injection
//! (L3–L8 — context-window optimization, task decomposition guidance,
//! safety constraints, output format, chain-of-thought triggers, and
//! meta-cognitive instructions — were never constructed anywhere; they were
//! removed as dead variants.)

use serde::{Deserialize, Serialize};

/// The prompt layers actually used by the request path.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PromptLayer {
    L1SystemPrompt,
    L2RoleIdentity,
}

impl PromptLayer {
    #[cfg(test)]
    pub fn all() -> Vec<PromptLayer> {
        vec![PromptLayer::L1SystemPrompt, PromptLayer::L2RoleIdentity]
    }

    #[cfg(test)]
    pub fn name(&self) -> &'static str {
        match self {
            PromptLayer::L1SystemPrompt => "system_prompt",
            PromptLayer::L2RoleIdentity => "role_identity",
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

        // Delegate to the shared CJK-aware estimator so prompt-layer token
        // accounting agrees with the rest of the binary (CLI, compression,
        // cache sizing) instead of a naive chars/4 heuristic.
        let token_estimate = crate::shared::token_estimator::estimate_tokens(&assembled);

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
                layer: PromptLayer::L2RoleIdentity,
                content: "role identity".to_string(),
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
        assert!(prompt.assembled.contains("role identity"));
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
