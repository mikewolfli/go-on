# 可观测性 API

*文档即将推出。此 API 提供指标、追踪、日志和健康监控的端点。*

## 概述

可观测性 API 为 go-on 部署提供全面的监控、追踪、日志记录和健康检查功能。

## 主要特性

- **指标收集**：系统和应用指标
- **分布式追踪**：端到端请求追踪
- **结构化日志**：集中式日志管理
- **健康监控**：系统健康和性能监控
- **告警**：实时告警和通知

## 端点

### 指标
- `GET /metrics` - 获取 JSON 格式的指标
- `GET /metrics/prometheus` - 获取 Prometheus 格式的指标
- `GET /metrics/summary` - 获取指标摘要

### 追踪
- `GET /traces` - 列出追踪
- `GET /traces/{id}` - 获取追踪详情
- `POST /traces/search` - 搜索追踪

### 日志
- `GET /logs` - 查询日志
- `GET /logs/stream` - 实时流式传输日志
- `POST /logs/export` - 导出日志

### 健康
- `GET /health` - 整体健康状态
- `GET /health/ready` - 就绪状态
- `GET /health/live` - 存活状态
- `GET /health/components` - 组件健康状态

### 告警
- `GET /alerts` - 列出活动告警
- `POST /alerts` - 创建告警
- `GET /alerts/history` - 告警历史

## 认证

大多数可观测性端点是公开的，但某些敏感数据可能需要认证。

## 速率限制

- 指标端点：每分钟 120 个请求
- 追踪端点：每分钟 60 个请求
- 日志端点：每分钟 90 个请求

## 下一步

本文档正在开发中。请稍后查看完整的 API 参考。