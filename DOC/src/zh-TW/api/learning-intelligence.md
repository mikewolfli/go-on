# 學習和智能 API

*文檔即將推出。此 API 提供機器學習、強化學習、自適應選擇和智能操作的端點。*

## 概述

學習和智能 API 為 go-on 部署提供機器學習能力、強化學習、自適應模型選擇和智能決策功能。

## 主要特性

- **機器學習**：模型訓練和推理
- **強化學習**：RL 算法和策略
- **自適應選擇**：智能模型和工具選擇
- **知識蒸餾**：知識提取和轉移
- **智能路由**：智能請求路由和負載均衡

## 端點

### 機器學習
- `GET /ml/models` - 列出 ML 模型
- `POST /ml/models` - 訓練 ML 模型
- `GET /ml/models/{id}` - 獲取 ML 模型
- `POST /ml/models/{id}/predict` - 進行預測
- `POST /ml/models/{id}/evaluate` - 評估模型

### 強化學習
- `GET /rl/policies` - 列出 RL 策略
- `POST /rl/policies` - 創建 RL 策略
- `GET /rl/policies/{id}` - 獲取 RL 策略
- `POST /rl/policies/{id}/train` - 訓練 RL 策略
- `POST /rl/policies/{id}/act` - 從策略獲取動作

### 自適應選擇
- `GET /selector/status` - 獲取選擇器狀態
- `POST /selector/select` - 選擇模型或工具
- `GET /selector/history` - 獲取選擇歷史
- `POST /selector/train` - 訓練選擇器

### 知識
- `GET /knowledge/bases` - 列出知識庫
- `POST /knowledge/bases` - 創建知識庫
- `GET /knowledge/bases/{id}` - 獲取知識庫
- `POST /knowledge/bases/{id}/query` - 查詢知識庫
- `POST /knowledge/distill` - 蒸餾知識

## 認證

所有端點都需要具有適當權限的認證。

## 速率限制

- ML 端點：每分鐘 30 個請求
- RL 端點：每分鐘 20 個請求
- 選擇端點：每分鐘 60 個請求
- 知識端點：每分鐘 40 個請求

## 下一步

本文檔正在開發中。請稍後查看完整的 API 參考。