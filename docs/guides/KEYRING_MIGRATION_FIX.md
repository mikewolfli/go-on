# DeepSeek 配置升级后"未就绪/connecting"问题修复

## 问题描述

升级 Go-On GUI 后，之前通过 CLI `--secret set` 或初次启动向导配置好的 DeepSeek API key，在 GUI 中显示"未就绪"，一直处于"connecting"状态。

## 根本原因

1. **CLI 配置方式**：使用 `go-on setup --secret keyring` 会将 API key 存入系统 keyring（Linux 用 Secret Service/libsecret，macOS 用 Keychain，Windows 用 Credential Manager），并在 `config.toml` 中写入 `keyring://go-on/deepseek_api_key` 引用。

2. **GUI 配置方式**：GUI 也会将 API key 存入 keyring，但同时在本地 `gui_config.json` 中保存一份明文副本（用于跨平台兼容），并设置 `validated: true` 标记。

3. **升级后的问题**：
   - GUI 判断"是否已配置"只看本地 JSON 的 `validated` 字段和 `api_key` 非空
   - 升级后没有迁移逻辑，导致 keyring 中已有的配置被认为"未配置"
   - Linux 环境下启动后端时，只从 JSON 读取明文 key 作为环境变量，无法读取 keyring 中的配置

## 修复方案

### 1. GUI 启动时自动迁移 keyring 配置

修改 `gui/src/config.rs` 中的 `load_app_config()` 函数：

```rust
/// Load GUI app config from JSON file and auto-migrate keyring providers
pub fn load_app_config() -> AppConfig {
    let path = app_config_path();
    let mut config = if let Ok(content) = std::fs::read_to_string(&path) {
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        AppConfig::default()
    };

    // Auto-migrate: detect providers in keyring but not in config
    let mut changed = false;
    for provider_name in [
        "deepseek", "openai", "anthropic", "qwen", "gemini", "groq", "mistral",
    ] {
        if config.providers.iter().any(|p| p.name == provider_name) {
            continue;
        }
        if let Some(key) = crate::keyring_util::get_api_key(provider_name) {
            eprintln!(
                "Auto-migrating '{}' from keyring to gui_config.json",
                provider_name
            );
            config.providers.push(ProviderConfig {
                name: provider_name.to_string(),
                api_key: key,
                model: "auto".to_string(),
                validated: true,
            });
            changed = true;
        }
    }

    if changed {
        save_app_config(&config);
    }

    config
}
```

**效果**：
- GUI 启动时自动检测 keyring 中的 provider 配置
- 如果 keyring 中有但 JSON 中没有，自动同步到 JSON 并标记为 `validated: true`
- 用户无需重新输入 API key

### 2. 后端启动时优先读取 keyring（已支持）

`gui/src/app.rs` 中启动后端的逻辑已经支持同时从 JSON 和 keyring 读取：

```rust
// 1. From gui_config.json providers
for p in &config.providers {
    if !p.api_key.is_empty() {
        set_env(&p.name.to_lowercase(), &p.api_key);
    }
}
// 2. From keyring (covers CLI --secret set cases)
let known = [
    "deepseek", "openai", "anthropic", "qwen", "gemini", "groq", "mistral", "copilot",
];
for name in &known {
    if let Some(key) = crate::keyring_util::get_api_key(name) {
        set_env(name, &key);
    }
}
```

**效果**：
- 优先使用 keyring 中的最新配置
- 兼容 CLI `--secret set` 配置方式
- JSON 配置作为 fallback

## 测试步骤

1. **准备环境**（模拟升级场景）：
```bash
# 1. 确保 keyring 中有 deepseek_api_key（Linux）
secret-tool store --label='Go-On DeepSeek' service go-on account deepseek_api_key
# 输入你的 API key

# 2. 清空 GUI 配置（模拟升级后首次启动）
rm -f ~/.config/go-on-gui/gui_config.json
```

2. **运行 GUI**：
```bash
cargo run --release -p go-on-gui-egui
```

3. **验证结果**：
   - GUI 启动后不会显示 Setup 向导（因为检测到 keyring 中有配置）
   - 打开 Settings > Providers 标签，应该看到 DeepSeek 已配置且 `validated: true`
   - Monitor 标签中，DeepSeek 状态应该显示为"ready"，而不是"未就绪"
   - Backend 应该成功启动，不会一直"connecting"

4. **使用测试脚本**：
```bash
./test_keyring_migration.sh
```

## 技术细节

### Keyring 存储格式

- **Service**: `go-on`
- **Account**: `{provider}_api_key`（例如 `deepseek_api_key`）
- **跨平台支持**：
  - Linux: Secret Service (libsecret) via D-Bus
  - macOS: Keychain
  - Windows: Credential Manager

### 配置文件路径

- **Backend config**: `~/.config/go-on/config.toml` 或项目根目录
- **GUI config**: `~/.config/go-on-gui/gui_config.json`（Linux）
  - macOS: `~/Library/Application Support/com.goon.go-on-gui/gui_config.json`
  - Windows: `%APPDATA%\goon\go-on-gui\config\gui_config.json`

### 优先级规则

启动后端时的环境变量设置优先级：
1. Keyring 中的配置（最高优先级）
2. `gui_config.json` 中的明文配置
3. 系统环境变量（如果后端直接通过 CLI 启动）

## 受影响的版本

- **修复前**：所有版本升级后首次启动 GUI
- **修复后**：`v0.9.5+`（当前版本）

## 相关文件

- `gui/src/config.rs` - 配置加载和迁移逻辑
- `gui/src/app.rs` - 后端启动和环境变量设置
- `gui/src/keyring_util.rs` - 系统 keyring 读写工具
- `src/core/config.rs` - Backend 配置读取（支持 `keyring://` 引用）
- `src/agents/agent.rs` - Agent 初始化时的 secret 解析

## 建议

1. **首次配置**：建议使用 GUI 或 CLI `--secret keyring` 模式，避免明文环境变量
2. **跨平台同步**：如果需要在多台机器间同步配置，手动导出 API key 并在新机器上重新配置
3. **安全性**：GUI 在 `gui_config.json` 中保存明文副本是为了跨平台兼容，如果担心安全问题，可以手动删除 JSON 中的 `api_key` 字段（仅保留 keyring 配置）
