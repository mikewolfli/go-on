# 可觀測性 API

*文檔即將推出。此 API 提供指標、追蹤、日誌和健康監控的端點。*

## 概述

可觀測性 API 為 go-on 部署提供全面的監控、追蹤、日誌記錄和健康檢查功能。

## 主要特性

- **指標收集**：系統和應用指標
- **分佈式追蹤**：端到端請求追蹤
- **結構化日誌**：集中式日誌管理
- **健康監控**：系統健康和性能監控
- **告警**：實時告警和通知

## 端點

### 指標
- `GET /metrics` - 獲取 JSON 格式的指標
- `GET /metrics/prometheus` - 獲取 Prometheus 格式的指標
- `GET /metrics/summary` - 獲取指標摘要

### 追蹤
- `GET /traces` - 列出追蹤
- `GET /traces/{id}` - 獲取追蹤詳情
- `POST /traces/search` - 搜索追蹤

### 日誌
- `GET /logs` - 查詢日誌
- `GET /logs/stream` - 實時流式傳輸日誌
- `POST /logs/export` - 導出日誌

### 健康
- `GET /health` - 整體健康狀態
- `GET /health/ready` - 就緒狀態
- `GET /health/live` - 存活狀態
- `GET /health/components` - 組件健康狀態

### 告警
- `GET /alerts` - 列出活動告警
- `POST /alerts` - 創建告警
- `GET /alerts/history` - 告警歷史

## 認證

大多數可觀測性端點是公開的，但某些敏感數據可能需要認證。

## 速率限制

- 指標端點：每分鐘 120 個請求
- 追蹤端點：每分鐘 60 個請求
- 日誌端點：每分鐘 90 個請求

## 下一步

本文檔正在開發中。請稍後查看完整的 API 參考。