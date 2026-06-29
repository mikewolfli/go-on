---
name: prompt-optimizer
description: Analyzes, improves, and restructures LLM prompts for clarity, efficiency, and reliability
version: 1.0.0
author: go-on-team
tags: [prompt, llm, prompt-engineering, optimization, ai]
min_go_on_version: 1.0.0
---

# Prompt Optimizer Skill

Analyzes existing prompts or constructs new ones following prompt engineering best practices. Provides structured feedback on system prompts, user prompts, and few-shot examples to improve output quality, reduce token usage, and eliminate ambiguity.

## How It Works

1. **Analyze** — Deconstructs the prompt into components: system role, instructions, context, examples, output format, constraints
2. **Evaluate** — Scores the prompt across 5 dimensions: clarity (are instructions unambiguous?), specificity (is the desired output well-defined?), efficiency (are there redundant tokens?), structure (is the information well-ordered?), robustness (does it handle edge cases?)
3. **Improve** — Rewrites the prompt following established patterns (chain-of-thought, role prompting, structured output, XML tagging, negative prompts) and platform-specific optimizations (Claude XML tags, OpenAI JSON mode, system vs user message separation)
4. **Optimize** — Reduces token count by removing redundancies, consolidating instructions, and using concise language while preserving semantic meaning

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `prompt` | string | The prompt to analyze and/or optimize |
| `action` | string | Action: `analyze`, `improve`, `optimize`, `build` (default: `analyze`) |
| `goal` | string | Optional: description of what the prompt should achieve (used with `action=build`) |
| `model` | string | Optional: target model for model-specific optimizations (`claude`, `gpt-4o`, `deepseek`, `gemini`, default: `general`) |
| `output_format` | string | Optional: desired output format (`text`, `json`, `markdown`, `code`, default: `text`) |

## Example

```json
{
  "prompt": "Write a function in Rust. It should read a file and parse some numbers. Return the result.",
  "action": "improve",
  "model": "claude",
  "output_format": "code"
}
```

**Example output:**

```markdown
## Improved Prompt

```
You are a senior Rust engineer. Your task is to write a single, well-documented
Rust function based on the specification below.

### Requirements
- Read a file from a given path (`impl AsRef<Path>`)
- Parse each line as an `f64`, skipping invalid lines
- Return `Result<Vec<f64>, io::Error>` so the caller can handle errors

### Constraints
- Use idiomatic Rust (no unsafe, prefer iterators, proper error handling)
- Include doc comments with parameter descriptions, return type, and error cases
- Add one unit test demonstrating usage with a mock file

### Output format
Only return the Rust code inside a single ```rust code block. Do not include
explanations outside the code block.
```

## Analysis (original vs improved)

| Dimension | Original | Improved |
|-----------|----------|----------|
| Clarity | 3/10 | 9/10 |
| Specificity | 2/10 | 9/10 |
| Efficiency | 6/10 | 7/10 |
| Structure | 4/10 | 10/10 |
| Robustness | 2/10 | 8/10 |

## Key Improvements
1. **Role assignment** — "senior Rust engineer" sets expertise expectations
2. **Explicit signature** — `impl AsRef<Path>` + `Result<Vec<f64>, io::Error>` eliminates ambiguity
3. **Error handling** — "skip invalid lines" covers edge cases
4. **Output constraint** — Prevents extraneous explanations
5. **Test requirement** — Ensures correctness is verifiable
```
