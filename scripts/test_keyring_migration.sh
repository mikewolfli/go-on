#!/bin/bash
# Test script to verify keyring migration fix

set -e

echo "=== Testing Keyring Migration Fix ==="
echo ""

# 1. Check if keyring has deepseek_api_key
echo "1. Checking keyring for deepseek_api_key..."
if command -v secret-tool &> /dev/null; then
    # Linux Secret Service
    if secret-tool lookup service go-on account deepseek_api_key &> /dev/null; then
        echo "   ✓ Found deepseek_api_key in keyring (Linux Secret Service)"
    else
        echo "   ✗ deepseek_api_key NOT found in keyring"
        echo "   Run: secret-tool store --label='Go-On DeepSeek' service go-on account deepseek_api_key"
        exit 1
    fi
else
    echo "   ! secret-tool not found, assuming keyring is configured"
fi
echo ""

# 2. Clean gui_config.json to simulate fresh start
GUI_CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/go-on-gui/gui_config.json"
if [ -f "$GUI_CONFIG" ]; then
    echo "2. Backing up existing gui_config.json..."
    cp "$GUI_CONFIG" "${GUI_CONFIG}.backup"
    echo "   ✓ Backup saved to ${GUI_CONFIG}.backup"
fi
echo ""

# 3. Test: remove deepseek from config to simulate upgrade scenario
echo "3. Testing auto-migration scenario..."
if [ -f "$GUI_CONFIG" ]; then
    # Create a minimal config without deepseek
    cat > "$GUI_CONFIG" <<'EOF'
{
  "backend_url": "http://127.0.0.1:8090",
  "language": "zh-CN",
  "theme": "简约",
  "features": {
    "monitor": true,
    "chat": true,
    "skills": true,
    "workflow": true,
    "autotune": true,
    "security": true,
    "config": true,
    "providers": true,
    "workflow_run_center": false,
    "autotune_chain_injection": false,
    "skills_lifecycle": false,
    "providers_ops": false,
    "monitor_history_alerts": false,
    "config_safe_mode": false,
    "setup_enterprise": false
  },
  "enterprise": {
    "active_environment": "dev",
    "environments": [
      {"name": "dev", "backend_url": "http://127.0.0.1:8090"}
    ],
    "secret_source": "keyring",
    "import_path": "gui_config.import.json",
    "export_path": "gui_config.export.json"
  },
  "providers": []
}
EOF
    echo "   ✓ Created test config without deepseek provider"
fi
echo ""

echo "4. Next steps:"
echo "   a) Launch GUI: cargo run --release -p go-on-gui-egui"
echo "   b) GUI should auto-detect deepseek from keyring"
echo "   c) Check that deepseek appears in Settings > Providers tab"
echo "   d) Backend should start successfully with deepseek configured"
echo ""

echo "=== Fix Verification ==="
echo "The fix adds auto-migration logic in gui/src/config.rs:"
echo "  - On startup, GUI checks keyring for known providers"
echo "  - If found but not in gui_config.json, auto-adds them"
echo "  - Marks them as validated: true"
echo "  - Saves updated config"
echo ""
echo "This solves the 'skip导致未就绪' issue after upgrades."
