# TeamController 接口确认（实施顺序第 1 步）

写于 2026-11-06。对象：`upstream/main` @ `535469ca0` 的 `src/sim/team_script_vm.rs`（2532 行）。

对应上级决定的第 1 项：确认 `team_script_vm` 的公开 API 和 AutoPlay 可调用路径。

结论先说：**只读路径完全可用，写入路径不存在。**

---

## 一、可以调用的（正式构建可用）

`Simulation::team_script_vm() -> &TeamScriptVm` @ `world/mod.rs:3436`，`pub fn`。

`TeamScriptVm` 的 19 个 `pub fn` 里 16 个不受 `cfg(test)` 门控，只读查询与规则注册都在：

| 方法 | 行 | 用途 |
|---|---:|---|
| `teams_in_order()` | 907 | 按 `TeamClass::Array` 原始顺序遍历所有队伍 |
| `team(id)` | 1026 | 取单个队伍状态 |
| `register_script(TeamScriptDefinition)` | 797 | 注册脚本 |
| `register_task_force(TeamTaskForceDefinition)` | 804 | 注册 TaskForce |
| `register_team_type(TeamTypeDefinition)` | 811 | 注册 TeamType |
| `register_ai_trigger(TeamAiTriggerDefinition)` | 816 | 注册 AI 触发器 |

`TeamScriptState` 的读取面（`team_script_vm.rs:628-730`）：

| 方法 | 行 | 返回 |
|---|---:|---|
| `id()` | 628 | 队伍 id |
| `owner()` | 645 | 所属房屋 |
| `team_type_id()` | 649 | 来源 TeamType，`Option` |
| `script_id()` | 668 | 当前脚本 |
| `cursor()` | 677 | 脚本执行进度 |
| `advance_pending()` | 677 | 是否有待推进动作 |
| `members()` | 682 | 成员迭代器 |
| `member_count()` | 697 | 成员数 |
| `succeeded()` | 720 | 脚本是否成功完成 |
| `refusal()` | 725 | 失败原因 `Option<TeamScriptRefusal>` |
| `formed()` | 730 | 是否已完成编成 |

**这已经满足「读取队伍状态」的职责。**

---

## 二、不能调用的（致命缺口）

### 2.1 建队 API 是 test-only

| 方法 | 行 | 门控 |
|---|---:|---|
| `create_team(owner, script, members, frame)` | 1451 | `#[cfg(any(test, feature = "test-support"))]` |
| `create_team_from_type(owner, team_type, candidates, frame)` | 1487 | 同上 |

**正式构建里这两个方法不存在。**

### 2.2 没有可变访问器

```rust
// world/mod.rs:1483
pub(crate) team_script_vm: TeamScriptVm,

// world/mod.rs:3436
pub fn team_script_vm(&self) -> &TeamScriptVm {   // 只有 &，没有 &mut
    &self.team_script_vm
}
```

`register_*` 全是 `&mut self`，而 AutoPlay 从 `Observation` 或 `Simulation` 都只能拿到 `&`。

**结论：AutoPlay 现在连注册一个 TeamType 都做不到。**

---

## 三、零售队伍是自动创建的，且对所有房屋生效

`world/house_defeat.rs:288`，每个房屋每帧：

```rust
crate::sim::ai_team_creation::update_team_creation(self, rules, owner, frame_effects);
```

这条路对**所有房屋**执行，包括玩家。它读 `[AITriggerTypes]`、`RatioAITriggerTeam`（`+0x565C`）、`AITriggerTypeClass::Is_Eligible @ 0x0041E720`，按权重抽签建队。

`update_team_creation` 是 `pub(crate)`，AutoPlay 同样调不到——但它**已经在跑**。

**这意味着：玩家房屋现在就在被零售 AI 建队逻辑驱动。** 这可能就是 v2 里「军队行为难以解释」的一个隐藏原因，那时候没人知道底层还有这套东西。

---

## 四、给上级的确认结论与建议

### 4.1 决定第 1 条的「TeamController Adapter」需要引擎侧改动

上级要求「增加稳定的 TeamController 适配接口」。审计结果是：**这个接口现在不存在，必须新加**。

最小可行接口（建议）：

```rust
impl Simulation {
    /// AutoPlay 的建队入口。与零售 `update_team_creation` 互斥，
    /// 由 Control State 决定哪一方驱动某个房屋。
    pub fn create_team_for(
        &mut self,
        owner: InternedId,
        team_type_id: InternedId,
        candidates: &[TeamScriptMember],
    ) -> Option<u64>;

    /// 解散。零售脚本有 `Dispersal` 动作，但 AutoPlay 需要一个直接入口。
    pub fn disband_team(&mut self, team_id: u64) -> bool;
}
```

### 4.2 必须先回答一个问题：谁驱动玩家房屋

现在 `update_team_creation` 对玩家房屋也在跑。如果 AutoPlay 也建队，同一帧会有两套建队逻辑：

- 零售按 `[AITriggerTypes]` 权重抽签
- AutoPlay 按战略需求

**这两者会互相干扰。** 需要在 Control State 里加一个开关，明确某房屋的队伍由谁创建。

这是上级决定第 1 条第 6 点「必须验证招募、脚本执行、完成、解散和重新分配的完整流程」之前的前置问题。

### 4.3 第 2 步（验证原生队伍闭环）可以先做，不需要新接口

虽然建队要引擎改动，**验证零售队伍已经在闭环运行**是只读的，现在就能做：

1. 装载 Arena.mmx，加一个 AI 房屋
2. 跑若干帧，用 `teams_in_order()` 观察队伍出现、`formed()`、`cursor()` 推进、`succeeded()`、成员变化
3. 断言「招募 → 执行 → 完成/失败 → 解散」确实发生

这个验证不依赖任何新 API，且能回答「零售 AI 到底怎么用队伍」——这直接决定 AutoPlay 该在哪个层次介入。

---

## 五、我的建议顺序

上级的第 4 条实施顺序我建议微调：把「验证原生队伍闭环」提到「确认 API」之后立刻做，而不是等接口建好。

| 上级顺序 | 我的建议 | 理由 |
|---|---|---|
| 1. 确认 API | 1. 确认 API ✅ 本文档 | 已完成 |
| 2. 验证原生闭环 | 2. **验证原生闭环（只读，现在就能做）** | 先看清零售怎么用队伍，再决定 AutoPlay 接口形状 |
| 3. 扩展 Observation（Ammo/Passenger） | 3. 加 `create_team_for` / `disband_team` | 建队是 AutoPlay 的根本前提 |
| 4. Conditional Capability | 4. 扩展 Observation（Ammo/Passenger） | 保持不变 |
| 5. Economy Ledger | 5. Conditional Capability | 保持不变 |
| 6. 测试 | 6. Economy Ledger + 测试 | 保持不变 |

**理由**：接口形状应该由「零售队伍实际怎么工作」决定，而不是先设计再验证。第 2 步是只读的，没有风险，且能让接口设计有依据。
