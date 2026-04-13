#!/bin/bash

# CI测试脚本
# 这个脚本模拟GitHub Actions CI流程中的所有步骤

set -e  # 任何命令失败则退出
echo "=== 开始CI测试 ==="

# 1. 构建项目
echo "=== 步骤1: 构建项目 ==="
cargo build --verbose
echo "✅ 构建成功"

# 2. 运行测试
echo "=== 步骤2: 运行测试 ==="
cargo test --all --verbose
echo "✅ 测试通过"

# 3. 代码检查 (允许未使用代码警告)
echo "=== 步骤3: 代码检查 ==="
cargo clippy --all -- -D warnings -A dead_code
echo "✅ 代码检查通过"

# 4. 格式化检查
echo "=== 步骤4: 格式化检查 ==="
cargo fmt --all -- --check
echo "✅ 格式化检查通过"

# 5. 运行特定模块的测试
echo "=== 步骤5: 运行i18n模块测试 ==="
cargo test i18n::tests::test_language_detection -- --nocapture
echo "✅ i18n测试通过"

# 5.1 OpenAI兼容回归测试
echo "=== 步骤5.1: 运行OpenAI兼容回归测试 ==="
cargo test openai_http_request_matrix_regression -- --nocapture
echo "✅ OpenAI兼容回归测试通过"

# 6. 验证语言文件
echo "=== 步骤6: 验证语言文件 ==="
if [ -f "languages/en_US.json" ]; then
    echo "✅ 英文语言文件存在"
    EN_KEY_COUNT=$(jq '.messages | length' languages/en_US.json 2>/dev/null || echo "0")
    echo "英文语言文件包含 $EN_KEY_COUNT 个翻译键"
else
    echo "❌ 英文语言文件不存在"
    exit 1
fi

if [ -f "languages/zh_CN.json" ]; then
    echo "✅ 中文语言文件存在"
    ZH_KEY_COUNT=$(jq '.messages | length' languages/zh_CN.json 2>/dev/null || echo "0")
    echo "中文语言文件包含 $ZH_KEY_COUNT 个翻译键"
else
    echo "❌ 中文语言文件不存在"
    exit 1
fi

# 7. 检查关键翻译键是否存在
echo "=== 步骤7: 检查关键翻译键 ==="
REQUIRED_KEYS=(
    "app.name"
    "app.description"
    "error.fatal"
    "error.handling_request"
    "info.resources_reloaded"
    "ui.legacy_health_score"
)

for key in "${REQUIRED_KEYS[@]}"; do
    if jq -e ".messages.\"$key\"" languages/en_US.json > /dev/null 2>&1; then
        echo "✅ 键 '$key' 存在于英文语言文件"
    else
        echo "❌ 键 '$key' 不存在于英文语言文件"
        exit 1
    fi
done

echo "=== CI测试完成 ==="
echo "✅ 所有CI步骤都通过了！"
echo ""
echo "总结:"
echo "1. 项目构建成功"
echo "2. 所有测试通过"
echo "3. 代码检查通过 (已忽略未使用代码警告)"
echo "4. 代码格式化正确"
echo "5. i18n系统工作正常"
echo "6. 语言文件完整"
echo "7. 关键翻译键都存在"
echo ""
echo "GitHub Actions CI流程应该能成功运行。"
