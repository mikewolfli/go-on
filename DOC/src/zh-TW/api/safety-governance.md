# 安全和治理 API

*文檔即將推出。此 API 提供安全策略、審計日誌、合規性監控和治理操作的端點。*

## 概述

安全和治理 API 為 go-on 部署提供安全管理、策略執行、審計跟蹤維護和合規性監控功能。

## 主要特性

- **安全策略**：定義和執行安全規則
- **審計日誌**：所有操作的全面審計跟蹤
- **合規性監控**：跟蹤法規和標準的合規性
- **訪問控制**：基於角色的訪問控制（RBAC）
- **事件響應**：安全事件管理

## 端點

### 安全策略
- `GET /security/policies` - 列出安全策略
- `POST /security/policies` - 創建安全策略
- `GET /security/policies/{id}` - 獲取安全策略
- `PUT /security/policies/{id}` - 更新安全策略
- `DELETE /security/policies/{id}` - 刪除安全策略

### 審計日誌
- `GET /audit/logs` - 查詢審計日誌
- `GET /audit/logs/{id}` - 獲取審計日誌條目
- `POST /audit/logs/export` - 導出審計日誌

### 合規性
- `GET /compliance/status` - 獲取合規性狀態
- `POST /compliance/checks` - 運行合規性檢查
- `GET /compliance/reports` - 生成合規性報告

### 訪問控制
- `GET /access/roles` - 列出角色
- `POST /access/roles` - 創建角色
- `GET /access/permissions` - 列出權限
- `POST /access/assignments` - 為用戶分配角色

## 認證

所有端點都需要具有適當權限的認證。

## 速率限制

- 安全端點：每分鐘 30 個請求
- 審計端點：每分鐘 60 個請求
- 合規性端點：每分鐘 20 個請求

## 下一步

本文檔正在開發中。請稍後查看完整的 API 參考。