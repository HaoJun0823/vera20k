# AutoPlay V3 计划书

**日期**：2026-11-07（第二版，吸收评审意见）
**基线**：`upstream/main` @ `535469ca0`，分支 `feature/autoplay-v3`
**当前 HEAD**：`d36241d9a`
**上游架构书**：`autoplay-v3.txt`（1249 行）

---

## 零、第二版修订说明

第一版有三处表述过度，一处执行顺序矛盾，另有遗漏。逐条修正：

| 编号 | 问题 | 处理 |
|---|---|---|
| 1 | 「能承担完整执行闭环」扩大了证据范围 | 改为分层表述，见 §1.1 |
| 2 | `succeeded` 语义未核实 | **已查源码**：它是脚本动作 49 的标记，与战术目标无关，见 §1.2 |
| 3 | 回归测试按统计量而非按 Team ID | 改为三层测试，见 §5.1 |
| 4 | B1 要发命令但 Arbiter 在 B3 | B1 就地建最小命令链，见 §5.2 |
| 5 | Control State 权限边界不够硬 | 改为「一切状态变更经统一检查」，见 §5.3 |
| 6 | 缺原生 AI 与 AutoPlay 共存的测试 | 新增六条交接测试，见 §5.4 |
| 7 | 账本可能重复记账 | 改为事件驱动 + Flow/Stock 分离，见 §5.6 |

---

## 一、结论摘要

三段验证已完成，可以给出第一个有证据的架构判断：

| 判断 | 证据 |
|---|---|
| 零售队伍系统**存在一条完整的原生生命周期路径** | 1 电脑 30000 tick：8 队伍，5 个推进 cursor，1 个跑到 action 49，4 个离开数组，0 refusal |
| 它**不能**被直接当作 AutoPlay 的任务系统 | 建队入口要 `test-support` 门控；无权限检查；无目标切换、无失败恢复、无重新编组 |
| 观测能力曾经不足，现已补上 | 三处 `cfg(test)` 放宽到 `telemetry`，`cargo test` 两种 feature 状态均 175 通过 |

**最重要的是一处被推翻的推断**：我先前根据 12000 tick 短局判断「脚本几乎不推进，可能有问题」。加大到 30000 tick 后，同一个配置出现了 cursor 到 6、`succeeded=true`、随后离开数组的周期。**短窗口的静止不等于卡死。**

### 1.1 证据只能支持到这一层

评审指出第一版扩大了结论。分三层看，证据只到第一层：

| 层次 | 当前证据 | 判断 |
|---|---|---|
| **VM 生命周期** | 创建、招募、脚本推进、action 49、离开数组 | **有实例支持** |
| **游戏任务执行** | 队伍是否完成预定战术目标 | **证据不足** |
| **战略任务管理** | 目标切换、失败恢复、重新编组、超时处理 | **完全未验证** |

所以准确表述是：

> 已验证零售队伍系统至少存在一条完整的原生队伍生命周期路径：创建、招募、脚本推进、成功状态和离开队伍数组。它可以作为 AutoPlay 的队伍执行后端进行进一步验证，但尚未证明它能独立承担战略任务管理。

### 1.2 `succeeded` 的确切语义（评审要求核实）

评审要求确认这个字段是不是「战术任务成功」。**已查源码，不是。**

```rust
// src/sim/team_script_vm.rs
/// `+0x84`: the team succeeded (script action 49). Its destruction then
/// counts as its AI triggers' success.
succeeded: bool,
```

```rust
// src/sim/team_script_vm/actions.rs:900
// `0x006E94BA`, and `0x006E9CE1` which also records success
// (`+0x84`), counted when the team is destroyed.
24 | 49 => {
    if let Some(team) = self.team_script_vm.teams.get_mut(&team_id) {
        team.advance_pending = true;
        team.succeeded |= action.action_id == 49;
    }
}
```

**结论**：`succeeded` 只有一个含义——脚本执行到了动作 49（`Script_Success` 标记）。它不检查任何战术条件，唯一后果是「该队伍被摧毁时记为其 AI 触发器的一次成功」（喂给 `AITriggerTypeClass::RegisterSuccess @ 0x0041FD60` 的权重统计）。

**它与「队伍打赢了」「完成了战略目标」无关。** 计划书第一版把它当成闭环证据，是过度解读。

### 1.3 离开数组也要分辨原因

4 个队伍离开数组，第一版直接当作「正常解散」。**这不对。**

`TeamScriptState::dissolves()` 是私有方法，它的条件（成员不足、超时、脚本结束）没有暴露。当前能观察到的只有「消失了」，无法区分：

- 脚本跑完后的正常清理
- 成员条件失效（`SUICIDE-SCRIPT` 那类）
- 未填满队伍的超时解散（`created_frame` + 未填满时钟）
- 被战斗摧毁

**这一项列入 A 阶段待办**（见 §5.1 的三层测试），不能靠现有观测下结论。

---

## 二、已完成的审计（Phase 0）

### 2.1 v3 是干净起点

| 事实 | 数据 |
|---|---|
| `upstream/main` 有 `autoplay/` | **没有** |
| 有 `observation.rs` / `unit_role.rs` | **没有** |
| 有 `Outbox` / `JobKind` | **没有** |
| v2 的 356 个提交 | 一个都不在 v3 |

架构书 §20.14 禁止「一次性重写并替换旧实现」的风险**不存在**——没有旧实现可替换。

### 2.2 引擎给 AI 的接口面

| 能力 | 位置 | 状态 |
|---|---|---|
| 房屋枚举 | `Simulation::houses()` @ `world/mod.rs:3440` | 可用 |
| 实体存储 | `Simulation::entities()` @ `world/mod.rs:4580` | 可用 |
| 命令准入 | `Simulation::take_admitted_commands()` @ `world/mod.rs:490` | 可用 |
| 场景装载 | `HeadlessScenario::load_session()` | 可用 |
| 队伍 VM（只读） | `Simulation::team_script_vm()` @ `world/mod.rs:3436` | 可用 |

基线编译 `exit=0`。

### 2.3 架构书的 12 个模块，实际缺口

| 模块 | v3 现状 |
|---|---|
| Game Interface | 部分（需封装） |
| World Model | **无** |
| Capability Database | **无** |
| Strategic Planner | **无** |
| Economy Planner | **无** |
| Task Manager | **无**，但零售有 18369 行同类实现 |
| Tactical Planner | **无** |
| Navigation | 部分（`pathfinding` 存在） |
| Action Executor | 部分（`command_queue`） |
| Command Arbiter | **无** |
| Decision Memory | **无** |
| Diagnostics | **无** |

**7 个完全缺失。**

### 2.4 零售 AI 链条是活的

```
master_frame.rs:352  FrameStep::TeamScript
    → run_team_ai_pass()      LogicClass::AI @ 0x0055B502
        → team_ai()           TeamClass::AI @ 0x006E9140
```

`src/sim/team_script_vm/` **18369 行**：`actions.rs` 3240、`reinforcements.rs` 1055、`team_ai.rs` 865、`membership.rs` 644，另有约 11000 行测试。

`ai_team_creation.rs` 有 `AITriggerTypeClass::Is_Eligible @ 0x0041E720`、`RatioAITriggerTeam`（`+0x565C`）。

**并且这条链对玩家房屋也在跑**（`world/house_defeat.rs:288`，每帧每房屋）。

---

## 三、两组验证的完整数据

### 3.1 第一组：短局诊断（Arena.mmx，seed 0x5EED，1 电脑，30000 tick）

| 队伍 | 出现 | 最大 cursor | 成员 | 结果 |
|---|---:|---:|---:|---|
| 0 | 2500 | **6** | 8 | 静止 20500 tick |
| 1 | 4500 | 0 | 1 | 6500 离开 |
| 2 | 6500 | 0 | **17** | 8500 离开 |
| 3 | 8500 | 0 | 1 | 11000 离开 |
| 4 | 10500 | **6** | 5 | **`succeeded=true`**，15000 离开 |
| 5 | 12500 | 2 | 5 | 静止 14500 tick |
| 6 | 14500 | 1 | 1 | 静止 14500 tick |
| 7 | 14500 | 1 | 1 | 静止 14500 tick |

**汇总：8 个队伍，8 个来自 TeamType，4 个招募到 ≥4 人，5 个推进过 cursor，1 个脚本完成，4 个离开数组，0 refusal。**

**这构成完整生命周期**：组建 → 招募 → 执行 → 完成 → 解散。

### 3.2 第二组：七电脑长局（同图同种子，30000 tick）

```
distinct teams      1
recruited to >= 4   1
advanced past 0     0
script completed    0
left the array      0

team 0: 出现 2500，成员 4，cursor 全程 -1，静止 27500 tick
```

**同一配置下两组结果完全相反。** 单房屋数据不能代表真实对局。

### 3.3 七电脑异常的可能原因（未定论）

| 假设 | 待验证 |
|---|---|
| 多房屋稀释 `[AITriggerTypes]` 抽签权重 | 需要打印 `RatioAITriggerTeam` 与实际抽签 |
| 七 AI 混战消耗，达不到建队门槛 | 需要对照存活军队数 |
| 建队条件依赖的某资源在多房屋下不足 | 需要逐条件检查 |

**这不是 AutoPlay 的阻塞项**——AutoPlay 会用 `create_team_from_type` 直接建队，不走抽签。

---

## 四、已完成的改动

| 提交 | 内容 |
|---|---|
| `f3d48afb6` | Phase 0 审计文档 |
| `09b01173f` | TeamController 接口确认文档 |
| `6e020712f` | `autoplay-teams` 只读探针 |
| `4756e111b` | 三处 `cfg(test)` → `cfg(any(test, feature="telemetry"))`，探针升级为结构化观测 |

### 4.1 门控放宽的检查结果

上级要求核实两点，均已通过：

| 检查点 | 结论 |
|---|---|
| `TeamScriptRefusal` 能否在 telemetry 构建中访问 | **能**。它是 `pub enum`，无独立门控 |
| 三个方法是否只读 | **是**。全部 `&self` 直接返回字段 |

### 4.2 `create_team_from_type` 的审查（P1 要求）

| 检查点 | 结论 |
|---|---|
| 是否只负责创建、不伪造数据 | **是**。从已注册 `TeamType` 取 `task_force_id`，按 `TaskForce` 顺序接收候选项 |
| 正式运行能否正确处理归属/TeamType/TaskForce/招募/销毁 | **能**。走零售同一条路径（`TeamScriptState::new` + `member_team` 索引） |
| AutoPlay 是否有权限 | **它不检查**。权限必须由 Control State 在调用方把关 |

**结论：复用安全，但权限控制是 AutoPlay 自己的责任。**

---

## 五、下一步计划

### 阶段 A：锁定已验证的行为

评审指出第一版的 A1 有漏洞：三个统计量可以被不同队伍分别满足，不代表任何一个队伍走完了生命周期。改为**三层测试**。

#### 5.1 三层测试（替代原 A1）

**A1-a 受控确定性测试**（单元级，不依赖随机 AI）

用一个手工构造的最小场景走完整链条，按同一个 Team ID 断言：

1. 创建成功，拿到 Team ID
2. 同一 Team 招募到预期成员数
3. 同一 Team 的 cursor 至少推进一次
4. 同一 Team 到达 action 49
5. 同一 Team 从活动集合移除
6. 移除时机与第 4 步的因果关系明确

**要求**：确定性，不依赖 `[AITriggerTypes]` 抽签，跑 100 次结果一致。

**A1-b 固定种子集成测试**（验证真实触发条件）

固定地图、固定种子、固定房屋数，断言「至少有一个队伍走完 A1-a 的六步」。允许具体是哪个队伍变化，但**六步必须落在同一个 ID 上**。

**A1-c 长局稳定性测试**（`#[ignore]`，手动跑）

7 电脑、30000+ tick，只做统计观察，不断言。用于发现回归，不作为 CI 门禁。

#### 5.2 其余 A 阶段项

| 项 | 内容 | 备注 |
|---|---|---|
| A2 | 放宽 `create_team_from_type` 到独立 feature `autoplay` | 评审同意 |
| A3 | 探针加 `--json`，便于自动化对比 | |
| A4 | **查清「离开数组」的成因** | 评审要求，见 §1.3 |

A4 需要读 `dissolves()` 的私有条件，可能要把原因也暴露到 `telemetry`。这是新增的一步。

**验收**：A1-a 与 A1-b 通过；A4 能区分至少三种离开原因。

### 阶段 B：AutoPlay 骨架

评审指出 B1 有顺序矛盾：要发命令，但 Arbiter 在 B3。**改为 B1 就地建最小命令链**，避免两套执行机制。

#### 5.3 B1：最小纵向闭环（含最小仲裁）

评审给的链路，逐项实现：

```
读取单位状态
    ↓
检查 AutoPlay 控制权        ← 最小 Control State
    ↓
生成移动意图
    ↓
命令准入检查               ← 最小 Arbiter
    ↓
提交游戏命令
    ↓
读取单位新位置
    ↓
确认命令是否产生预期效果     ← 最小 Executor 验证
```

**不做**：多命令源冲突、玩家优先级、重复抑制、失败重规划——这些留给 B3，但**接口形状在 B1 就定死**，B3 只扩展不替换。

#### 5.4 Control State 的边界（评审第七条）

评审要求一条硬规则，采纳：

> **任何能够改变游戏状态的 AutoPlay 操作，都必须经过统一的控制权检查。**

范围明确包括：

| 类别 | 具体操作 |
|---|---|
| 单位命令 | 移动、攻击、停止、守卫 |
| 队伍操作 | `create_team_from_type`、招募成员、改脚本目标、释放成员 |
| 生产 | 生产单位、建造建筑、取消队列 |
| 能力 | 特殊能力、部署、展开 |

概念结构（字段名待匹配引擎类型）：

```rust
struct ControlState {
    global_enabled: bool,
    production_enabled: bool,
    combat_enabled: bool,
    ability_enabled: bool,
    house_id: HouseId,
}
```

**评审特别强调的一点，这里单独列出**：

> 创建时合法，不代表之后一直合法。

所以检查必须是**每帧实时**的，不是建队时查一次。要能响应：

- 玩家接管了队伍中的某些单位
- 单位被心灵控制
- 房屋被击败
- AutoPlay 被关闭

**这条直接对应第一版计划没写的一个漏洞**：AutoPlay 关闭后，之前创建的队伍 VM 仍在运行并继续控制单位。

#### 5.5 原生 AI 与 AutoPlay 的共存测试（评审第八条）

这是第一版完全没写的一块，评审指出它最容易出问题。采纳其六条测试：

| 场景 | 预期 |
|---|---|
| AutoPlay 关闭 | 原生 AI 正常运行 |
| AutoPlay 开启但未接管单位 | 原生 AI 正常运行 |
| AutoPlay 接管单位 | 原生 AI 不再覆盖其命令 |
| AutoPlay 释放单位 | 原生 AI 可以重新接管 |
| 玩家手动控制单位 | AutoPlay 不覆盖玩家命令 |
| AutoPlay 关闭时仍有旧队伍 | 旧队伍不再继续执行 AutoPlay 指令 |

**这一组比任何战术规划都优先。** 理由：`world/house_defeat.rs:288` 每帧对每个房屋跑 `update_team_creation`，原生 AI 与 AutoPlay 必然会碰面。

#### 5.6 B2 / B3

| 增量 | 交付 | 验收 |
|---|---|---|
| B2 | 完整 World Model + Diagnostics | 导出结构化世界快照与决策追踪 |
| B3 | 扩展 Arbiter：多命令源冲突、玩家优先级、重复抑制、失败重规划 | 架构书 §18.1 五条 + §5.5 六条全通过 |

**不做的**：不建 Task Manager、不建 Squad 系统、不做小队协同。

### 阶段 C：能力模型

评审第六条的方向采纳，并加一条硬约束：

> **AutoPlay 只观察引擎的动态状态，不自行模拟弹药、装填、乘客和武器切换规则。**

三层分离保持不变：

| 层 | 含义 | 例子 |
|---|---|---|
| Capability | 单位具备什么 | 有对空武器 |
| Condition | 什么条件下生效 | 需要特定乘员 / 必须展开 |
| Runtime Availability | 当前能否执行 | 弹药、装填、状态 |

`Observation` 待补的字段（按实际决策需求逐个加，不一次做完）：

- `current_ammo` / `reload_state`
- `PassengerCargo`
- 当前有效武器
- 当前目标
- 当前动作状态
- 是否正在装填 / 运输 / 卸载
- 是否展开
- 是否被控制 / 禁用 / 无法行动

### 阶段 D：账本与评估

评审第八条要求**事件驱动 + Flow/Stock 分离**，采纳。

| 账本 | Flow（一段时间的量） | Stock（当前存量） |
|---|---|---|
| Production | 生产了多少、花了多少 | 当前拥有 |
| Loss | 损失了多少 | — |
| Combat | 伤害、击杀、交换 | — |
| Income | 收入、支出 | 当前资金 |
| Asset | 所有权转移次数 | 当前资产与战略设施 |

**防重复记账**：评审给的两个例子直接写进设计——

| 事件 | 正确记法 | 错误记法 |
|---|---|---|
| 工程师占领敌方建筑 | 一次**所有权转移**，Asset 账本更新归属 | 不算我方新建，也不算敌方摧毁 |
| 单位被心灵控制 | 原始损失与控制权变化**分开记**；恢复控制不记为重新生产 | 不合并成一条 |

**数据来源**：以引擎事件为主，定期用世界快照对账。

**明确不做**：不把六维指标压成单一加权分数。

---

## 六、验收标准：不用胜负

评审第七节明确要求，采纳：

> 不要用击败零售 AI 作为 AutoPlay 的主要验收标准。

理由有实测支持：七电脑下零售 AI 30000 tick 只建了 1 个队伍且 cursor 从未启动。**它的行为不稳定，不能作绝对基线。**

改用固定条件的指标对比：

| 固定项 | 值 |
|---|---|
| 地图 | Arena.mmx |
| 种子 | 0x5EED（及其余五个） |
| 初始资源 | 启动参数固定 |
| 阵营 | 固定 |
| 房屋数 | 1 / 7 两档 |

对比的是**不同版本 AutoPlay 的自身表现**，六维分别记录：

| 维度 | 指标 |
|---|---|
| Economic | 收入、支出、净资产 |
| Military | 单位价值、交换比、损失 |
| Territory | 已揭示面积、控制点 |
| Technology | 科技层级、建筑完成度 |
| Objectives | 任务完成数（待 Task 系统落地后） |
| Survival | 存活时间、基地完好度 |

**不压成单一分数**（§五 阶段 D 已定）。

---

## 七、原生 AI 与 AutoPlay 的最终分工

评审最后一节给出的分工图，采纳并记录为架构约束：

```
AutoPlay Strategic Planner
        │  决定为什么行动、去哪里、优先级、失败后怎么办
        ▼
AutoPlay Objective / Intent
        │
        ▼
Native Team VM Adapter        ← 阶段 B 之后建
        │
        ▼
原生 Team / Script / Membership   ← 已有 18369 行，不重写
        │
        ▼
游戏单位执行
```

**职责边界**：

| 层 | 负责 | 不负责 |
|---|---|---|
| AutoPlay Strategic | 目标选择、优先级、失败策略 | 成员管理、脚本动作 |
| Adapter | 把 Intent 翻译成建队请求与队伍命令 | 战略判断、原生脚本执行 |
| 原生 Team VM | 成员招募、脚本动作、编队维护 | 战略目标 |

**这解决了第一版留下的最大悬念**：不需要二选一。AutoPlay 补的是「为什么」和「失败了怎么办」，原生补的是「怎么把队伍组织起来并执行动作」。

**暂不写完整 Task Manager 不等于永远不写。** 等到 §五 阶段 B 的骨架跑通一个完整对局，看 Intent → Adapter → 原生 VM 这条链缺什么，再决定 Task 层的形状。

---

## 八、需要上级确认的四件事

### 6.1 七电脑异常是否值得单独调查

七电脑下 30000 tick 只建 1 个队伍且未启动。这不阻塞 AutoPlay，但它是零售 AI 在真实对局下的实际表现。**要不要先查清，还是先推进 AutoPlay？**

我的建议：**先推进**。理由：AutoPlay 不依赖 `[AITriggerTypes]` 抽签。但这个现象应记录在案，因为它影响「零售 AI 作为对照基线」的可信度。

### 6.2 `create_team_from_type` 用哪个 feature

选项：

| 方案 | 优点 | 缺点 |
|---|---|---|
| 新 feature `autoplay` | 语义清晰 | 多一个 feature |
| 复用 `telemetry` | 不加 feature | 语义混淆（观测 vs 写入） |
| 复用 `test-support` | 零改动 | 生产构建要开测试 feature，风险高 |

我的建议：**新 feature `autoplay`**。理由：写入能力不应该藏在观测或测试 feature 里。

### 6.3 权限模型的责任边界

`create_team_from_type` 不检查权限。上级第 3.3 条要求「不能因为某个接口可见，就绕过玩家控制、阵营归属和全局 AutoPlay 开关」。

**问：这个检查应该加在引擎侧（`create_team_from_type` 内部），还是 AutoPlay 侧的 Command Arbiter？**

我的建议：**AutoPlay 侧**。理由：引擎不应知道 AutoPlay 的存在；而 Arbiter 本来就是干这个的。

### 8.4 Task Manager 的落地时机

评审同意「暂不写完整 Task Manager」，同时指出最终不能完全没有任务管理。**§七 已记录分工**，这里只留时机问题。

**待回答**：Intent → Adapter → 原生 VM 这条链跑通一个完整对局后，如果发现原生队伍无法表达某些战略意图（例如「这个队伍打完这个基地后改去打另一个」），那部分缺口是补在 Adapter 里，还是需要独立 Task 层？

我的建议：**先做 Adapter，把缺口具体化**。等能列出「原生表达不了的第 N 类意图」再决定形状，而不是现在设计一个可能用不上的 Task 层。

---

## 九、风险清单

| 风险 | 影响 | 缓解 |
|---|---|---|
| **原生 AI 与 AutoPlay 抢同一单位** | 单位反复改目标、命令刷新、无法开火 | §5.4 每帧控制权检查 + §5.5 六条交接测试 |
| **AutoPlay 关闭后旧队伍仍在跑** | 已关闭的 AI 继续控制单位 | 控制权检查必须每帧实时，非建队时一次 |
| 建队后状态变化（心灵控制、玩家接管、房屋被击败） | 创建时合法、之后非法 | 同上，归属与控制权实时复核 |
| `create_team_from_type` 放宽后误用 | 绕过玩家意图建队 | 封装在 AutoPlay 受控入口后，不暴露给其他模块 |
| 七电脑下的零售 AI 行为不可靠 | 无法用「击败零售 AI」作为验收 | §六 改用固定条件多维指标 |
| v3 与 v2 长期分叉 | 无法回收 v2 的成果 | 已在审计中列出可摘取清单（`damage_value` 账本） |
| `succeeded` 被误读为战术成功 | 统计口径错误 | §1.2 已核实并写入文档 |

---

## 十、这份计划不做什么

明确排除，避免范围蔓延：

1. **不重写 `team_script_vm`**（上级决定）
2. **不在 AutoPlay 里再写一套 Team / Squad / Script 执行系统**（上级明确反对）
3. **不把多维指标加权成单一 AI Score**（上级决定）
4. **不在引擎里加 AutoPlay 特判**（架构书 §20.12；评审同意，`create_team_from_type` 只放宽门控，不加权限逻辑）
5. **不做完整战略行为**（上级：「本阶段优先建立接口和验证能力」）
6. **暂不调查七电脑 AITrigger 异常**（评审同意，见 §三 3.3）
7. **不在 AutoPlay 里重复模拟弹药、装填、乘客和武器切换**（评审第六条）

---

## 附：如何复现本文档的所有数据

```bash
cd E:/Projects/vera20k-autoplay-v3

# 构建（门控放宽后需要 telemetry）
python -m tools.cargo_run -- build --profile fast -p vera20k-core \
    --bin autoplay-teams --features telemetry

# 第一组：短局诊断
<resolved>/fast/autoplay-teams.exe "<RA2_DIR>" Arena.mmx 30000 1

# 第二组：七电脑长局
<resolved>/fast/autoplay-teams.exe "<RA2_DIR>" Arena.mmx 30000 7

# 测试（两种 feature 状态都应是 175 passed）
python -m tools.cargo_run -- test -p vera20k-core --lib team_script_vm
python -m tools.cargo_run -- test -p vera20k-core --lib team_script_vm --features telemetry
```

`<resolved>` 由 `tools/cargo_run.py` 的 `resolve_binary` 决定，构建缓存属于主 checkout。
