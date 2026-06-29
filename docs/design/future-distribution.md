# Go-On Worker Swarm 分布式架构方案

> 文档状态: **蓝图中未实现的功能方案**
> 关联: FUTURE5 §M2 (Worker Swarm), FUTURE5 §M4 (Consensus Engine), `federated_transport.rs` (gRPC 桩代码)
> 当前架构: 单机多进程（GUI ↔ Backend ↔ Tool 子进程），Worker Swarm 需要**真正的多机分布式基础设施**

---

## 目录

1. [为什么当前架构不支持 Worker Swarm](#1-为什么当前架构不支持-worker-swarm)
2. [分布式 Worker Swarm 全功能清单](#2-分布式-worker-swarm-全功能清单)
3. [已存在的代码（桩/蓝图）](#3-已存在的代码桩蓝图)
4. [需要新建的基础设施](#4-需要新建的基础设施)
5. [架构设计](#5-架构设计)
6. [跨节点任务执行流程](#6-跨节点任务执行流程)
7. [gRPC 协议定义](#7-grpc-协议定义)
8. [实现优先级与路线图](#8-实现优先级与路线图)
9. [风险与应对](#9-风险与应对)

---

## 1. 为什么当前架构不支持 Worker Swarm

### 当前架构

```
┌──────────┐  spawn     ┌──────────────┐  spawn    ┌────────────┐
│  GUI      │ ────────► │  Backend x1  │ ────────► │ Tool 子进程 │
│  (单实例)  │ ◄──────── │  (单节点)     │           │ (cargo等)   │
└──────────┘  HTTP/SSE  └──────────────┘           └────────────┘
```

- **一个进程**：所有 Agent 在同一个进程中通过 `tokio::spawn` 并发运行
- **一个实例**：没有水平扩展能力
- **本地状态**：`AgentRegistry`、`Council`、`Memory` 全部在进程内存中
- **无网络通信**：Agent 之间通过函数调用直接交互，没有跨节点 RPC

### Worker Swarm 要求

```
┌──────────┐          ┌──────────────┐         ┌──────────────┐
│  Gateway  │ ────►  │  Coordinator │ ◄─────► │  Worker A    │
│  (LB)     │         │  (Leader)    │         │  (Node 2)    │
└──────────┘          ├──────────────┤         ├──────────────┤
                      │  Worker Swarm│────────►│  Worker B    │
                      │  (调度器)     │  gRPC   │  (Node 3)    │
                      └──────────────┘         └──────────────┘
                                                    │
                                                    ▼
                                            ┌──────────────┐
                                            │  Worker C    │
                                            │  (Node N)    │
                                            └──────────────┘
```

| 维度 | 当前架构 | Worker Swarm 要求 |
|------|---------|-------------------|
| **节点数** | 1 个 backend 进程 | N 个物理节点（N ≥ 2） |
| **Agent 通信** | 内存函数调用 | gRPC/消息队列跨节点 |
| **状态** | 进程内存 | 分布式一致存储 |
| **任务调度** | 本地 JoinSet | 分布式调度器 |
| **容错** | 进程崩溃=全挂 | 节点故障可迁移 |
| **发现** | 无（单一进程） | 服务注册与发现 |
| **负载均衡** | 无 | 多节点流量分发 |

---

## 2. 分布式 Worker Swarm 全功能清单

### 2.1 节点生命周期管理

| # | 功能 | 说明 | 优先级 |
|---|------|------|--------|
| N1 | **节点注册** | 新节点启动时向协调者注册（ID、地址、能力标签） | P0 |
| N2 | **心跳检测** | 每 5-15 秒发送心跳，超时判定离线 | P0 |
| N3 | **优雅退出** | 节点关闭前完成当前任务、通知协调者、迁移状态 | P0 |
| N4 | **节点上线/离线事件** | 协调者广播节点状态变化给所有 Worker | P0 |
| N5 | **能力广播** | 节点定期广播自己的能力（GPU型号、可用模型、剩余资源） | P1 |
| N6 | **自动扩缩容** | 基于负载指标自动启停 Worker 节点 | P2 |

### 2.2 分布式任务调度

| # | 功能 | 说明 | 优先级 |
|---|------|------|--------|
| T1 | **任务队列** | 持久化分布式任务队列（如 NATS/RabbitMQ） | P0 |
| T2 | **任务分派** | 协调者按能力/负载/亲和性将任务派给最合适的 Worker | P0 |
| T3 | **任务迁移** | Worker 故障时，未完成任务迁移到其他 Worker | P0 |
| T4 | **任务幂等性** | 每个任务有唯一 ID，支持去重执行 | P0 |
| T5 | **优先级队列** | 高优先级任务插队，防止低优任务阻塞关键路径 | P1 |
| T6 | **任务分片** | 大任务拆分为子任务，分发到多个 Worker 并行执行 | P1 |
| T7 | **本地优先调度** | 数据所在节点优先执行，减少跨节点数据传输 | P2 |

### 2.3 分布式 Agent 执行

| # | 功能 | 说明 | 优先级 |
|---|------|------|--------|
| A1 | **Agent 注册 (跨节点)** | Agent 在多节点上注册，协调者知道每个 Agent 运行在哪个 Worker | P0 |
| A2 | **Agent 路由** | 用户请求路由到有目标 Agent 的 Worker | P0 |
| A3 | **Agent 间通信** | 跨节点的 Agent 通过 RPC 交换消息 | P0 |
| A4 | **Agent 状态同步** | Agent 运行时状态的跨节点一致性同步 | P1 |
| A5 | **Agent 热迁移** | 不中断服务地将 Agent 从一个节点迁移到另一个 | P2 |
| A6 | **Agent 版本管理** | 多版本 Agent 共存，A/B 测试 | P2 |

### 2.4 分布式状态与存储

| # | 功能 | 说明 | 优先级 |
|---|------|------|--------|
| S1 | **分布式 Memory** | 跨节点的共享记忆存储（已有 pgvector 蓝图） | P0 |
| S2 | **分布式 Session** | 用户会话跨节点可用 | P0 |
| S3 | **分布式 Token Cache** | 多级缓存：本地 L1 + 分布式 L2 | P1 |
| S4 | **分布式 Vector Store** | 向量数据库集群（已有 multi-users-server 的 pgvector） | P1 |
| S5 | **分布式 Artifact Ledger** | 任务产物跨节点可追溯 | P1 |
| S6 | **一致性感知识别** | etcd/Raft 集群维护全局配置和共识状态 | P1 |

### 2.5 分布式共识与治理

| # | 功能 | 说明 | 优先级 |
|---|------|------|--------|
| C1 | **Leader 选举** | 多协调者节点中选举 Leader（已有 Raft 蓝图） | P0 |
| C2 | **分布式 Council** | Council 成员在多个节点上，通过 RPC 投票 | P0 |
| C3 | **共识决策** | 跨节点提案通过 Raft/Paxos 达成一致 | P1 |
| C4 | **分布式审计** | 全链路审计事件跨节点汇聚 | P1 |
| C5 | **跨节点限流** | 全局速率限制和配额（已有 multi-users-server 限流桩） | P1 |
| C6 | **全局预算控制** | 多节点统一的 Token/API 预算管理 | P2 |

### 2.6 分布式联邦学习

| # | 功能 | 说明 | 优先级 |
|---|------|------|--------|
| F1 | **权重传输** | 节点间传输模型权重（已有 gRPC 桩代码） | P0 |
| F2 | **安全聚合** | 差分隐私 + 安全多方计算 | P1 |
| F3 | **全局模型同步** | 周期聚合各节点局部模型为全局模型 | P1 |
| F4 | **节点选择性加入/退出** | 节点可随时加入/退出联邦学习 | P2 |

### 2.7 可观测性

| # | 功能 | 说明 | 优先级 |
|---|------|------|--------|
| O1 | **分布式追踪** | OpenTelemetry 跨节点 Trace 串联 | P0 |
| O2 | **节点健康仪表盘** | 实时显示所有节点健康状态、负载、任务数 | P0 |
| O3 | **跨节点日志聚合** | 所有节点的日志统一收集和查询 | P1 |
| O4 | **分布式告警** | 节点故障自动告警和通知 | P1 |
| O5 | **性能基准跨节点对比** | 不同节点上的性能指标对比分析 | P2 |

---

## 3. 已存在的代码（桩/蓝图）

以下代码已经存在，但状态不同：

### 🟢 可直接使用的代码

| 组件 | 文件 | 状态 |
|------|------|------|
| `FederatedTransport` trait | `src/intelligence/reinforcement/federated_transport.rs` | ✅ 完整 trait 定义 |
| `NodeDiscovery` trait | `src/intelligence/reinforcement/federated_discovery.rs` | ✅ 完整 trait 定义 |
| `StaticDiscovery` | `src/intelligence/reinforcement/federated_discovery.rs` | ✅ 生产可用 |
| `Heartbeat` | `src/intelligence/reinforcement/federated_discovery.rs` | ✅ 生产可用 |
| `InProcessTransport` | `src/intelligence/reinforcement/federated_transport.rs` | ✅ 生产可用（测试用） |
| `PeerInfo` / `NodeRole` | `src/intelligence/reinforcement/federated_transport.rs` | ✅ 完整数据结构 |
| `NodeInfo` | `src/intelligence/reinforcement/federated_discovery.rs` | ✅ 完整数据结构 |

### 🟡 需要补充的桩代码

| 组件 | 文件 | 状态 | 缺失部分 |
|------|------|------|---------|
| `GrpcFederatedTransport` | `federated_transport.rs` | ⚠️ 桩代码 | 所有方法返回 `Ok(true)`，无真正 gRPC 调用 |
| `proto/federated.proto` | 不存在 | ❌ 需要创建 | 需要在 `proto/` 目录下创建完整的 protobuf 定义 |
| `build.rs` | 不存在 | ❌ 需要创建 | 需要 `tonic-build` 编译 proto |
| `FederatedService` tonic server | 不存在 | ❌ 需要创建 | 无 gRPC 服务器实现 |
| 分布式 Memory | `multi-users-server` 的 pgvector | 🟡 蓝图 | 只有 PostgreSQL 连接，无分布式数据分片/复制 |

### ❌ 完全不存在的基础设施

| 组件 | 说明 |
|------|------|
| etcd / Consul 集成 | 无服务注册中心实现 |
| 分布式任务队列 | 无 NATS / RabbitMQ 集成 |
| Raft 共识 | 无 Leader 选举和共识算法 |
| 分布式 Session 管理 | 无跨节点会话共享 |
| 跨节点 Trace | 无 OTel Trace 跨节点串联 |
| 自动扩缩容 | 无基于负载的节点管理 |

---

## 4. 需要新建的基础设施

### 4.1 模块结构

```text
src/distributed/                          # NEW — 分布式基础设施根目录
├── mod.rs                               # 模块声明 + 全局配置
├── config.rs                            # DistributedConfig 统一配置
├── node.rs                              # NodeManager — 节点生命周期管理
├── registry/                            # 服务注册与发现
│   ├── mod.rs
│   ├── etcd.rs                          # etcd 实现
│   ├── consul.rs                        # Consul 实现
│   └── memory.rs                        # 内存实现（单机测试用）
├── transport/                           # 节点间通信
│   ├── mod.rs
│   ├── grpc.rs                          # 真正 gRPC 客户端/服务端
│   ├── proto/                           # protobuf 定义
│   │   └── swarm.proto                  # Worker Swarm 的完整 proto
│   └── build.rs                         # tonic-build 编译脚本
├── scheduler/                           # 分布式调度器
│   ├── mod.rs
│   ├── queue.rs                         # 任务队列抽象
│   ├── nats.rs                          # NATS 实现
│   ├── rabbitmq.rs                      # RabbitMQ 实现
│   └── assignment.rs                    # 任务分配策略（能力/负载/亲和性）
├── consensus/                           # 分布式共识
│   ├── mod.rs
│   ├── raft.rs                          # Raft 共识算法
│   ├── leader.rs                        # Leader 选举
│   └── state_machine.rs                 # 一致性状态机
├── session/                             # 分布式 Session
│   ├── mod.rs
│   ├── manager.rs                       # Session 管理器
│   └── migration.rs                     # Session 迁移
├── fault/                               # 容错机制
│   ├── mod.rs
│   ├── detector.rs                      # 故障检测
│   ├── recovery.rs                      # 恢复逻辑
│   └── partition.rs                     # 网络分区处理
└── observability/                       # 分布式可观测性
    ├── mod.rs
    ├── tracing.rs                       # 跨节点 Trace 串联
    ├── logging.rs                       # 日志聚合
    └── dashboard.rs                     # 集群仪表盘
```

### 4.2 核心数据结构

```rust
/// 分布式 Swarm 配置
pub struct DistributedConfig {
    /// 当前节点 ID（UUID）
    pub node_id: String,
    /// 节点角色
    pub role: NodeRole,
    /// 监听地址（gRPC 端口）
    pub listen_addr: String,
    /// 注册中心地址（etcd/consul）
    pub registry_addrs: Vec<String>,
    /// 任务队列地址（NATS/RabbitMQ）
    pub queue_addrs: Vec<String>,
    /// 心跳间隔秒数
    pub heartbeat_interval_secs: u64,
    /// 心跳超时秒数
    pub heartbeat_timeout_secs: u64,
    /// Leader 选举超时毫秒数
    pub election_timeout_ms: u64,
    /// 最大任务重试次数
    pub max_task_retries: u32,
    /// 是否为协调者节点
    pub is_coordinator: bool,
}

/// 分布式任务（扩展自现有 TaskCharacteristics）
pub struct DistributedTask {
    /// 全局唯一任务 ID
    pub task_id: String,
    /// 任务类型
    pub task_type: TaskType,
    /// 任务描述
    pub description: String,
    /// 所需能力标签
    pub required_capabilities: Vec<String>,
    /// 数据本地化提示（优先调度到数据所在节点）
    pub data_locality_hint: Option<String>,
    /// 优先级（0-100，越高越优先）
    pub priority: u8,
    /// 超时秒数
    pub timeout_secs: u64,
    /// 最大重试次数
    pub max_retries: u32,
    /// 幂等性 token（用于去重）
    pub idempotency_token: String,
    /// 父任务 ID（子任务链）
    pub parent_task_id: Option<String>,
}

/// 分布式节点状态
pub struct NodeState {
    /// 节点 ID
    pub node_id: String,
    /// 地址
    pub addr: String,
    /// 角色
    pub role: NodeRole,
    /// 能力标签
    pub capabilities: Vec<String>,
    /// 是否在线
    pub online: bool,
    /// 最后心跳时间戳
    pub last_heartbeat_ms: u64,
    /// 当前负载（0.0 - 1.0）
    pub load: f64,
    /// 运行中任务数
    pub active_tasks: u32,
    /// 内存使用率
    pub memory_usage: f64,
    /// GPU 信息（如果有）
    pub gpu_info: Option<GpuInfo>,
    /// 可用模型列表
    pub available_models: Vec<ModelInfo>,
}

/// 分布式调度决策
pub struct SchedulingDecision {
    /// 任务 ID
    pub task_id: String,
    /// 选中的 Worker 节点 ID
    pub assigned_worker: String,
    /// 调度原因
    pub reason: SchedulingReason,
    /// 预估执行耗时
    pub estimated_duration_ms: u64,
    /// 调度时间戳
    pub scheduled_at_ms: u64,
}

pub enum SchedulingReason {
    /// 能力匹配最优
    CapabilityMatch(f64),
    /// 负载最低
    LowestLoad(f64),
    /// 数据本地化
    DataLocality,
    /// 亲和性调度（同一用户/会话的请求集中到同一节点）
    Affinity,
    /// 随机（兜底）
    Random,
}

/// 分布式共识提案
pub struct ConsensusProposal {
    /// 提案 ID
    pub proposal_id: String,
    /// 提案类型
    pub proposal_type: ProposalType,
    /// 提案内容（JSON）
    pub payload: Value,
    /// 发起者节点
    pub proposer: String,
    /// 创建时间戳
    pub created_ms: u64,
    /// 投票截止时间戳
    pub deadline_ms: u64,
}

pub enum ProposalType {
    /// 新节点加入集群
    NodeJoin,
    /// 节点主动离开
    NodeLeave,
    /// 配置变更
    ConfigChange,
    /// Agent 部署/更新
    AgentDeploy,
    /// 全局策略更新
    PolicyUpdate,
}
```

### 4.3 需要在原架构中修改的部分

| 领域 | 需要改造 | 复杂度 |
|------|---------|--------|
| `AgentRegistry` | 增加跨节点查询，本地+远端两层 | 中 |
| `AgentSelector` | 增加基于节点负载的加权评分 | 低 |
| `OrchestrationCouncil` | 投票改为 gRPC RPC，支持跨节点成员 | 高 |
| `MultiAgentPipeline` | 任务可调度到远程 Worker | 中 |
| `TaskDecomposer` | 支持分片感知（知道已分片到哪些节点） | 中 |
| `Session` | 增加跨节点共享，etcd 或 Redis 后端 | 中 |
| `Memory`/`VectorStore` | pgvector 支持分布式查询（已有蓝图） | 低 |
| `ConsciousnessMetrics` | 跨节点聚合 | 中 |
| `AlertManager` | 跨节点告警汇聚 | 低 |
| `governance.status` | 增加集群视图（已知哪些节点在线） | 中 |

---

## 5. 架构设计

### 5.1 整体拓扑

```
                        ┌───────────────────┐
                        │    Load Balancer   │
                        │  (nginx/HAProxy)   │
                        └────────┬──────────┘
                                 │
          ┌──────────────────────┼──────────────────────┐
          ▼                      ▼                      ▼
   ┌─────────────┐       ┌─────────────┐       ┌─────────────┐
   │ Coordinator │       │  Worker A   │       │  Worker B   │
   │  (Leader)   │──────►│  (Node 2)   │──────►│  (Node 3)   │
   │             │ gRPC  │             │ gRPC  │             │
   │ ● Council   │◄──────│ ● Agent 1   │◄──────│ ● Agent 2   │
   │ ● Scheduler │       │ ● Agent 3   │       │ ● Agent 5   │
   │ ● Registry  │       │ ● Memory    │       │ ● Memory    │
   │ ● Session   │       │ ● Vector    │       │ ● Vector    │
   └──────┬──────┘       └──────┬──────┘       └──────┬──────┘
          │                     │                     │
          │              ┌──────┴──────┐              │
          │              │  Worker C   │              │
          │              │  (Node N)   │              │
          │              │ ● Agent 4   │              │
          │              └──────┬──────┘              │
          │                     │                     │
          └─────────────────────┼─────────────────────┘
                                │
                    ┌───────────┴───────────┐
                    │      etcd 集群         │
                    │  (共识 + 配置 + 发现)   │
                    └───────────────────────┘
                    ┌───────────────────────┐
                    │      NATS 集群         │
                    │  (任务队列 + 消息)      │
                    └───────────────────────┘
                    ┌───────────────────────┐
                    │   PostgreSQL + Citus   │
                    │  (分布式向量 + 状态)    │
                    └───────────────────────┘
```

### 5.2 Coordinator 职责

```
Coordinator (Leader 节点)
  │
  ├─ Service Registry ← etcd
  │   ├─ 处理节点注册/注销
  │   ├─ 维护节点能力索引
  │   └─ 健康检查（Heartbeat 汇聚）
  │
  ├─ Task Scheduler
  │   ├─ 接收来自 Gateway 的用户请求
  │   ├─ 查询 Registry 获取可用 Worker
  │   ├─ 按能力/负载/亲和性分配 Worker
  │   └─ 推送任务到 NATS 任务队列
  │
  ├─ Council (分布式)
  │   ├─ 跨节点成员管理
  │   ├─ 提案广播 + RPC 投票
  │   └─ 共识结果写回 etcd
  │
  ├─ Session Manager
  │   ├─ 用户 → Worker 映射
  │   ├─ 会话亲和性路由
  │   └─ Worker 故障时迁移会话
  │
  ├─ Leader Election (Raft)
  │   ├─ 多 Coordinator 节点选主
  │   ├─ 脑裂检测与预防
  │   └─ 自动切主
  │
  └─ Observability
      ├─ 分布式 Trace 收集中间件
      ├─ 节点指标聚合
      └─ 全局仪表盘
```

### 5.3 Worker 职责

```
Worker 节点
  │
  ├─ Node Agent
  │   ├─ 向 etcd 注册自身（能力+地址）
  │   ├─ 定期发送 Heartbeat
  │   ├─ 监控本地资源（CPU/内存/GPU）
  │   └─ 监听 shutdown 信号（优雅退出）
  │
  ├─ Task Executor
  │   ├─ 从 NATS 拉取分配给自己的任务
  │   ├─ 本地执行（Agent.chat() / Tool.run()）
  │   ├─ 执行结果写回 etcd / PostgreSQL
  │   └─ 任务超时/失败处理和重试
  │
  ├─ Local Agent Registry
  │   ├─ 本地 Agent 实例管理
  │   ├─ Agent 间通信（本地 = 函数调用）
  │   └─ Agent 热加载/卸载
  │
  ├─ Local Memory/Vector
  │   ├─ 本地 L1 Cache（高速）
  │   ├─ 分布式 L2 Cache 查询（pgvector）
  │   └─ 缓存一致性维护
  │
  └─ Local Governance
      ├─ 本地速率限制
      ├─ 本地 PUA 执行
      └─ 本地审计事件缓存（批量上报）
```

---

## 6. 跨节点任务执行流程

### 6.1 用户请求 → 分布式执行（完整流程）

```text
step 1: 用户发送请求 → Load Balancer
                │
step 2: LB → Coordinator (Leader)
                │
step 3: Coordinator 解析请求
        ├─ 提取用户会话 → Session Manager → 查 etcd: 该用户固定在哪个 Worker？
        ├─ 提取所需能力 → Registry → 查 etcd: 哪些 Worker 有此能力？
        └─ 任务分派 → Scheduler:
            ├─ 如果有会话亲和性 → 发到同一个 Worker
            ├─ 如果有数据本地化 → 发到数据所在 Worker
            └─ 否则 → 选负载最低的 Worker
                │
step 4: Coordinator → NATS: TaskAssignment{worker_id, task_payload}
                │
step 5: Worker 从 NATS 接收任务
        ├─ 创建本地 ExecutionContext
        ├─ 检查本地 AgentRegistry（或从 Registry 拉取 Agent）
        ├─ 如果需要子任务 → 提交子任务回 Coordinator（递归）
        └─ 执行任务（Chat/Tool/MultiAgentPipeline）
                │
step 6: 执行中
        ├─ Streaming 中间结果 → NATS → Coordinator → SSE → 用户
        ├─ Token 消耗 → 本地记录 → 批量上报 Coordinator
        └─ 心跳持续 → etcd 更新时间戳
                │
step 7: 执行完成
        ├─ 最终结果 → NATS → Coordinator → 用户
        ├─ 审计事件 → 本地缓存 → 批量写入 PostgreSQL
        ├─ Token 消耗 → 上报 Coordinator → TenantBudget
        └─ 任务完成标记 → etcd
                │
step 8: 故障场景（Worker 崩溃）
        ├─ Coordinator 检测到心跳超时
        ├─ etcd Worker 节点标记 offline
        ├─ 该 Worker 上所有活跃任务被重新入队
        ├─ 调度器避让该节点
        └─ 重新分派到健康 Worker（带幂等性 token）
```

### 6.2 Agent 间跨节点通信

```text
Agent A (Worker 1) → Agent B (Worker 2)
  │
  ├─ Agent A 输出 → 产出 AgentTaskEnvelope{target_agent: "agent-b", payload}
  ├─ MultiAgentPipeline 发现 target_agent 不在本地
  ├─ 提交到 Coordinator 的跨节点路由
  │
  ├─ Coordinator 查 Registry → Agent B 在 Worker 2 上
  ├─ Coordinator → NATS: InterAgentMessage{from: "agent-a", to: "agent-b", payload}
  │
  ├─ Worker 2 从 NATS 接收 → 交付给本地 Agent B
  ├─ Agent B 执行 → 产出结果
  └─ 结果原路返回（NATS → Coordinator → Worker 1）
```

### 6.3 分布式 Council 投票流程

```text
提案: "是否批准高风险代码变更？"
  │
  ├─ Coordinator.Council 创建提案 → etcd /proposals/{proposal_id}
  ├─ 通知所有 Council 成员（gRPC 广播或 etcd watch）
  │
  ├─ Worker A 的 CouncilMember: 拉取提案 → 本地评估 → 投票 → 写入 etcd
  ├─ Worker B 的 CouncilMember: 同上
  ├─ Worker C 的 CouncilMember: 同上
  │
  ├─ Coordinator 监听 etcd /votes/{proposal_id}/* 变化
  ├─ 达到法定人数 → 统计结果
  ├─ ConsensusEngine 检查:
  │   ├─ 加权多数通过 → ProposalStatus::Approved
  │   ├─ 加权多数否决 → ProposalStatus::Rejected
  │   └─ 未达法定人数 → 延长投票期
  │
  └─ 结果写入 etcd → 所有节点 watch 到 → 各自执行决议
```

---

## 7. gRPC 协议定义

### 7.1 Worker Swarm 服务 (swarm.proto)

```protobuf
syntax = "proto3";
package go_on.swarm;

// ─── 节点管理 ──────────────────────────────────────────────────────

service NodeService {
  // 节点注册（由 Worker → Coordinator）
  rpc Register(RegisterRequest) returns (RegisterResponse);
  // 心跳（由 Worker → Coordinator）
  rpc Heartbeat(HeartbeatRequest) returns (HeartbeatResponse);
  // 优雅退出通知
  rpc Leave(LeaveRequest) returns (LeaveResponse);
  // 批量获取节点列表
  rpc ListNodes(ListNodesRequest) returns (ListNodesResponse);
}

message RegisterRequest {
  string node_id = 1;
  string addr = 2;           // gRPC 监听地址
  NodeRole role = 3;
  repeated string capabilities = 4;  // ["gpu-a100", "model-gpt4", "memory-64gb"]
  GpuInfo gpu = 5;           // GPU 信息（可选）
  repeated string available_models = 6;
}

message RegisterResponse {
  bool accepted = 1;
  string cluster_id = 2;     // 分配的集群 ID
  string error = 3;          // 拒绝原因（如果 accepted=false）
}

message HeartbeatRequest {
  string node_id = 1;
  double load = 2;           // 0.0 - 1.0
  uint32 active_tasks = 3;
  double memory_usage = 4;   // 0.0 - 1.0
}

message HeartbeatResponse {
  bool acknowledged = 1;
  // Coordinator 命令（如触发重启、驱逐）
  repeated NodeCommand commands = 2;
}

message NodeCommand {
  CommandType type = 1;
  string payload = 2;        // JSON 格式的命令参数
}

enum CommandType {
  COMMAND_UNSPECIFIED = 0;
  COMMAND_EVICT = 1;         // 驱逐该节点上的所有任务
  COMMAND_RESTART = 2;       // 重启节点
  COMMAND_DRAIN = 3;         // 排空任务（不再分配新任务）
  COMMAND_UPDATE_CONFIG = 4; // 更新节点配置
}
```

```protobuf
// ─── 任务调度 ──────────────────────────────────────────────────────

service TaskService {
  // Coordinator → Worker 提交任务
  rpc SubmitTask(TaskRequest) returns (TaskResponse);
  // Worker 拉取待执行任务（由 Coordinator 分配的）
  rpc PullTask(PullTaskRequest) returns (PullTaskResponse);
  // 报告任务执行结果
  rpc ReportTaskResult(TaskResultRequest) returns (TaskResultResponse);
  // 子任务提交（Worker → Coordinator，用于任务再分解）
  rpc SubmitSubtask(SubtaskRequest) returns (SubtaskResponse);
}

message TaskRequest {
  string task_id = 1;
  string idempotency_token = 2;  // 幂等性去重
  string task_type = 3;
  string description = 4;
  repeated string required_capabilities = 5;
  bytes payload = 6;             // 序列化的任务参数（JSON）
  uint32 priority = 7;           // 0-100
  uint64 timeout_secs = 8;
  uint32 max_retries = 9;
  string parent_task_id = 10;    // 子任务链追踪
  string session_id = 11;        // 会话亲和性
}

message TaskResponse {
  bool accepted = 1;
  string error = 2;
}

message TaskResultRequest {
  string task_id = 1;
  bool success = 2;
  bytes output = 3;             // 序列化输出（JSON）
  string error_message = 4;
  uint64 duration_ms = 5;
  uint32 input_tokens = 6;
  uint32 output_tokens = 7;
  repeated AuditEvent audit_events = 8;
}
```

```protobuf
// ─── 跨节点 Agent 通信 ─────────────────────────────────────────────

service AgentService {
  // 跨节点 Agent 消息传递
  rpc SendAgentMessage(AgentMessageRequest) returns (AgentMessageResponse);
  // 查询 Agent 位置
  rpc LocateAgent(AgentLocateRequest) returns (AgentLocateResponse);
}

message AgentMessageRequest {
  string from_agent = 1;
  string to_agent = 2;
  string conversation_id = 3;
  bytes payload = 4;
  bool require_response = 5;
  uint64 timeout_ms = 6;
}
```

```protobuf
// ─── 分布式 Council ────────────────────────────────────────────────

service CouncilService {
  // 提交提案
  rpc SubmitProposal(ProposalRequest) returns (ProposalResponse);
  // 投票
  rpc CastVote(VoteRequest) returns (VoteResponse);
  // 查询提案状态
  rpc GetProposalStatus(ProposalStatusRequest) returns (ProposalStatusResponse);
}

message ProposalRequest {
  string proposal_id = 1;
  string title = 2;
  string description = 3;
  string proposer_node = 4;
  repeated string options = 5;
  uint64 deadline_ms = 6;
}

message VoteRequest {
  string proposal_id = 1;
  string member_node = 2;
  string selected_option = 3;
  uint32 voting_power = 4;
  string rationale = 5;
}
```

```protobuf
// ─── 状态同步 ──────────────────────────────────────────────────────

service StateSyncService {
  // 订阅状态变更（如 Session 更新、Agent 状态变化）
  rpc WatchState(WatchStateRequest) returns (stream StateEvent);
  // 执行状态同步 RPC
  rpc SyncState(SyncStateRequest) returns (SyncStateResponse);
}
```

### 7.2 现有代码的集成点

```rust
// 现有的 GrpcFederatedTransport 桩 → 需要改为真正实现：
// src/intelligence/reinforcement/federated_transport.rs

impl FederatedTransport for GrpcFederatedTransport {
    async fn submit_weights(&self, peer: &PeerInfo, weights: &ModelWeights) -> Result<bool> {
        // 当前：Ok(true) ← 桩代码
        // 需要: 
        //   let channel = self.connect(peer).await?;
        //   let mut client = FederatedServiceClient::new(channel);
        //   let req = SubmitWeightsRequest { ... };
        //   let resp = client.submit_weights(req).await?;
        //   Ok(resp.into_inner().accepted)
    }
}
```

---

## 8. 实现优先级与路线图

### 阶段一：基础通信 (P0) — 1-2 月

| 编号 | 任务 | 产出 |
|------|------|------|
| P0-1 | 创建 `proto/swarm.proto` 和 `build.rs` | protobuf 编译通过 |
| P0-2 | 实现真正的 `GrpcNodeTransport`（node.proto） | 节点间 gRPC 可通信 |
| P0-3 | etcd 集成：`EtcdRegistry` 实现 `NodeDiscovery` | 节点可向 etcd 注册 |
| P0-4 | 心跳系统对接 etcd | 节点离线可检测 |
| P0-5 | `DistributedConfig` + 环境变量解析 | 节点启动配置 |
| P0-6 | 单机集成测试：2 个 Worker + 1 个 Coordinator | 基础拓扑可用 |

### 阶段二：任务调度 (P0) — 2-3 月

| 编号 | 任务 | 产出 |
|------|------|------|
| P0-7 | NATS/RabbitMQ 集成：`DistributedTaskQueue` | 任务可在节点间流转 |
| P0-8 | Coordinator 调度器：能力匹配 + 负载感知 | 任务被分配到正确的 Worker |
| P0-9 | 任务幂等性和去重 | 故障恢复不重复执行 |
| P0-10 | 将 `MultiAgentPipeline` 改为支持远程 Agent | 多 Agent 跨节点协作 |
| P0-11 | `AgentRegistry` 增加远端代理层 | 本地没有 Agent 时查远端 |

### 阶段三：共识与选举 (P1) — 3-4 月

| 编号 | 任务 | 产出 |
|------|------|------|
| P1-1 | Raft Leader 选举实现 | Coordinator HA |
| P1-2 | 分布式 Council（投票→RPC） | 跨节点治理决策 |
| P1-3 | etcd 配置/提案存储 | 集群一致配置 |
| P1-4 | 脑裂检测和预防 | 网络分区安全 |
| P1-5 | 分布式 Session 管理器 | 用户会话跨节点可用 |

### 阶段四：容错与可观测 (P1) — 4-5 月

| 编号 | 任务 | 产出 |
|------|------|------|
| P1-6 | Worker 崩溃后任务自动迁移 | 零中断容错 |
| P1-7 | 分布式 Trace 串联（OTel） | 请求完整链路 |
| P1-8 | 跨节点日志聚合 | 统一日志查询 |
| P1-9 | 节点 Dashboard | 集群状态可视化 |
| P1-10 | 优雅退出 + drain 模式 | 运维不中断 |

### 阶段五：高级功能 (P2) — 5-8 月

| 编号 | 任务 | 产出 |
|------|------|------|
| P2-1 | 自动扩缩容 | 基于负载的 Worker 自动启停 |
| P2-2 | 全局速率限制和预算 | 多租户跨节点控制 |
| P2-3 | 数据本地化调度优化 | 减少跨节点数据传输 |
| P2-4 | Agent 热迁移 | 不中断服务的 Agent 移动 |
| P2-5 | 联邦学习全链路 | 跨节点模型训练闭环 |
| P2-6 | 全局性能基准对比 | 跨节点指标对比分析 |

---

## 9. 风险与应对

| 风险 | 影响 | 概率 | 应对 |
|------|------|------|------|
| **网络分区导致脑裂** | 两半集群各自选主，状态分裂 | 高 | etcd Raft + 多数派原则 + fencing |
| **gRPC 调用超时链式放大** | 节点变慢导致所有调用排队 | 高 | 超时传播 + 熔断器 + 后备降级 |
| **分布式状态不一致** | 不同节点看到不同的 Agent 状态 | 中 | 写操作走 etcd CAS + 读操作容忍最终一致 |
| **跨节点 Trace 数据爆炸** | 高并发场景下 Trace 存储飙升 | 中 | 采样率自适应 + 聚合 Trace |
| **任务重复执行** | 重试导致同一任务被执行两次 | 中 | 幂等性 token + 去重表 |
| **节点加入/离开风暴** | 大规模节点变更导致集群不稳定 | 低 | 变更速率限制 + 渐进式注册 |
| **配置漂移** | 不同节点配置不一致 | 低 | 配置版本化 + etcd watch + 强制对齐 |
| **安全边界突破** | 节点间通信被截获或篡改 | 低 | mTLS 双向认证 + 请求签名 |

---

## 10. 部署方案

### 10.1 部署概述

Worker Swarm 分布式部署包含以下组件：

| 组件 | 部署方式 | 实例数 | 说明 |
|------|---------|--------|------|
| **Coordinator** | Deployment/StatefulSet | 3（HA） | 处理请求路由、任务调度、Council 决策 |
| **Worker** | Deployment | N（动态扩缩） | 执行 Agent 任务和 Tool 调用 |
| **etcd** | StatefulSet | 3 | 服务注册、配置存储、Leader 选举 |
| **NATS** | StatefulSet | 3 | 分布式任务队列、Agent 间消息 |
| **PostgreSQL + Citus** | StatefulSet | 2+ | 分布式向量存储、会话持久化 |
| **Redis** | StatefulSet | 3（Sentinel） | 分布式缓存、Session 缓存 |
| **Load Balancer** | Service/Ingress | 2 | 用户请求入口，TLS 终止 |
| **Prometheus + Grafana** | Deployment | 1 | 集群监控 |
| **Tempo + Loki** | Deployment | 1 | 分布式 Trace + 日志 |

### 10.2 Docker 镜像构建

#### 10.2.1 多阶段 Dockerfile

```dockerfile
# Dockerfile — 多架构构建，产物用于 Coordinator 和 Worker

# ── Stage 1: Build ────────────────────────────────────────────────
FROM rust:1.85-slim-bookworm AS builder

ARG PROFILE=multi-users-server
ARG FEATURES="--no-default-features -F $PROFILE,swarm"

RUN apt-get update && apt-get install -y \
    protobuf-compiler \
    libprotobuf-dev \
    cmake \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .

RUN cargo build --locked $FEATURES --release

# ── Stage 2: Runtime (Coordinator) ────────────────────────────────
FROM debian:bookworm-slim AS coordinator

RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/go-on /usr/local/bin/go-on

EXPOSE 8090 9090 50051

HEALTHCHECK --interval=10s --timeout=3s --retries=3 \
    CMD curl -sf http://localhost:8090/health || exit 1

ENTRYPOINT ["go-on", "--features", "swarm", "--profile", "multi-users-server"]

# ── Stage 2b: Runtime (Worker) ────────────────────────────────────
FROM debian:bookworm-slim AS worker

RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    git \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/go-on /usr/local/bin/go-on

EXPOSE 50051

HEALTHCHECK --interval=10s --timeout=3s --retries=3 \
    CMD curl -sf http://localhost:50051/health || exit 1

ENTRYPOINT ["go-on", "--features", "swarm", "--profile", "multi-users-server", "--node-role", "worker"]
```

#### 10.2.2 构建命令

```bash
# 构建 Coordinator 镜像
docker build --target coordinator -t go-on/coordinator:latest .

# 构建 Worker 镜像
docker build --target worker -t go-on/worker:latest .

# 多架构构建（ARM64 + AMD64）
docker buildx build \
    --platform linux/amd64,linux/arm64 \
    --target coordinator \
    -t go-on/coordinator:1.0.0 \
    --push .
```

### 10.3 Helm Chart 部署（Kubernetes）

以下是一个完整的 Helm chart 目录结构：

```text
helm/go-on-swarm/
├── Chart.yaml          # chart 元数据
├── values.yaml         # 默认配置
├── values-prod.yaml    # 生产环境覆盖
├── values-staging.yaml # 预发布环境覆盖
├── templates/
│   ├── _helpers.tpl    # 模板帮助函数
│   ├── namespace.yaml
│   ├── coordinator/
│   │   ├── deployment.yaml   # Coordinator Deployment
│   │   ├── service.yaml      # Coordinator Service
│   │   ├── hpa.yaml          # 水平自动扩缩（Coordinator 通常不用）
│   │   └── pdb.yaml          # Pod 中断预算
│   ├── worker/
│   │   ├── deployment.yaml   # Worker Deployment
│   │   ├── hpa.yaml          # Worker 自动扩缩
│   │   └── pdb.yaml
│   ├── etcd/
│   │   └── statefulset.yaml  # etcd 集群
│   ├── nats/
│   │   └── statefulset.yaml  # NATS 集群
│   ├── postgres/
│   │   └── statefulset.yaml  # PostgreSQL + Citus
│   ├── redis/
│   │   └── statefulset.yaml  # Redis Sentinel
│   ├── ingress.yaml          # 流量入口
│   ├── configmap.yaml        # 共享配置
│   └── monitoring/
│       ├── prometheus-rule.yaml
│       └── grafana-dashboard.yaml
```

#### 10.3.1 values.yaml 核心配置

```yaml
# helm/go-on-swarm/values.yaml

# ── 全局配置 ──────────────────────────────────────────────────────
global:
  imageRegistry: "ghcr.io/go-on"
  imageTag: "1.0.0"
  imagePullPolicy: "IfNotPresent"

# ── Coordinator ───────────────────────────────────────────────────
coordinator:
  replicas: 3  # HA 至少 3 副本
  resources:
    requests:
      cpu: "2"
      memory: "4Gi"
    limits:
      cpu: "4"
      memory: "8Gi"
  env:
    GO_ON_NODE_ROLE: "coordinator"
    GO_ON_LISTEN_ADDR: "0.0.0.0:50051"
    GO_ON_HTTP_PORT: "8090"
    GO_ON_ETCD_ENDPOINTS: "etcd-0.etcd:2379,etcd-1.etcd:2379,etcd-2.etcd:2379"
    GO_ON_NATS_URLS: "nats://nats-0.nats:4222,nats://nats-1.nats:4222,nats://nats-2.nats:4222"
    GO_ON_POSTGRES_DSN: "postgres://goon:password@postgres:5432/goon?sslmode=require"
    GO_ON_REDIS_URL: "redis://redis-sentinel:26379"
    RUST_LOG: "info,go_on=debug"
  # Leader 选举配置
  election:
    enabled: true
    timeoutMs: 3000
    heartbeatIntervalMs: 500

# ── Worker ────────────────────────────────────────────────────────
worker:
  replicas: 10    # 初始 10 个 Worker
  minReplicas: 3  # HPA 最少 3
  maxReplicas: 100  # HPA 最多 100
  resources:
    requests:
      cpu: "4"       # Worker 需要更多 CPU（LLM 推理）
      memory: "8Gi"
    limits:
      cpu: "8"
      memory: "16Gi"
  env:
    GO_ON_NODE_ROLE: "worker"
    GO_ON_LISTEN_ADDR: "0.0.0.0:50051"
    GO_ON_ETCD_ENDPOINTS: "etcd-0.etcd:2379,etcd-1.etcd:2379,etcd-2.etcd:2379"
    GO_ON_NATS_URLS: "nats://nats-0.nats:4222,nats://nats-1.nats:4222,nats://nats-2.nats:4222"
    GO_ON_POSTGRES_DSN: "postgres://goon:password@postgres:5432/goon?sslmode=require"
    RUST_LOG: "info,go_on=debug"
  # Worker 能力标签——影响调度器分配
  capabilities:
    - "gpu-a100"
    - "model-gpt4"
    - "memory-64gb"
  hpa:
    enabled: true
    targetCPUUtilization: 70
    targetMemoryUtilization: 80
    behavior:
      scaleDown:
        stabilizationWindowSeconds: 300
        policies:
        - type: Pods
          value: 2
          periodSeconds: 60
      scaleUp:
        stabilizationWindowSeconds: 0
        policies:
        - type: Percent
          value: 100
          periodSeconds: 15

# ── etcd 集群 ─────────────────────────────────────────────────────
etcd:
  replicas: 3
  storage: "20Gi"
  storageClass: "ssd"
  resources:
    requests:
      cpu: "1"
      memory: "2Gi"

# ── NATS 集群 ─────────────────────────────────────────────────────
nats:
  replicas: 3
  storage: "10Gi"
  storageClass: "ssd"

# ── PostgreSQL + Citus ─────────────────────────────────────────────
postgres:
  replicas: 2
  storage: "100Gi"
  storageClass: "ssd"
  resources:
    requests:
      cpu: "2"
      memory: "8Gi"
  db:
    name: "goon"
    user: "goon"
    password: ""  # 通过 Secret 注入
    ssl: "require"

# ── Ingress ───────────────────────────────────────────────────────
ingress:
  enabled: true
  host: "api.go-on.ai"
  tls:
    secretName: "go-on-tls"
  annotations:
    nginx.ingress.kubernetes.io/proxy-body-size: "64m"
    nginx.ingress.kubernetes.io/proxy-read-timeout: "300"
    nginx.ingress.kubernetes.io/proxy-send-timeout: "300"
```

### 10.4 Kubernetes 核心 Templates

#### 10.4.1 Coordinator Deployment

```yaml
# helm/go-on-swarm/templates/coordinator/deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: {{ include "swarm.fullname" . }}-coordinator
  labels:
    app: {{ include "swarm.name" . }}
    component: coordinator
spec:
  replicas: {{ .Values.coordinator.replicas }}
  strategy:
    type: RollingUpdate
    rollingUpdate:
      maxSurge: 1
      maxUnavailable: 0  # 零停机更新
  selector:
    matchLabels:
      app: {{ include "swarm.name" . }}
      component: coordinator
  template:
    metadata:
      labels:
        app: {{ include "swarm.name" . }}
        component: coordinator
    spec:
      affinity:
        podAntiAffinity:
          preferredDuringSchedulingIgnoredDuringExecution:
          - weight: 100
            podAffinityTerm:
              labelSelector:
                matchExpressions:
                - key: component
                  operator: In
                  values:
                  - coordinator
              topologyKey: "kubernetes.io/hostname"
      terminationGracePeriodSeconds: 60
      containers:
      - name: coordinator
        image: "{{ .Values.global.imageRegistry }}/coordinator:{{ .Values.global.imageTag }}"
        imagePullPolicy: {{ .Values.global.imagePullPolicy }}
        ports:
        - containerPort: 8090
          name: http
        - containerPort: 50051
          name: grpc
        - containerPort: 9090
          name: metrics
        env:
        {{- range $key, $val := .Values.coordinator.env }}
        - name: {{ $key }}
          value: {{ $val | quote }}
        {{- end }}
        - name: GO_ON_NODE_ID
          valueFrom:
            fieldRef:
              fieldPath: metadata.name
        - name: GO_ON_POD_IP
          valueFrom:
            fieldRef:
              fieldPath: status.podIP
        resources:
          {{- toYaml .Values.coordinator.resources | nindent 10 }}
        readinessProbe:
          httpGet:
            path: /health
            port: 8090
          initialDelaySeconds: 10
          periodSeconds: 10
        livenessProbe:
          httpGet:
            path: /health
            port: 8090
          initialDelaySeconds: 30
          periodSeconds: 20
```

#### 10.4.2 Worker Deployment（含 HPA）

```yaml
# helm/go-on-swarm/templates/worker/deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: {{ include "swarm.fullname" . }}-worker
  labels:
    app: {{ include "swarm.name" . }}
    component: worker
spec:
  replicas: {{ .Values.worker.replicas }}
  strategy:
    type: RollingUpdate
    rollingUpdate:
      maxSurge: 5        # 快速扩容
      maxUnavailable: 1  # 允许 1 个不可用
  selector:
    matchLabels:
      app: {{ include "swarm.name" . }}
      component: worker
  template:
    metadata:
      labels:
        app: {{ include "swarm.name" . }}
        component: worker
    spec:
      affinity:
        podAntiAffinity:
          preferredDuringSchedulingIgnoredDuringExecution:
          - weight: 50
            podAffinityTerm:
              labelSelector:
                matchExpressions:
                - key: component
                  operator: In
                  values:
                  - worker
              topologyKey: "kubernetes.io/hostname"
      terminationGracePeriodSeconds: 120  # Worker 需要更长时间完成当前任务
      containers:
      - name: worker
        image: "{{ .Values.global.imageRegistry }}/worker:{{ .Values.global.imageTag }}"
        imagePullPolicy: {{ .Values.global.imagePullPolicy }}
        ports:
        - containerPort: 50051
          name: grpc
        - containerPort: 9090
          name: metrics
        env:
        {{- range $key, $val := .Values.worker.env }}
        - name: {{ $key }}
          value: {{ $val | quote }}
        {{- end }}
        - name: GO_ON_NODE_ID
          valueFrom:
            fieldRef:
              fieldPath: metadata.name
        - name: GO_ON_NODE_CAPABILITIES
          value: {{ join "," .Values.worker.capabilities | quote }}
        resources:
          {{- toYaml .Values.worker.resources | nindent 10 }}
        readinessProbe:
          grpc:
            port: 50051
          initialDelaySeconds: 15
          periodSeconds: 10
        livenessProbe:
          grpc:
            port: 50051
          initialDelaySeconds: 30
          periodSeconds: 20
---
# HPA — Worker 自动扩缩
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: {{ include "swarm.fullname" . }}-worker
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: {{ include "swarm.fullname" . }}-worker
  minReplicas: {{ .Values.worker.minReplicas }}
  maxReplicas: {{ .Values.worker.maxReplicas }}
  metrics:
  - type: Resource
    resource:
      name: cpu
      target:
        type: Utilization
        averageUtilization: {{ .Values.worker.hpa.targetCPUUtilization }}
  - type: Resource
    resource:
      name: memory
      target:
        type: Utilization
        averageUtilization: {{ .Values.worker.hpa.targetMemoryUtilization }}
  behavior:
    scaleDown:
      stabilizationWindowSeconds: {{ .Values.worker.hpa.behavior.scaleDown.stabilizationWindowSeconds }}
    scaleUp:
      stabilizationWindowSeconds: {{ .Values.worker.hpa.behavior.scaleUp.stabilizationWindowSeconds }}
```

#### 10.4.3 Ingress（用户请求入口）

```yaml
# helm/go-on-swarm/templates/ingress.yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: {{ include "swarm.fullname" . }}-ingress
  annotations:
    {{- range $key, $val := .Values.ingress.annotations }}
    {{ $key }}: {{ $val | quote }}
    {{- end }}
spec:
  ingressClassName: nginx
  tls:
  - hosts:
    - {{ .Values.ingress.host }}
    secretName: {{ .Values.ingress.tls.secretName }}
  rules:
  - host: {{ .Values.ingress.host }}
    http:
      paths:
      - path: /chat
        pathType: Prefix
        backend:
          service:
            name: {{ include "swarm.fullname" . }}-coordinator
            port:
              number: 8090
      - path: /health
        pathType: Prefix
        backend:
          service:
            name: {{ include "swarm.fullname" . }}-coordinator
            port:
              number: 8090
```

### 10.5 docker-compose 本地测试部署

用于开发环境和 CI 的单机多节点模拟：

```yaml
# docker-compose.yml
version: "3.9"

services:
  # ── 基础设施 ──────────────────────────────────────────────────
  etcd-0:
    image: bitnami/etcd:3.5
    environment:
      - ALLOW_NONE_AUTHENTICATION=yes
      - ETCD_NAME=etcd-0
      - ETCD_INITIAL_CLUSTER=etcd-0=http://etcd-0:2380,etcd-1=http://etcd-1:2380,etcd-2=http://etcd-2:2380
      - ETCD_INITIAL_CLUSTER_STATE=new
  etcd-1:
    image: bitnami/etcd:3.5
    environment:
      - ALLOW_NONE_AUTHENTICATION=yes
      - ETCD_NAME=etcd-1
      - ETCD_INITIAL_CLUSTER=etcd-0=http://etcd-0:2380,etcd-1=http://etcd-1:2380,etcd-2=http://etcd-2:2380
      - ETCD_INITIAL_CLUSTER_STATE=new
  etcd-2:
    image: bitnami/etcd:3.5
    environment:
      - ALLOW_NONE_AUTHENTICATION=yes
      - ETCD_NAME=etcd-2
      - ETCD_INITIAL_CLUSTER=etcd-0=http://etcd-0:2380,etcd-1=http://etcd-1:2380,etcd-2=http://etcd-2:2380
      - ETCD_INITIAL_CLUSTER_STATE=new

  nats-0:
    image: nats:2.10-alpine
    command: "-c /etc/nats/nats.conf"
    volumes:
      - ./deploy/nats.conf:/etc/nats/nats.conf
  nats-1:
    image: nats:2.10-alpine
    command: "-c /etc/nats/nats.conf"
    volumes:
      - ./deploy/nats.conf:/etc/nats/nats.conf
  nats-2:
    image: nats:2.10-alpine
    command: "-c /etc/nats/nats.conf"
    volumes:
      - ./deploy/nats.conf:/etc/nats/nats.conf

  postgres:
    image: citusdata/citus:12.1
    environment:
      - POSTGRES_USER=goon
      - POSTGRES_PASSWORD=devpassword
      - POSTGRES_DB=goon
    volumes:
      - pgdata:/var/lib/postgresql/data
    ports:
      - "5432:5432"

  redis-sentinel:
    image: bitnami/redis-sentinel:7.2
    environment:
      - REDIS_SENTINEL_QUORUM=2

  # ── Coordinator ────────────────────────────────────────────────
  coordinator-0:
    image: go-on/coordinator:dev
    ports:
      - "8090:8090"   # HTTP (用户请求入口)
      - "50051:50051" # gRPC (节点间通信)
    environment:
      GO_ON_NODE_ROLE: "coordinator"
      GO_ON_NODE_ID: "coordinator-0"
      GO_ON_LISTEN_ADDR: "0.0.0.0:50051"
      GO_ON_HTTP_PORT: "8090"
      GO_ON_ETCD_ENDPOINTS: "etcd-0:2379,etcd-1:2379,etcd-2:2379"
      GO_ON_NATS_URLS: "nats://nats-0:4222,nats://nats-1:4222,nats://nats-2:4222"
      GO_ON_POSTGRES_DSN: "postgres://goon:devpassword@postgres:5432/goon"
      RUST_LOG: "debug"
    depends_on:
      etcd-0:
        condition: service_started
      nats-0:
        condition: service_started

  coordinator-1:
    image: go-on/coordinator:dev
    ports:
      - "8091:8090"
      - "50052:50051"
    environment:
      GO_ON_NODE_ROLE: "coordinator"
      GO_ON_NODE_ID: "coordinator-1"
      GO_ON_LISTEN_ADDR: "0.0.0.0:50051"
      GO_ON_HTTP_PORT: "8090"
      GO_ON_ETCD_ENDPOINTS: "etcd-0:2379,etcd-1:2379,etcd-2:2379"
      GO_ON_NATS_URLS: "nats://nats-0:4222,nats://nats-1:4222,nats://nats-2:4222"
      GO_ON_POSTGRES_DSN: "postgres://goon:devpassword@postgres:5432/goon"
      RUST_LOG: "debug"
    depends_on:
      etcd-0:
        condition: service_started

  # ── Worker ─────────────────────────────────────────────────────
  worker-0:
    image: go-on/worker:dev
    environment:
      GO_ON_NODE_ROLE: "worker"
      GO_ON_NODE_ID: "worker-0"
      GO_ON_LISTEN_ADDR: "0.0.0.0:50051"
      GO_ON_ETCD_ENDPOINTS: "etcd-0:2379,etcd-1:2379,etcd-2:2379"
      GO_ON_NATS_URLS: "nats://nats-0:4222,nats://nats-1:4222,nats://nats-2:4222"
      GO_ON_NODE_CAPABILITIES: "gpu-a100,model-gpt4,memory-64gb"
      RUST_LOG: "debug"
    depends_on:
      - coordinator-0
      - etcd-0
      - nats-0
    deploy:
      resources:
        reservations:
          devices:
          - driver: nvidia
            count: 1
            capabilities: [gpu]

  worker-1:
    image: go-on/worker:dev
    environment:
      GO_ON_NODE_ROLE: "worker"
      GO_ON_NODE_ID: "worker-1"
      GO_ON_LISTEN_ADDR: "0.0.0.0:50051"
      GO_ON_ETCD_ENDPOINTS: "etcd-0:2379,etcd-1:2379,etcd-2:2379"
      GO_ON_NATS_URLS: "nats://nats-0:4222,nats://nats-1:4222,nats://nats-2:4222"
      GO_ON_NODE_CAPABILITIES: "cpu-only,model-llama3,memory-32gb"
      RUST_LOG: "debug"
    depends_on:
      - coordinator-0
      - etcd-0
      - nats-0

volumes:
  pgdata:
```

#### 10.5.1 NATS 配置文件

```conf
# deploy/nats.conf
port: 4222

cluster {
  port: 6222
  routes: [
    nats-route://nats-0:6222,
    nats-route://nats-1:6222,
    nats-route://nats-2:6222
  ]
}

jetstream {
  store_dir: /data/jetstream
  max_memory_store: 1GB
}
```

### 10.6 ConfigMap 配置管理

```yaml
# helm/go-on-swarm/templates/configmap.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: {{ include "swarm.fullname" . }}-config
data:
  go-on.yaml: |
    distributed:
      cluster_name: {{ .Values.global.clusterName | default "go-on-swarm" }}

      # 心跳
      heartbeat_interval_secs: 5
      heartbeat_timeout_secs: 15

      # 任务
      task_retention_hours: 72
      max_task_retries: 3
      task_result_ttl_secs: 3600

      # 调度
      scheduler:
        strategy: "capability_first"  # capability_first | load_balance | affinity_first
        rebalance_interval_secs: 60
        enable_data_locality: true

      # Leader 选举
      election:
        enabled: true
        raft_port: 50052
        snapshot_interval: 10000

      # 安全
      tls:
        enabled: true
        ca_cert_path: "/etc/go-on/tls/ca.crt"
        server_cert_path: "/etc/go-on/tls/tls.crt"
        server_key_path: "/etc/go-on/tls/tls.key"

      # 可观测性
      observability:
        tracing:
          enabled: true
          endpoint: "http://tempo:4317"
          sampling_rate: 0.1  # 10% 采样
        metrics:
          prometheus_port: 9090
        logging:
          format: "json"
          level: "info"
```

### 10.7 环境和配置变量

| 环境变量 | 默认值 | 说明 |
|---------|--------|------|
| `GO_ON_NODE_ROLE` | `coordinator` | 节点角色：`coordinator` / `worker` |
| `GO_ON_NODE_ID` | 自动生成 | 节点唯一 ID（K8s 中可用 `metadata.name`） |
| `GO_ON_LISTEN_ADDR` | `0.0.0.0:50051` | gRPC 监听地址 |
| `GO_ON_HTTP_PORT` | `8090` | HTTP 端口（仅 Coordinator） |
| `GO_ON_ETCD_ENDPOINTS` | — | etcd 集群地址，逗号分隔 |
| `GO_ON_NATS_URLS` | — | NATS 集群地址，逗号分隔 |
| `GO_ON_POSTGRES_DSN` | — | PostgreSQL 连接串 |
| `GO_ON_REDIS_URL` | — | Redis/Sentinel 地址 |
| `GO_ON_NODE_CAPABILITIES` | — | Worker 能力标签，逗号分隔 |
| `GO_ON_SCHEDULER_STRATEGY` | `capability_first` | 调度策略 |
| `GO_ON_ELECTION_TIMEOUT_MS` | `3000` | Leader 选举超时 |
| `RUST_LOG` | `info` | 日志级别 |

### 10.8 扩缩容指南

#### 场景一：手动扩缩 Worker

```bash
# K8s 环境
kubectl scale deployment/go-on-swarm-worker --replicas=20

# docker-compose 环境（需手动修改 compose 文件后重新部署）
docker compose up -d --scale worker=10
```

#### 场景二：自动扩缩（HPA）

HPA 基于 CPU/内存利用率自动调整 Worker 数量：

| 指标 | 扩容阈值 | 缩容阈值 | 冷却时间 |
|------|---------|---------|---------|
| CPU 利用率 | > 70% | < 50% | 扩容 15s，缩容 300s |
| 内存利用率 | > 80% | < 60% | 同上 |

```bash
# 查看 HPA 状态
kubectl get hpa go-on-swarm-worker

# 手动设置扩缩范围
kubectl autoscale deployment go-on-swarm-worker \
    --min=3 --max=50 \
    --cpu-percent=70
```

#### 场景三：基于自定义指标的扩缩（KEDA）

```yaml
# KEDA ScaledObject — 基于 NATS 队列深度
apiVersion: keda.sh/v1alpha1
kind: ScaledObject
metadata:
  name: go-on-worker-keda
spec:
  scaleTargetRef:
    name: go-on-swarm-worker
  triggers:
  - type: nats-jetstream
    metadata:
      natsServerMonitoringEndpoint: "nats-0:8222"
      queueLength: "100"
      activationQueueLength: "10"
```

### 10.9 升级与回滚

#### 10.9.1 滚动升级

```bash
# Helm 升级
helm upgrade go-on-swarm ./helm/go-on-swarm \
    --set global.imageTag=1.1.0 \
    --values values-prod.yaml \
    --reuse-values

# 查看升级状态
kubectl rollout status deployment/go-on-swarm-worker
```

#### 10.9.2 版本兼容性

| 升级方向 | Model 版本 | etcd Schema | gRPC API |
|---------|-----------|------------|----------|
| 1.0.0 → 1.1.0 | 兼容 | 向前兼容 | 向后兼容 |
| 1.1.0 → 2.0.0 | 不兼容（需迁移） | 需迁移 | 不兼容 |

升级策略：
1. 先升级 Worker（所有版本兼容）
2. 再升级 Coordinator（支持旧 Worker 连接）
3. 最后升级基础设施（etcd、NATS、PostgreSQL）
4. 每个步骤间隔 5 分钟观察指标

#### 10.9.3 回滚

```bash
# Helm 回滚到上一版本
helm rollback go-on-swarm 1

# K8s 手动回滚
kubectl rollout undo deployment/go-on-swarm-worker
kubectl rollout undo deployment/go-on-swarm-coordinator
```

### 10.10 灾备方案

#### 10.10.1 数据备份

| 组件 | 备份方式 | 频率 | 保留期 |
|------|---------|------|--------|
| PostgreSQL | pg_dump + WAL 归档 | 每日全量 + 实时 WAL | 30 天 |
| etcd | etcd snapshot | 每小时 | 7 天 |
| NATS | JetStream 备份 | 每日 | 7 天 |
| 审计日志 | 导出到 S3 | 实时 | 1 年 |

```bash
# etcd 快照备份
ETCDCTL_API=3 etcdctl snapshot save /backup/etcd-snapshot.db \
    --endpoints=https://etcd-0:2379 \
    --cacert=/etc/etd/tls/ca.crt \
    --cert=/etc/etcd/tls/tls.crt \
    --key=/etc/etcd/tls/tls.key

# PostgreSQL 备份
pg_dump -h postgres -U goon -d goon \
    --format=custom \
    --file=/backup/postgres-$(date +%Y%m%d).dump
```

#### 10.10.2 跨可用区部署

```yaml
# K8s topology spread — 跨 AZ 分布
# 在 values.yaml 中配置
affinity:
  podTopologySpread:
  - maxSkew: 1
    topologyKey: "topology.kubernetes.io/zone"
    whenUnsatisfiable: ScheduleAnyway
    labelSelector:
      matchLabels:
        app: go-on-swarm
```

#### 10.10.3 故障恢复步骤

```text
场景 A: 单个 Worker 故障
  ├─ K8s 自动重启 Pod
  ├─ etcd 标记节点 offline
  ├─ Coordinator 将活跃任务重新入队
  └─ 新 Pod 启动后自动注册，接收新任务

场景 B: 单个 Coordinator 故障（HA 模式）
  ├─ Load Balancer 自动剔除故障节点
  ├─ Leader 重新选举
  ├─ etcd 数据不受影响
  └─ Coordinator Pod 重启后自动加入集群

场景 C: etcd 集群故障
  ├─ 多数节点存活 → 自动恢复
  ├─ 全部节点故障 → 从备份恢复 etcd snapshot
  └─ 恢复后所有 Coordinator/Worker 自动重新连接

场景 D: 整个集群故障
  ├─ 恢复 etcd（从 snapshot）
  ├─ 恢复 PostgreSQL（从 pg_dump）
  ├─ 启动 Coordinator → 自动初始化集群
  ├─ 启动 Worker → 自动注册
  └─ NATS JetStream 从备份恢复未完成任务
```

### 10.11 部署检查清单

#### 10.11.1 启动前

- [ ] etcd 集群已启动且健康（`etcdctl endpoint health`）
- [ ] NATS 集群已启动且 JetStream 可用
- [ ] PostgreSQL + Citus 已启动且 schema 已迁移
- [ ] Redis Sentinel 已启动
- [ ] TLS 证书已部署（CA / 服务端证书 / 客户端证书）
- [ ] 防火墙规则已配置（gRPC 50051 / HTTP 8090 / etcd 2379 / NATS 4222）
- [ ] Prometheus + Grafana 已部署
- [ ] Tempo + Loki 已部署

#### 10.11.2 启动中

```bash
# 1. 部署基础设施
helm install goon-infra ./helm/go-on-infra --values values-prod.yaml

# 2. 等待 etcd/NATS/PostgreSQL Ready
kubectl wait --for=condition=Ready pod -l app=etcd --timeout=120s
kubectl wait --for=condition=Ready pod -l app=nats --timeout=120s

# 3. 部署 Coordinator
helm install goon-coordinator ./helm/go-on-coordinator --values values-prod.yaml

# 4. 验证 Coordinator 健康
kubectl wait --for=condition=Ready pod -l component=coordinator --timeout=60s
curl https://api.go-on.ai/health

# 5. 部署 Worker
helm install goon-worker ./helm/go-on-worker --values values-prod.yaml

# 6. 验证 Worker 注册
kubectl logs -l component=coordinator --tail=20 | grep "registered"
curl https://api.go-on.ai/governance/status | jq '.cluster.nodes'

# 7. 部署 Ingress
helm install goon-ingress ./helm/go-on-ingress --values values-prod.yaml

# 8. 端到端测试
curl -X POST https://api.go-on.ai/chat/stream \
    -H "Content-Type: application/json" \
    -d '{"messages":[{"role":"user","content":"hello"}],"mode":"ask"}'
```

#### 10.11.3 启动后

- [ ] Grafana 仪表盘显示所有节点在线
- [ ] 发送测试请求确认端到端可用
- [ ] 验证 Trace 跨节点串联
- [ ] 验证日志聚合
- [ ] 验证 Leader 选举（停掉 Leader Coordinator）
- [ ] 验证 Worker 容错（停掉一个 Worker）
- [ ] 验证备份已配置
- [ ] 告警规则已配置

### 10.12 运维命令速查

```bash
# ── 节点管理 ──────────────────────────────────────────────────
# 查看集群节点
kubectl exec deploy/go-on-swarm-coordinator-0 -- go-on node list

# 查看节点详情
kubectl exec deploy/go-on-swarm-coordinator-0 -- go-on node info <node-id>

# 排空 Worker（不再分配新任务，等待当前任务完成）
kubectl exec deploy/go-on-swarm-coordinator-0 -- go-on node drain <node-id>

# 驱逐 Worker（立即迁移所有任务）
kubectl exec deploy/go-on-swarm-coordinator-0 -- go-on node evict <node-id>

# ── 任务管理 ──────────────────────────────────────────────────
# 查看活跃任务
kubectl exec deploy/go-on-swarm-coordinator-0 -- go-on task list --status running

# 查看等待队列
kubectl exec deploy/go-on-swarm-coordinator-0 -- go-on task list --status queued

# 取消任务
kubectl exec deploy/go-on-swarm-coordinator-0 -- go-on task cancel <task-id>

# ── Council 管理 ───────────────────────────────────────────────
# 查看 Council 成员
kubectl exec deploy/go-on-swarm-coordinator-0 -- go-on council members

# 查看活跃提案
kubectl exec deploy/go-on-swarm-coordinator-0 -- go-on council proposals --status active

# ── 配置管理 ──────────────────────────────────────────────────
# 查看当前配置
kubectl exec deploy/go-on-swarm-coordinator-0 -- go-on config show

# 在线更新配置（经 etcd 广播到所有节点）
kubectl exec deploy/go-on-swarm-coordinator-0 -- go-on config set scheduler.strategy load_balance
```

---

## 附录：与现有架构的关系

| Profile | 分布式支持 | 说明 |
|---------|-----------|------|
| `local` | ❌ 无 | 单机本地运行，不涉及分布式 |
| `simple-server` | ❌ 无 | 单机服务器，不支持 Worker Swarm |
| `multi-users-server` | 🟡 部分 | 已有 PostgreSQL/pgvector 和 mTLS，可扩展为 Coordinator |
| `full` | ❌ 无 | 同 local（SQLite） |

### 与现有蓝图的映射

| 现有蓝图 | 对应功能 | 实现状态 |
|---------|---------|---------|
| FUTURE5 §M1 Coordinatory Council | 分布式 Council + Leader 选举 | ❌ 未实现 |
| FUTURE5 §M2 Worker Swarm | 本文档全部内容 | ❌ 未实现 |
| FUTURE5 §M3 Task Contract v2 | `DistributedTask` + 幂等性 | ❌ 未实现 |
| FUTURE5 §M4 Consensus Engine | 分布式共识 + etcd Raft | ❌ 未实现 |
| FUTURE5 §M5 Brain Loop | 跨节点 Brain Loop 闭环 | ❌ 未实现 |
| FUTURE5 §M6 Node Reputation | 基于节点行为的信誉评分 | ❌ 未实现 |
| FUTURE5 §M7 Distributed Memory | pgvector 集群 | 🟡 部分（有 PostgreSQL 无分片） |
| FUTURE5 §M8 Federated RL | `GrpcFederatedTransport` | 🟡 桩代码 |
| BLUE52 §GAP-B52-06 | gRPC 联邦传输 | 🟡 桩代码 |
| BLUE52 §GAP-B52-08 | 节点发现 + Heartbeat | ✅ 代码可用 |

---

> **文档信息**
> - 创建日期: 2026-06-29
> - 版本: 1.0
> - 状态: 蓝图/未实现
> - 关联 issue: Worker Swarm (FUTURE5 M2), Consensus Engine (FUTURE5 M4)
> - 当前架构限制: 单机多进程模式不支持多节点分布式，需要此方案中描述的全部基础设施
