# GUI Chat 切换死机修复方案

## 问题描述

用户报告：**切换到 Chat 标签页时应用死机**

## 根本原因分析

虽然之前已经为 phases 和 models 加载添加了 5 秒超时保护，但问题在于：

1. **同时触发多个后端请求**: 在首次显示 Chat 视图时，会立即启动两个异步后端请求（加载 phases 和 models）
2. **UI 响应延迟**: 即使请求是异步的，在密集的网络操作期间，GUI 主线程可能出现短暂的响应延迟，用户感知为"死机"
3. **资源争用**: 两个并发请求可能导致 tokio runtime 的资源争用，影响 UI 刷新

## 解决方案：延迟加载（Delayed Loading）

### 核心策略

不在切换标签页时立即触发后端请求，而是：
1. **延迟 100ms** 再加载 phases（让 UI 先完成渲染）
2. **延迟 150ms** 再加载 models（错开请求时间）
3. **保留 5 秒超时保护**（防止后端无响应）

### 技术实现

#### 1. 新增状态字段

```rust
/// Whether phases loading has been scheduled
phases_load_scheduled: bool,
```

#### 2. 延迟加载逻辑

```rust
// 只在第一次显示时 schedule 一次
if !self.phases_load_scheduled && !self.phases_loaded {
    self.phases_load_scheduled = true;
    
    tokio::spawn(async move {
        // 等待 100ms 让 UI 先渲染
        tokio::time::sleep(Duration::from_millis(100)).await;
        
        // 5秒超时保护
        match tokio::time::timeout(
            Duration::from_secs(5),
            backend_clone.config_baseline(),
        ).await {
            Ok(Ok(baseline)) => { /* 处理结果 */ }
            _ => { /* 超时或错误 */ }
        }
    });
}
```

#### 3. 错开请求时间

- Phases: 100ms 延迟
- Models: 150ms 延迟

避免两个请求同时启动，减少资源争用。

## 修改文件

- `gui/src/views/chat.rs`
  - 新增 `phases_load_scheduled` 字段
  - 修改 `show()` 方法：添加延迟加载逻辑
  - 修改 `new()` 方法：初始化新字段
  - 修改 `process_pending()` 方法：标记 `phases_loaded = true`

## 优化效果

| 修复前 | 修复后 |
|--------|--------|
| 切换到 Chat 立即触发 2 个后端请求 | 延迟 100-150ms 后分批触发 |
| UI 可能短暂冻结（用户感知为死机） | UI 立即响应，后台异步加载 |
| 无重试保护，用户永久看不到数据 | 保留超时保护，失败时优雅降级 |

## 验证步骤

1. 编译通过：`cd gui && cargo check` ✅
2. 运行测试：启动 GUI，多次切换到 Chat 标签页
3. 预期行为：
   - 切换瞬间 UI 立即响应
   - 100ms 后 phases 在后台加载
   - 150ms 后 models 在后台加载
   - 如果后端无响应，5 秒后超时，不会阻塞 UI

## 相关文档

- `GUI_DEADLOCK_PREVENTION_COMPREHENSIVE.md` - 27 个超时修复的详细文档
- `GUI_DEADLOCK_FIX_QUICKREF.md` - 快速参考指南

## 技术亮点

✅ **非阻塞**: 使用 `tokio::spawn` + `tokio::time::sleep` 完全异步  
✅ **超时保护**: 5 秒超时防止无限等待  
✅ **错峰加载**: 延迟并错开请求，减少资源争用  
✅ **优雅降级**: 失败时不影响 UI 继续使用  

---

**修复日期**: 2026-05-08  
**测试状态**: 编译通过 ✅
