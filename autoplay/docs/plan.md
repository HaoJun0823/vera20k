# AutoPlay V3 计划书

**日期**：2026-11-06
**基线**：`upstream/main` @ `535469ca0`，分支 `feature/autoplay-v3`
**当前 HEAD**：`4756e111b`
**上游架构书**：`autoplay-v3.txt`（1249 行）

---

## 一、结论摘要

三段验证已完成，可以给出第一个有证据的架构判断：

| 判断 | 证据 |
|---|---|
| 零售队伍脚本系统**能承担**完整执行闭环 | 1 电脑 30000 tick：8 队伍，5 个推进 cursor，1 个 `succeeded=true`，4 个离开数组，0 refusal |
| 它**不能**被直接当作 AutoPlay 的任务系统 | 建队入口 `create_team_from_type` 要 `test-support` 门控；无权限检查；无目标切换、无失败恢复 |
| 观测能力曾经不足，现已补上 | 三处 `cfg(test)` 放宽到 `telemetry`，`cargo test` 两种 feature 状态均 175 通过 |

**最重要的是一个被推翻的推断**：我先前根据 12000 tick 短局判断「脚本几乎不推进，可能有问题」。加大到 30000 tick 后，同一个配置出现了 cursor 到 6、`succeeded=true`、随后离开数组的完整周期。**短窗口的静止不等于卡死。**

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

按上级第 4 条的顺序，调整为：

### 阶段 A：锁定已验证的行为（本周）

| 项 | 内容 |
|---|---|
| A1 | 把「1 电脑 30000 tick：≥1 队伍 `succeeded`、≥1 离开数组、0 refusal」写成回归测试 |
| A2 | 放宽 `create_team_from_type` 到新 feature（如 `autoplay`），不加权限逻辑 |
| A3 | 探针加 `--json` 输出，便于自动化对比 |

**验收**：`cargo test` 覆盖队伍生命周期；AutoPlay 侧能建出一个队伍并观察它执行。

### 阶段 B：AutoPlay 骨架（最小闭环）

按架构书 Phase 1，但切成三个可独立验收的增量：

| 增量 | 交付 | 验收 |
|---|---|---|
| B1 | Game Interface 封装 + Control State | 能读世界、能发一条移动命令，单位确实移动 |
| B2 | World Model + Diagnostics | 导出结构化世界快照与决策追踪 |
| B3 | Command Arbiter + Action Executor | 架构书 §18.1 五条基础测试通过 |

**不做的**：不建 Task Manager、不建 Squad 系统、不做小队协同。等 A 阶段证明原生队伍够用再说。

### 阶段 C：能力模型（等 B 完成）

| 项 | 内容 |
|---|---|
| C1 | 扩展 `Observation` 暴露动态 `current_ammo` / `reload_state` / `PassengerCargo` |
| C2 | Conditional Capability 数据模型（Capability / Condition / Runtime Availability 三层分离） |
| C3 | 按上级要求：能力不等于当前可用状态 |

### 阶段 D：账本与评估

| 项 | 内容 |
|---|---|
| D1 | 移植 v2 的 `damage_value` 到 v3 的 `house_tracking.rs` |
| D2 | 五个账本：Production / Loss / Combat / Income / Asset |
| D3 | 多维指标向量（Economic / Military / Territory / Technology / Objectives / Survival） |

**明确不做**：不把多维指标压成单一加权分数。

---

## 六、需要上级确认的四件事

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

### 6.4 Phase 2 的 Task Manager 到底要不要写

这是最关键的架构问题。

**支持不写**：零售已有 18369 行队伍系统，能完成组建→招募→执行→完成→解散的闭环。再写一套会有两套生命周期、两套成员管理。

**支持写**：零售队伍做不到架构书 §8 要求的这些——动态目标切换、失败后恢复、成员损失后重新编组、进展超时检测。

我的判断：**先不写。** 用阶段 B 的骨架 + 原生队伍跑通一个完整对局，看差什么再补。这符合上级「先验证能力边界，再决定补什么」的要求。

---

## 七、风险清单

| 风险 | 影响 | 缓解 |
|---|---|---|
| 阶段 B 的 Command Arbiter 与零售 AI 抢同一房屋 | 两套逻辑互相干扰 | Control State 明确某房屋由谁驱动 |
| `create_team_from_type` 放宽后误用 | AutoPlay 建队绕过玩家意图 | Command Arbiter 统一把关 |
| 七电脑下的零售 AI 行为不可靠 | 无法用「击败零售 AI」作为验收 | 验收改用可复现的多维指标，不用胜负 |
| v3 与 v2 长期分叉 | 无法回收 v2 的成果 | 已在审计中列出可摘取清单（`damage_value` 账本） |

---

## 八、这份计划不做什么

明确排除，避免范围蔓延：

1. **不重写 `team_script_vm`**（上级决定）
2. **不在 AutoPlay 里再写一套 Team / Squad / Script 执行系统**（上级明确反对）
3. **不把多维指标加权成单一 AI Score**（上级决定）
4. **不在引擎里加 AutoPlay 特判**（架构书 §20.12）
5. **不做完整战略行为**（上级：「本阶段优先建立接口和验证能力」）

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
