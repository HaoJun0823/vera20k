# T01 单位意图追踪（L4）

| 项 | 值 |
|---|---|
| 状态 | 待办 |
| 层 | 4 单位意图与执行 |
| 依赖 | 无 |
| 负责人 | （谁领谁填） |
| 完成证据 | 提交号 + 实测数字（完成时填） |

## 1 要做什么

给每个受控单位留一份「我上次给它发了什么命令」的记录：同一条命令不重复发，
只有到期、或单位位移超过阈值、或目标变了才重发；微操命令之间互斥，不互相覆盖。
这是四层重构里 L4 那一层（当年 `tracked.rs` 做过、四层重构未完成），也是后面几张卡的地基。

## 2 游戏里能看到什么

单位不再因为每帧被重新下令而不开火；同一条命令不会刷屏；
被打断后能自己接上原来的活，而不是换个动作从头来。

## 3 合格线（可数）

- 基线：`6558ee2a7`，Arena.mmx seed 0x5EED 600 帧，`assignments=52 attacks=52 guards=0`、
  `active=13`、`ended={Reassess: 39}`。
- 同一单位、同一命令、条件未变时，窗口内**只发一次**；`assignments` 明显下降，
  `attacks` 仍 > 0（不是把攻击一起禁掉），`ended={Reassess: ...}` 仍出现（窗口语义保留）。
- 到期 / 位移超阈值 / 目标变化三种重发条件各有测试；微操互斥有测试。
- 全量 `vera20k-autoplay` 测试通过，数量不减少（当前 90）。

## 4 不许复发

- **P05** 高频发指令导致无法开火 / 无冷却开火 BUG：去重是本卡的直接目的。
- **P06** 飞机没装完弹就再次攻击、**P29** 飞机不装弹就投弹：重发前查 `availability` 的弹药/装填状态。
- **P28** 发了移动/攻击指令但无效、无动作：命令提交后用 `observe` 确认效果，无效才允许重发。
- **P15** 执行成功后发呆、**P07/P26** 在家挂机：窗口到期要重评估，不能因为去重就永远不再派活。
- **P01** 拆东墙补西墙：不许为了压 `assignments` 数字把任务结束机制删掉；
  去重与「任务能结束」必须同时成立。

## 5 依赖与前置事实

- `dispatch.rs` 已有 `Dispatcher`/`Suppression`，只做**单帧**重复抑制（三条件同时成立才抑制，
  失败记录会放行重试）；跨帧追踪不存在，两者不冲突，别删它。
- `idle.rs` 的 `REASSESS_FRAMES=150` 是任务窗口，窗口到期以 `TaskEnd::Reassess` 结束任务。
  **本卡不改这个语义**：窗口到期仍结束任务，但重发时若命令相同且条件未变则不发新命令。
- `frame.rs::plan_frame` 每帧最多产出一条已准入命令；`TaskBoard` 是 AutoPlay 自己的分配记录
  （引擎给每个新单位默认任务 `MissionId(5)`，所以「有没有活干」不能问引擎）。
- 单位运行时能力只读 `availability.rs`；**绝不重新实现弹药/装填/载员规则**。

## 6 验证命令

```powershell
cd E:\Projects\vera20k-autoplay-v3
$env:RA2_DIR = "I:\SteamLibrary\steamapps\common\Command & Conquer Red Alert II"
$env:VERA20K_REQUIRE_RETAIL_INI = "1"
python -m tools.cargo_run -- test -p vera20k-autoplay --lib
python -m tools.cargo_run -- build --profile fast -p vera20k-autoplay --bin autoplay-match
& "E:\Projects\vera20k-autoplay\target\owned-worktrees\072666c56a10730e\fast\autoplay-match.exe"
```

预期：`assignments` 低于 52、`attacks` > 0、`active=13`、`ended` 仍有 `Reassess`、
90+ 测试通过。再跑 3000 帧长局确认不漂移。

## 7 接口边界

能碰：`autoplay/src/dispatch.rs`、`frame.rs`、`idle.rs`、新增 `tracked.rs`、
`autoplay/src/tests/`、`autoplay/src/lib.rs`（导出）、`autoplay/src/bin/autoplay-match.rs`（报告计数）。
不许碰：`vera20k-core` 的命令语义、`src/sim/` 任何状态、`src/app/input/hotkeys*`。
一个状态一个主人：追踪记录归 `autoplay` crate，不写进 `Simulation`。

## 8 收尾要写什么

做到哪、下一张卡（T02/T03/T05 中依赖已满足的那张）、实测 `assignments`/`attacks`/`ended`
与测试数、未决项（尤其「真实对局是否触发重发」）；同步把本表状态改成「已完成」并填完成证据。
