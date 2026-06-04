# 学习和智能 API

*文档即将推出。此 API 提供机器学习、强化学习、自适应选择和智能操作的端点。*

## 概述

学习和智能 API 为 go-on 部署提供机器学习能力、强化学习、自适应模型选择和智能决策功能。

## 主要特性

- **机器学习**：模型训练和推理
- **强化学习**：RL 算法和策略
- **自适应选择**：智能模型和工具选择
- **知识蒸馏**：知识提取和转移
- **智能路由**：智能请求路由和负载均衡

## 端点

### 机器学习
- `GET /ml/models` - 列出 ML 模型
- `POST /ml/models` - 训练 ML 模型
- `GET /ml/models/{id}` - 获取 ML 模型
- `POST /ml/models/{id}/predict` - 进行预测
- `POST /ml/models/{id}/evaluate` - 评估模型

### 强化学习
- `GET /rl/policies` - 列出 RL 策略
- `POST /rl/policies` - 创建 RL 策略
- `GET /rl/policies/{id}` - 获取 RL 策略
- `POST /rl/policies/{id}/train` - 训练 RL 策略
- `POST /rl/policies/{id}/act` - 从策略获取动作

### 自适应选择
- `GET /selector/status` - 获取选择器状态
- `POST /selector/select` - 选择模型或工具
- `GET /selector/history` - 获取选择历史
- `POST /selector/train` - 训练选择器

### 知识
- `GET /knowledge/bases` - 列出知识库
- `POST /knowledge/bases` - 创建知识库
- `GET /knowledge/bases/{id}` - 获取知识库
- `POST /knowledge/bases/{id}/query` - 查询知识库
- `POST /knowledge/distill` - 蒸馏知识

## 认证

所有端点都需要具有适当权限的认证。

## 速率限制

- ML 端点：每分钟 30 个请求
- RL 端点：每分钟 20 个请求
- 选择端点：每分钟 60 个请求
- 知识端点：每分钟 40 个请求

## 下一步

本文档正在开发中。请稍后查看完整的 API 参考。