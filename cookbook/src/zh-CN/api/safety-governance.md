# 安全和治理 API

*文档即将推出。此 API 提供安全策略、审计日志、合规性监控和治理操作的端点。*

## 概述

安全和治理 API 为 go-on 部署提供安全管理、策略执行、审计跟踪维护和合规性监控功能。

## 主要特性

- **安全策略**：定义和执行安全规则
- **审计日志**：所有操作的全面审计跟踪
- **合规性监控**：跟踪法规和标准的合规性
- **访问控制**：基于角色的访问控制（RBAC）
- **事件响应**：安全事件管理

## 端点

### 安全策略
- `GET /security/policies` - 列出安全策略
- `POST /security/policies` - 创建安全策略
- `GET /security/policies/{id}` - 获取安全策略
- `PUT /security/policies/{id}` - 更新安全策略
- `DELETE /security/policies/{id}` - 删除安全策略

### 审计日志
- `GET /audit/logs` - 查询审计日志
- `GET /audit/logs/{id}` - 获取审计日志条目
- `POST /audit/logs/export` - 导出审计日志

### 合规性
- `GET /compliance/status` - 获取合规性状态
- `POST /compliance/checks` - 运行合规性检查
- `GET /compliance/reports` - 生成合规性报告

### 访问控制
- `GET /access/roles` - 列出角色
- `POST /access/roles` - 创建角色
- `GET /access/permissions` - 列出权限
- `POST /access/assignments` - 为用户分配角色

## 认证

所有端点都需要具有适当权限的认证。

## 速率限制

- 安全端点：每分钟 30 个请求
- 审计端点：每分钟 60 个请求
- 合规性端点：每分钟 20 个请求

## 下一步

本文档正在开发中。请稍后查看完整的 API 参考。