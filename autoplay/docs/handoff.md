# AutoPlay v3 交接文档

**生成时间**：2026-11-08
**分支**：`feature/autoplay-v3` @ `5163516c3`
**工作树**：`E:\Projects\vera20k-autoplay-v3`，干净，无未提交改动
**测试**：`vera20k-autoplay` 54 通过；`vera20k-core` 命令路径测试 2 通过

---

## 一、这个分支在做什么

以 `upstream/main` @ `535469ca0` 为起点，从零实现一个**自动玩家（AutoPlay）**，目标是补齐 `G:\Documents\autoplay-v2.txt` 与本项目 `autoplay/docs/plan.md` 里规划的特性。

**v2 已完整保留，未删除、未合并。**
- v2 工作树：`E:\Projects\vera20k-autoplay\`，分支 `feature/autoplay-v2` @ `dacfea1c6`
- v3 与 v2 无代码继承关系：`upstream/main` 上没有 v2 的 356 个提交

**为什么不继承 v2**：v2 尝试过的七项改动中有三项对账本数字完全没有影响。这证明**调参不是杠杆**，需要的是可观测、可解释的决策层。v3 从干净起点重建，避免「改写替换」式的架构债（架构书 §20.14）。

---

## 二、已完成的工作（按提交顺序）

| 提交 | 内容 |
|---|---|
| `2467b8400` | 队伍离开数组的原因可观测（A4） |
| `32294cb6d` | 队伍生命周期测试锁定到单一 Team ID（A1-a） |
| `91cd548b7` | 建队门控放宽到独立 `autoplay` feature（A2） |
| `08c64a93c` | B1 可达性探测：能触及什么、卡在哪 |
| `4a845caa4` | B1 纵向闭环打通（所有权是阻塞原因） |
| `e94db863a` | 命令路径固化为回归测试（2 个） |
| `595db4f71` | 新建 `vera20k-autoplay` crate：control / arbiter / executor |
| `47bd54211` | 观察层：命令是否真的产生效果 |
| `54afda5e6` | World Model + Diagnostics（B2） |
| `28de2ccdc` | 归属判定 `eligibility` + 架构书 §18.1 五条 |
| `875e0dcab` | 本交接文档初版 |
| `1ab620f0b` | 重复抑制 `dispatch`（B3） |
| `d7dec87f1` | 失败重规划 `replan`（B3） |
| `ea10567f0` | 命令来源仲裁 `claims`（B3） |
| `5163516c3` | `claims` 接入 `Arbiter::admit`（B3 收尾） |

### 阶段 A：引擎侧前置改造

**A4 — 队伍离开原因**（`2467b8400`）
`team_script_vm` 里 `destroy_team` 是唯一移除路径，但有七个调用点、五种成因。原先从外部看只是「id 消失了」，无法区分「脚本跑完」与「为腾队伍上限被淘汰」。
现在 `destroy_team` 记录 `TeamEndReason`，五类：`ScriptCompleted` / `Dissolved` / `CulledForTeamCap` / `ReinforcementEmpty` / `DestroyedByAction`。

实测（Arena.mmx，seed 0x5EED，1 电脑，30000 tick）四个离开事件：2 个 `Dissolved`、2 个 `ScriptCompleted`。
**关键发现**：某支队伍 cursor 停在 1、只剩 1 个成员，旧读法会认为它「原因不明消失」，实际它**跑完了动作表**。

**A1-a — 生命周期测试**（`32294cb6d`）
断言恰好一条离开记录、同一个 team id、原因是 `ScriptCompleted` 而非 `Dissolved`、标志位已设且两个成员仍在队。

写测试过程中发现两个引擎真实行为，已写进注释：
1. 已成型队伍清空成员**不会**产生 `Dissolved` —— 招募动作仍完成，脚本跑完，以 `ScriptCompleted` 离开
2. 移除成员不等于杀死它 —— 活着的 E1 在招募范围内会被重新招募（实测离开时仍带 2 个成员）

**A2 — 建队门控放宽**（`91cd548b7`）
`create_team_from_type` 与 `TeamScriptMember` 原先只在 `test-support` 下可达，意味着发布构建无法建队。
新增独立 `autoplay` feature（与 `telemetry` 分开：它改变游戏而非观测）。

**放宽时发现真实缺陷**：三个查找全是 `expect()`，外部传入未注册的 TeamType 会**让仿真 panic**。改成 `?`，返回 `Option<u64>`。

### 阶段 B1：纵向闭环

**可达性探测**（`08c64a93c`、`4a845caa4`）
计划要求七步链路：读单位状态 → 查控制权 → 生成意图 → 准入 → 提交 → 读新位置 → 确认效果。

探测分两轮才打通，找到三个事实：

| 事实 | 说明 |
|---|---|
| **所有权是阻塞原因** | 命令命名非本方单位时，队列准入、派发静默丢弃、无任何错误 |
| `HeadlessScenario::tick()` 传空命令 | 它硬编码 `advance_frame(&[])`，`issue_local_command` 写入的 FIFO 无人读取 |
| `execute_tick` 与 `tick+1` 比较 | 用当前 tick 会早一帧，命令永不到期 |

修正后：`unit 1143` 从 (84,30) 收到命令，50 tick 内到达 (85,30)。

**命令路径回归测试**（`e94db863a`）
两个测试固化上面第一、三条语义：己方坦克会动；敌方坦克的命令被准入但位置不变。

### 阶段 B1 核心：`vera20k-autoplay` crate

**为什么独立 crate**：从引擎内部可达的 API 无法通过编译，编译器强制边界。这是让 AutoPlay 不能 fork 引擎行为的唯一机械保证。

| 模块 | 职责 | 关键设计 |
|---|---|---|
| `control.rs` | ControlState 三开关 + 对象级判定 | 全局开关不重写子系统标志，关掉再开恢复原状 |
| `arbiter.rs` | 唯一准入入口 | 返回具名拒绝或已验证命令；`execute_tick` 由它设定 |
| `executor.rs` | 提交并报告帧是否提交 | 有 idle 变体，避免每个调用方各自发明「等待观察」循环 |
| `dispatch.rs` | 重复抑制 | 三条件同时成立才抑制；失败记录会放行重试 |
| `replan.rs` | 失败后决策 | 五种结论；`Unobservable` 什么都不改 |
| `claims.rs` | 来源优先级 | 同来源可自我覆盖；重试不爬升层级 |
| `observe.rs` | 命令是否产生预期效果 | 四种结果：`Moved`/`Stationary`/`Gone`/`Unobservable` |

`Stationary` 是最重要的一种：命令被准入、对象存活、什么都没发生。这正是所有权问题掩盖失败时的形态。

### 阶段 B2：世界模型与诊断

**`world.rs` — `Snapshot`**（`54afda5e6`）
一次读取，之后是不可变值：不持有引擎引用、无内部可变性、不会漂移。
落实评审规则：**永不保留引擎已拥有的东西的第二份副本**，同时保留事后可解释能力。

最重要的区分：`enemy()` 与 `enemy_visible()`。
引擎知道所有对象，玩家只知道看过的。**瞄准 `enemy()` 是作弊而非玩游戏。**
可见性走 `Simulation::fog().by_owner()`，一次取平面，并按房屋查；没有平面的房屋等于什么都没揭示（真实状态，非错误）。
读取本身不做过滤：悄悄丢弃对象会让「没有敌方建筑」和「我没看」无法区分。

**`trace.rs` — `Trace`**
记录决策的**全部候选、赢家、理由**。候选是赢家推不出来的部分，理由是判断而非分数复述。
直接针对本项目历史：v2 六个轮回调数字纹丝不动，因为决策是对一个没人打印的列表做 fold。
有一测试专门固定：**保留一个打印分数高于赢家的落选者**，让「排名不是 argmax」可见。

上限 512 且**报告丢弃了多少**；按子系统计数是总量而非当前长度。

### 阶段 B3：交接与归属

**`handover.rs`**（`bc007694a`）
读引擎自己的任务状态 `current`/`queued`/`suspended`，不维护意图影子副本。
读引擎时发现：`mission_host_promote` 只提升**已排队**的任务，从不写队列，所以提升路径不可能覆盖任何东西 —— 竞争写入者的痕迹表现为**排队中的任务**。

五条测试覆盖评审 §5.5 的六种情形，模块文档写明了每条能确立的部分与不能确立的部分。

**`eligibility.rs`**（`28de2ccdc`）
`arbiter` 问「我可以吗」，`eligibility` 问「这本来就归我管吗」。
四种具名拒绝：`Gone` / `HouseDefeated` / `HouseLeft` / `NotThisHouse`。

**每次调用重新读世界，不做缓存**：占领、心灵控制、被击败、被摧毁都在对局中发生。
测试专门验证：对象换手后答案随之改变 —— 若是缓存会继续回答「可以」。

架构书 §18.1 五条对应五个测试。**第五条「编队单位不托管」明确记录为无法从仿真层回答**：选中状态在 app 层（`MatchInputState.selection_order`），从不进入 `Simulation`。用纯仿真场景断言会**因为错误的原因通过**（空真）。计划的结构性答案是「一个房屋、一个控制器」。

---

## 三、关键架构决策与理由

### 1. 独立 crate 而非子模块
编译器强制边界，AutoPlay 无法触碰 `pub(crate)` 内部。

### 2. 一个房屋一个控制器
这使「玩家与 AutoPlay 共享单位」的问题目前不成立，也是 §18.1 第五条无法测试的根因。

### 3. 不缓存，每次重读
`ControlState` 与 `eligibility` 都按帧重读。评审强调「创建时合法不代表之后合法」。

### 4. 拒绝必须具名
「不是我管的」与「不被允许」需要不同响应。合并成布尔会让规划器对败局反复重试。

### 5. 观察与执行分开
`executor` 说帧提交了，`observe` 说世界变了。两者是不同的判断，中间正是失败被掩盖的地方。

---

## 四、当前能做什么、不能做什么

### 能
- 判定某单位此刻是否可被本 AutoPlay 驱动（含击败、离场、异主、销毁）
- 提交一条命令并确认帧是否提交
- 观察命令是否真的产生效果（四种具名结果）
- 取世界快照，区分「引擎知道」与「玩家可见」
- 记录带全部候选的决策追踪
- 判断某单位当前/排队/挂起的任务
- 抑制重复命令，同时不阻止失败后的重试
- 失败后按结果决定重试/放弃/等待/不决策
- 按来源优先级让步：玩家 > 脚本 > AutoPlay

### 不能
- 驱动原生队伍 AI 打完整一局并实际争抢单位（需要战斗场景 fixture）
- 回答玩家是否选中某单位（app 层状态，仿真层不可见）
- 战略规划、任务管理、小队协同（计划明确不做）
- **阻止原生 AI 或触发器写同一单位**——本 crate 看不到它们的命令，
  只能在自己要写时让步（见 `claims.rs` 模块文档）

---

## 五、下一步

### 优先级一：完整对局测试
现有 54 个测试都是**单元级**：构造一个场景，验证一个行为。
唯一能证明整条链路在真实比赛里成立的证据，是让原生队伍 AI 与 AutoPlay
在同一局里跑起来，看交接是否真的按规则发生。

需要的东西：用 `battle_session` + `MatchScenario::setup` 构造的战斗场景，
即 `sim_bench_match` 已经在用的那套。B1 探测已经验证过地形与房屋可用。

### 优先级二：计划 C 阶段
Observation 扩展：`current_ammo`、`reload_state`、`PassengerCargo`、
当前有效武器、当前目标、行动状态、装载/运输/部署/瘫痪。

计划这里有一条硬规则：**绝不重新实现引擎的弹药/装填/载员规则**，只读。

### 优先级三：计划 D 阶段
五个账本（生产、损失、战斗、收入、资产）做成事件驱动，且
**流量与存量分离**。六维向量，永不单一加权分数。

### 已知风险
- 计划 §4 列的验收**从不用「击败零售 AI」**衡量：7 电脑测试显示零售 AI 是不稳定基准。改为固定地图/种子/资源/阵营/房屋数，六个维度（经济、军事、领土、科技、目标、生存）横向比较 AutoPlay 版本自身。

---

## 六、复现命令

```powershell
# 测试
cd E:\Projects\vera20k-autoplay-v3
$env:RA2_DIR = "I:\SteamLibrary\steamapps\common\Command & Conquer Red Alert II"
python -m tools.cargo_run -- test -p vera20k-autoplay --lib
python -m tools.cargo_run -- test -p vera20k-core --lib autoplay_command_path

# B1 探测（需先构建）
python -m tools.cargo_run -- build --profile fast -p vera20k-core --bin autoplay-slice
$exe = "E:\Projects\vera20k-autoplay\target\owned-worktrees\072666c56a10730e\fast\autoplay-slice.exe"
& $exe "$env:RA2_DIR" Arena.mmx 600

# 队伍诊断
python -m tools.cargo_run -- build --profile fast -p vera20k-core --bin autoplay-teams --features telemetry
& "...\autoplay-teams.exe" "$env:RA2_DIR" Arena.mmx 30000 1
```

注意：`cargo_run` 只构建，运行需调用解析后的 `.exe`。二进制落在**主检出**的 `target/owned-worktrees/`，不在工作树内。

---

## 七、需要知道的踩坑

| 坑 | 应对 |
|---|---|
| `git commit -m` 带引号/括号失败 | 写消息到文件，用 `git commit -F` |
| `cargo_run -- run` 不支持 | 它只接受 build/test/check/clippy/rustc |
| `docs/*` 被 .gitignore | 放 `autoplay/docs/`（不忽略） |
| 命令走 `issue_local_command` 后无反应 | 用 `advance_frame(commands, ...)` 的参数 |
| 自己写的 `.py` 脚本被 PowerShell heredoc 吃掉 | 用 `write` 工具直接写文件 |
| `Select-String \| Select-Object -Skip -1` 报错 | 用 `Get-Content \| Select-Object -Skip N` |

---

## 八、参考文档

| 文件 | 内容 |
|---|---|
| `autoplay/docs/plan.md` | 393 行完整计划（第二版，评审后修订） |
| `autoplay/docs/phase0-audit.md` | 181 行 Phase 0 审计 |
| `autoplay/docs/team-controller-interface.md` | 102 行队伍控制器接口定义 |
| `src/sim/world/autoplay_command_path_tests.rs` | 命令路径的两个回归测试 |
| `autoplay/src/lib.rs` | 模块总览，含各处设计理由 |

---

## 九、与评审的关键结论

评审对计划的裁决：**总体通过**，四条必修已全部落实。

其核心架构裁决，本分支严格遵守：
- 不重写 `team_script_vm`
- 不建第二套 Team/Squad/Script 系统
- 暂不建完整 Task Manager，但不说永不
- 不做单一加权 AI 分数，用六维向量
- AutoPlay 用独立 feature，不在引擎内特殊处理
- 分工：AutoPlay 定「为什么/去哪/优先级/失败了怎么办」，原生管成员组织与脚本执行
