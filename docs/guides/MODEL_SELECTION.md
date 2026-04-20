# Model Selection and Automatic Mode (Phase 10+)

## Overview

The go-on project now supports dynamic model selection after provider selection, with both manual model picking and automatic mode policies for optimal model selection based on task characteristics.

## Features

### 1. Dynamic Model Listing (Option A)

After selecting an AI provider, users can:
- View all available models for that provider
- See model characteristics (capabilities, context window, etc.)
- Select a specific model or use automatic selection

**Supported Providers with Model Listing:**
- ✅ DeepSeek (v3, Chat, Coder)
- ✅ Wenxin/Baidu (Ernie 4.0 Turbo, 3.5 Turbo)
- 🔄 Other providers can implement `available_models()` method

### 2. Automatic Model Selection Policies (Option B)

**Selection Strategies:**
```rust
pub enum ModelSelectionStrategy {
    MostCapable,      // Always use best model
    Fastest,          // Prioritize speed
    Cheapest,         // Minimize cost
    Balanced,         // Balance cost and capability
    Explicit,         // User manual selection
}
```

**Automatic Policies:**
```rust
pub enum AutomaticModePolicy {
    AlwaysMostCapable,      // Always use most capable model
    AdaptiveCapability,     // Choose based on task complexity
    CostOptimized,          // Minimize cost
    SpeedOptimized,         // Maximize speed
}
```

### 3. Task-Based Selection Criteria

The system analyzes task characteristics:
```rust
pub struct SelectionCriteria {
    complexity_level: u8,              // 1-5: simple to complex
    requires_vision: bool,             // Vision capabilities needed
    requires_function_calling: bool,   // Function calling needed
    requires_code: bool,               // Code generation needed
    min_context_window: Option<usize>, // Minimum context size
    max_cost_cents: Option<u32>,       // Budget constraint
    prefer_speed: bool,                // Speed priority
}
```

## Architecture

### New Modules

1. **src/model_selector.rs**
   - `ModelSelector` - Core selection engine
   - `SelectionCriteria` - Task characteristics
   - `ModelCharacteristics` - Model metadata
   - Selection strategy implementation

2. **Updated src/agent.rs**
   - Added `ModelInfo` struct
   - Added `available_models()` to Agent trait
   - Added `default_model()` to Agent trait

3. **Agent Implementations**
   - DeepSeek: 3 models (v3, Chat, Coder)
   - Wenxin: 2 models (4.0 Turbo, 3.5 Turbo)
   - Framework for other providers

### Configuration

**model_selection_mode options in config.toml:**
```toml
[config]
model_selection_mode = "adaptive"  # or "explicit", "cost", "speed", "capable"
```

## Usage Examples

### Example 1: Provider → Model Selection Flow

**Step 1:** User selects provider (DeepSeek)
```
Provider: DeepSeek ✓
```

**Step 2:** UI fetches available models via `available_models()`
```
Available Models:
- DeepSeek v3 (Most capable, 64K context)
- DeepSeek Chat (Fastest, default)
- DeepSeek Coder (Code specialist)
```

**Step 3:** User selects model or auto-mode
```
Selected: DeepSeek v3 ✓
```

### Example 2: Automatic Selection Based on Task

**Complex task (complexity=5):**
```
Criteria: {complexity_level: 5, requires_function_calling: true}
Policy: AdaptiveCapability
Selected Model: deepseek-v3 (most capable)
```

**Simple task (complexity=1):**
```
Criteria: {complexity_level: 1, prefer_speed: true}
Policy: AdaptiveCapability
Selected Model: deepseek-chat (fastest)
```

**Cost-conscious task:**
```
Criteria: {max_cost_cents: Some(10)}
Policy: CostOptimized
Selected Model: cheapest-available
```

## Implementation Details

### Adding Model Listing to New Providers

```rust
impl Agent for MyProviderAgent {
    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "model-v1".to_string(),
                name: "Model v1".to_string(),
                description: "Description".to_string(),
                is_default: true,
                capabilities: vec!["chat".to_string()],
                context_window: Some(4096),
            },
        ]
    }

    fn default_model(&self) -> Option<ModelInfo> {
        self.available_models().into_iter().find(|m| m.is_default)
    }
}
```

### Using Model Selector

```rust
use crate::model_selector::{ModelSelector, SelectionCriteria, ModelSelectionStrategy};

let criteria = SelectionCriteria::complex();
let models = vec![/* available models */];
let strategy = ModelSelectionStrategy::MostCapable;

let selected_model = ModelSelector::select_model(&criteria, &models, strategy);
```

## Mode Integration

### Ask Mode
- User enters query
- Selects provider
- **NEW:** Selects or auto-selects model
- Gets response

### Edit Mode
- User selects code to edit
- Selects provider
- **NEW:** Selects or auto-selects model
- Gets inline edit suggestions

### Agent Mode
- Complex multi-step task
- System automatically selects provider based on phase
- **NEW:** System automatically selects optimal model based on task characteristics

### Full Auto Mode
- Complete autonomous operation
- Phase determines provider
- **NEW:** Task complexity determines optimal model

## VS Code Extension Updates

**settingsView.ts (model selection):**
```html
<label>Provider:</label>
<select id="provider" onchange="loadModels()">
  <option>DeepSeek</option>
  <option>OpenAI</option>
  <option>Wenxin</option>
</select>

<label>Model:</label>
<select id="model">
  <!-- Populated dynamically after provider selection -->
</select>

<label>Selection Mode:</label>
<select id="selectionMode">
  <option value="explicit">Manual Selection</option>
  <option value="adaptive">Adaptive (based on task)</option>
  <option value="cost">Cost Optimized</option>
  <option value="speed">Speed Optimized</option>
</select>
```

## Next Steps

1. ✅ Add model listing to all providers (DeepSeek, Wenxin done)
2. ✅ Implement automatic selection strategies
3. ⏳ Wire model selector into ACP request handler
4. ⏳ Update VS Code extension UI
5. ⏳ Add model switching in chat interface

## Testing

Model selector includes comprehensive tests:
```bash
cargo test model_selector --lib
```

Test coverage:
- Most capable selection
- Cheapest selection
- Fastest selection
- Balanced selection
- Capability filtering
- Cost constraints
- Context window requirements

## Configuration Examples

**Always use most capable:**
```toml
model_selection_mode = "capable"
```

**Adaptive based on task complexity:**
```toml
model_selection_mode = "adaptive"
```

**Cost-conscious (startup):**
```toml
model_selection_mode = "cost"
```

**Speed-critical (real-time):**
```toml
model_selection_mode = "speed"
```

**User chooses each time:**
```toml
model_selection_mode = "explicit"
```

