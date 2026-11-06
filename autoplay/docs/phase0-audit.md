# AutoPlay V3 · Phase 0 审计

写于 2026-11-06。审计对象：`upstream/main` @ `535469ca0`（分支 `feature/autoplay-v3`）。

这份文档回答上级架构书 §19 Phase 0 要求的四件事，并且先纠正一个前提。

---

## 零、先纠正一个前提：v3 是全新的，不是迁移

审计开始前我以为要做「旧系统 → 新架构的迁移映射」。实际不是。

| 事实 | 数据 |
|---|---|
| `upstream/main` 有 `autoplay/` 目录吗 | **没有** |
| 有 `src/sim/control/observation.rs` 吗 | **没有** |
| 有 `src/rules/unit_role.rs` 吗 | **没有** |
| 有 `Outbox` / `JobKind` / `Observation` 类型吗 | **都没有** |
| 保留下来的是什么 | `house_tracking.rs`（1083 行，3 个 pub fn）、`headless_scenario.rs`（3598 行，8 个 pub fn）、`scenario_session.rs`（2340 行，71 个 pub fn） |

**结论：v2 的 356 个提交全部不在 v3 里。** v3 是干净起点，审计对象应当是「引擎给了什么接口」，而不是「旧代码怎么搬」。

这消掉了架构书里最重的一块风险（§20.14 禁止一次性重写并替换），因为**没有旧实现可替换**。

---

## 一、引擎现状：v3 给了什么

### 1.1 读取世界的接口（对应架构书 §3.1 查询接口）

| 能力 | 位置 | 状态 |
|---|---|---|
| 房屋枚举 | `Simulation::houses()` @ `world/mod.rs:3440` | 有 |
| 实体存储 | `Simulation::entities()` @ `world/mod.rs:4580` | 有 |
| 实体存储定义 | `entity_store.rs:52` | 有 |
| 场景装载（零售地图） | `HeadlessScenario::load(retail_dir, map, seed)` | 有 |
| 加人类房屋 | `HeadlessScenario::add_human_house(name, country)` | 有 |
| 单帧推进 | `HeadlessScenario::tick()` | 有 |
| 仿真引用 | `HeadlessScenario::sim()` | 有 |

**缺**：没有 v2 那种 `Observation` 投影层。v3 要求调用方直接读 `EntityStore` 和 `Houses`。

这是差异不是缺陷——但要意识到：**架构书 §4「World Model」在 v3 里必须由 AutoPlay 自己建，引擎不会给。**

### 1.2 发命令的接口（对应架构书 §3.2 命令接口）

| 能力 | 位置 |
|---|---|
| 取本帧已准入命令 | `Simulation::take_admitted_commands()` @ `world/mod.rs:490` |
| 命令记录编解码 | `command_queue/dispatch.rs` 的 `encode` / `write_to` / `decode` |
| 时间窗控制 | `dispatch.rs:148` `apply_timing_update`、`178` `set(start,end)` |
| 网络命令时序 | `scenario_session.rs:1266` `install_network_command_timing` |

**关键判断**：v3 的命令通路是 `CommandEnvelope` + 准入队列，**不是** v2 那种 `Outbox` → `issue`。架构书 §11「Action Executor」需要在这个通路上重建。

### 1.3 已经被引擎承认的子系统

`src/sim/` 下与 AI 相关且**已存在**的：

```
combat  movement  pathfinding  production  vision  team_script_vm
mission  docking  transport_unload  house_tracking  aircraft  arena_fixture
```

### 1.4 零售 AI 脚本系统已经存在，而且很大

这是审计里最重要的发现，架构书完全没提到。

`src/sim/team_script_vm/` **共 18369 行**，是零售 `[TeamTypes]` / `[ScriptTypes]` / `[AITriggerTypes]` 的完整执行器：

| 文件 | 行数 | 内容 |
|---|---:|---|
| `actions.rs` | 3240 | 脚本动作执行 |
| `reinforcements.rs` | 1055 | 增援逻辑 |
| `team_ai.rs` | 865 | 队伍决策 |
| `membership.rs` | 644 | 队伍编成 |
| `registry_install.rs` | 617 | 脚本注册 |
| `super_actions.rs` | 257 | 超级武器动作 |
| `follow_friendlies.rs` | 171 | 跟随友军 |
| `scout_houses.rs` | 131 | 侦察房屋 |
| `orders.rs` | 94 | 命令派发 |
| 测试 | ~11000 | 各动作的回归测试 |

`team_ai.rs` 已有这些函数：`team_ai()`、`team_recruit_short_entries()`、`team_member_refuses_cell()`、`team_under_strength_retreat()`、`guard_slower_weight()`。

**这意味着什么**

架构书把「Task Manager」（§8）和「Tactical Planner」（§9）当作要新建的东西。但零售本身已经有一套队伍级任务系统在跑——招募、撤退、增援、侦察，都有实现。

**已确认这条链是活的**：

```
master_frame.rs:352  FrameStep::TeamScript
      → run_team_ai_pass()          // LogicClass::AI @ 0x0055B502
          → team_ai()               // TeamClass::AI @ 0x006E9140
```

而 `ai_team_creation.rs` 有 `AITriggerTypeClass::Is_Eligible @ 0x0041E720`，以及 `RatioAITriggerTeam`（`+0x565C`）等零售键——**`[AITriggerTypes]` 的触发器系统也在**。

三个必须回答的问题：

1. **AutoPlay 的定位是什么？** 零售 AI 已经有一套完整驱动。AutoPlay 若从零建 Task Manager，等于在同一帧里跑两套互不知情的决策系统。需要明确：AutoPlay 是「替代 `[AITriggerTypes]`」，还是「在它之上加一层」？
2. **`team_ai` 进程内能不能被 AutoPlay 复用？** `run_team_ai_pass` 是 `pub(crate)`，在 `FrameStep` 里按帧推进。AutoPlay 如果要驱动自己的队伍，最省力的是构造 `TeamTypeDefinition` 走同一条路。
3. **`[AITriggerTypes]` 是不是架构书 §8 要的 Task 系统？** 它有触发条件、队伍、优先级、权重（`RatioAITriggerTeam`）——形状上非常接近。若答案是「是」，Phase 2 不应从零写。

**在这个问题有答案之前，不应动 Task 相关代码。** 这是审计里唯一的阻塞项。

---

## 二、v2 留下了什么（按「能否摘取」分类）

356 个提交里有价值的部分不多，因为文档判定的方向错误影响了大半。

### 2.1 可直接摘取（纯数据/逻辑修复，与架构无关）

| 提交 | 内容 | 为什么值得摘 |
|---|---|---|
| `d07f67691` | 伤害折算成钱的引擎账本（`house_tracking.rs` + `receiver_health.rs` + 2 测试） | 这是「单位有效性 = 金钱」的度量，架构书 §7.2 的边际价值计算需要它 |
| `dacfea1c6` | `unit_role.rs` 四缺陷修复 | 其中三条是纯 bug：`aim` 只算穿透率不算伤害、`punch` 重复计算、IFV 17 个乘客武器被当成自己的 |

**注意**：`d07f67691` 依赖 `house_tracking.rs`，而它在 v3 里存在——但 v3 的版本没有 `damage_value` 字段。摘取时要重新加。

### 2.2 可参考但需重写

| v2 内容 | 价值 | 重写理由 |
|---|---|---|
| `policy.rs` 的金钱账本（`built` / `lost` / `destroyed` / `exchange`） | 高：这是唯一客观的评估指标 | 属于 Diagnostics，应进新架构而不是塞在策略里 |
| `autoplay/src/bin/roles.rs`（48 行） | 高：角色表诊断工具 | 独立工具，几乎可直接用 |
| 侦察兵「保留单位、到达即弃目标」的发现 | 中：`max_seen` 从 ≤4 提到 12 | 逻辑可复用，实现要进 Task 系统 |

### 2.3 应废弃

| v2 内容 | 废弃理由 |
|---|---|
| 全部硬编码常数（`ARMY_TO_MARCH=30`、`ENGAGE_ODDS=3`、`WALL_BREAKER_DPS=100`、`DEFENCES_ENOUGH=4`、`MAIN_BATTLE_FLOOR`…） | 架构书 §20.6 明确禁止「用无限增加硬编码条件修复系统性问题」 |
| `role_score` 取最大分的机制 | 架构书 §7.2 要求边际收益递减；取最大值是 FV 泛滥的根因 |
| 7 维浮点 Role 模型 | 架构书 §5.1「不能继续使用单一 Role」 |
| 每 term 重发一次的 `Job` | 架构书 §8.2 要求任务有生命周期 |

---

## 三、v2 的失败清单（审计价值最高的部分）

这些是**用真实对局测出来的**，新架构不应重犯。

### 3.1 试过并回退的 7 个改动

| 改动 | 实测结果 |
|---|---|
| 威胁分布按造价加权 | 丢建造场 `GACNST`，`lost` +63% |
| 威胁分布计入建筑（仅可见敌人） | 账本完全不变 |
| `unit_role` per-role punch + concrete 单值 | 权重更准，`lost` +110% |
| 建筑离基地免 30 兵门槛 | 拆到 2 建筑，`lost` 89900 |
| 防御上限 8→4 | 省下的钱流向 FV 而非坦克，`lost` +79% |
| `role_score` 按价格折减 | 账本**零变化** |
| 侦察兵前沿过滤 | `seen` 掉回 0..1 |

**共同点：7 个里有 3 个改动后账本一字未变。** 说明那些参数不是杠杆——这正是架构书说「不要继续调单位分类器」的实证支持。

### 3.2 已确立的实测基线（Arena.mmx，24000 ticks，六种子）

v2 最好的配置 `42a0c28df`：

| spent | lost | destroyed | exchange | own |
|---|---|---|---|---|
| 224900 | 12900 | 42100 | **3.264x** | 63/69/63/64/67/59 |

**敌方建筑拆除数：0。** 六个种子一个都没拆掉。

v3 若要证明自己更好，需要击败这个基线——但更重要的是先能跑起来。

### 3.3 单位有效性实测（这是唯一值得继承的度量）

用「造成的伤害折算成钱 ÷ 建造花费」：

| 类型 | 赚 | 花 | 效率‰ |
|---|---|---|---|
| MTNK 灰熊 | 4221 | 4900 | **86** |
| GGI 重装大兵 | 242 | 2800 | 8 |
| E1 美国大兵 | 118 | 2000 | 6 |
| FV IFV | 196 | 3600 | **5** |

**整个军队的正效益全靠坦克一辆车撑着。**

### 3.4 数据陷阱（新架构必须处理）

| 陷阱 | 事实 |
|---|---|
| 飞机 `ammo=1` | `BEAG`/`ORCA` 只带一枚炸弹，`dps_permille` 算出 225‰/168‰ 是**投弹瞬间值**，不是持续输出。v2 的角色模型完全没读 `ammo` |
| IFV 的 17 个武器槽 | 槽 2 之后是乘客武器（`CRNuke` 含核弹），一次只能装一个 |
| 光棱坦克造不出来 | `PRISM` 部署前无武器，`is_army` 排除它；只有 `SREF`（部署后）有 `Comet`（射程 10 格、混凝土 30‰） |
| 彩蛋单位定标尺 | `ARND`（终结者，cost=10）、`CLNT`（cost=10）曾是全表反步兵标杆 |

---

## 四、接口面差距：架构书要的 vs v3 有的

| 架构书模块 | v3 是否提供基础 | 缺口 |
|---|---|---|
| §3 Game Interface | 部分（`houses()` / `entities()` / `take_admitted_commands()`） | 需要封装成稳定接口，不能直接读 `EntityStore` |
| §4 World Model | **无** | 全部自建 |
| §5 Capability Database | **无**（`unit_role.rs` 不在 v3） | 全部自建，且要按 §5.2 的多维结构而不是 7 维 Role |
| §6 Strategic Planner | **无** | 全部自建 |
| §7 Economy/Production | **无** | 全部自建 |
| §8 Task Manager | **无** | 全部自建；`team_script_vm` 可能是基础但需先研究 |
| §9 Tactical Planner | **无** | 全部自建 |
| §10 Navigation | 部分（`pathfinding` 子系统已存在） | 需要封装 |
| §11 Action Executor | 部分（`command_queue` + `CommandEnvelope`） | 需要重建验证与节流 |
| §12 Command Arbiter | **无** | 全部自建 |
| §14 Decision Memory | **无** | 全部自建 |
| §17 Diagnostics | **无** | 全部自建 |

**12 个模块，v3 只有 3 个有部分基础。**

---

## 五、Phase 1 的切入点建议

架构书 Phase 1 列了 8 个模块：AutoPlay Context、Game Interface、World Observer、World Model、Control State、Command Arbiter、Action Executor、Diagnostics。

一次做 8 个违反 §20.15「每个阶段必须有可独立验收的结果」。建议切成三个可验收的增量：

### 增量 1：能读能写的最小闭环

**交付**：一个 `autoplay` crate（新），能
1. 装载 Arena.mmx，加一个人类房屋
2. 读到自己和敌方的全部实体（复用 `EntityStore`）
3. 给一个单位发一条移动命令，确认它动了

**验收**：一个集成测试，跑 100 tick，断言某个单位位置变了。

**为什么先做这个**：架构书 §3 说 Game Interface 是「整个系统的基础」。目前 `take_admitted_commands` 的用法没有先例，需要先摸通。

### 增量 2：World Model + Diagnostics

**交付**：统一的实体视图 + 决策追踪日志。

**验收**：能导出「当前我方 N 个单位、敌方 M 个、各在何处」的结构化 dump。

### 增量 3：Command Arbiter + Action Executor

**交付**：命令的验证、节流、状态机（`Requested → Validated → Issued → Accepted → Executing → Completed`）。

**验收**：架构书 §18.1 的五条基础测试全部通过。

**Task 系统推迟到 Phase 2**——它依赖 World Model 和 Command Arbiter 都稳定。

---

## 六、需要上级确认的三个判断

**1. `team_script_vm` 怎么定位？** 零售的 AI 脚本执行器已经存在。是把它作为「任务系统」的实现基础，还是完全绕开另建？这决定 Phase 2 的形状，需要先研究再定。

**2. `ammo` 与 `Passengers` 的语义边界。** 飞机弹药、IFV 乘客武器这两类「条件性火力」在能力模型里怎么表达？架构书 §5.2 的 `weapon.ammunition` 字段有了名字但没有语义定义。

**3. 度量标准是否沿用 v2 的金钱账本？** 我建议沿用——它是 v2 唯一客观、可复现的指标（`exchange` 3.264x、`earned_per_credit`）。但它依赖引擎侧的 `damage_value` 计数，需要重新加进 v3 的 `house_tracking.rs`。

---

## 附：本审计的事实来源

| 结论 | 验证方式 |
|---|---|
| v3 无 autoplay 代码 | `Get-ChildItem autoplay` 不存在；`Outbox`/`JobKind` 全文 0 处 |
| 356 个 v2 提交不在 v3 | `git rev-list --count upstream/main..HEAD`（在 v2 上）= 356 |
| v3 基线可编译 | `cargo build --profile fast -p vera20k-core --lib` → `exit=0` |
| 接口位置 | 直接读 `world/mod.rs` / `headless_scenario.rs` / `scenario_session.rs` 的 `pub fn` |
| v2 失败数据 | `autoplay/docs/session-close.md` 与 `round-thirty-three.md` 的实测表 |
