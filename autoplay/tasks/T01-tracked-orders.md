# T01 单位意图追踪（L4）

| 项 | 值 |
|---|---|
| 状态 | 已完成 |
| 层 | 4 单位意图与执行 |
| 依赖 | 无 |
| 负责人 | 本会话 |
| 完成证据 | 提交 `autoplay: stop re-sending unchanged orders (T01)`（2026-11-10）；93 测试通过；600 帧 `assignments=16`（基线 52）、`attacks=16`、`suppressed_still_running=36`；3000 帧 `assignments=29`（基线 260）、`suppressed_still_running=231`；`active=13`、`ended={Reassess: 39/247}`、账本不变 |

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

- **实测纠正（写卡时的猜测有误）**：`dispatch.rs` 的 `Dispatcher` 本来就是**跨帧**追踪的
  唯一所有者（`orders: BTreeMap<u64, Issued>` 存 payload / frame / 期望 mission / 是否仍生效），
  `repeat_of` 还会读引擎 mission 判断「还在执行」。它此前只被 `replan.rs` 和测试调用，
  **生产路径 `plan_frame` 与客户端 `sim_tick.rs` 完全没接它**——这才是缺口。
  按「一个状态一个主人」，本卡**不新建 `tracked.rs`**，把已有 `Dispatcher` 接进决策链即可。
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
测试全通过。再跑 3000 帧长局确认不漂移。

**实测（已完成）**：600 帧 `assignments=16 attacks=16 guards=0`、
`suppressed_still_running=36`、`active=13`、`ended={Reassess: 39}`；
3000 帧 `assignments=29 attacks=16 guards=13`、`suppressed_still_running=231`、
`active=13`、`ended={Reassess: 247}`。两次账本均与基线一致
（`produced=6/14`、`destroyed=3/4`、`stock=13`）。93 个测试通过、客户端
`check` 与 `--profile fast` 编译通过。

## 7 接口边界

能碰：`autoplay/src/frame.rs`、`autoplay/src/tests/frame.rs`、
`autoplay/src/bin/autoplay-match.rs`、客户端 `src/app/match_runtime/state.rs`（加每局
`Dispatcher` 字段）、`src/app/initialize.rs`（构造点）、`src/app/match_runtime/sim_tick.rs`
（提交成功后 `note_issued`）。
不许碰：`vera20k-core` 的命令语义、`src/sim/` 任何状态、`src/app/input/hotkeys*`、
`dispatch.rs` 已有的 `repeat_of` 三条件（失败可重试必须保留）。
一个状态一个主人：追踪记录归 `Dispatcher`，不写进 `Simulation`，不另建 `tracked.rs`。

## 8 收尾记录

- 做到哪：复用已有 `Dispatcher` 接进 `plan_frame`；相同命令且仍在执行时**不发命令、
  续期任务窗口**；命令变化、任务结束、记录失败三种情况照常下发。`FramePlan` 带
  `payload`/`expected`，调用方仅在提交成功后 `note_issued`。客户端 `MatchState` 加
  每局 `Dispatcher`，并在新对局加载与开关关闭时与 `TaskBoard` 同步清空。
- 下一张卡：**T02**（被取代判定已满足依赖；T03/T05/T06 也可并行）。
- 实测数字：见头部完成证据与 §6。
- 未决项：
  - 「位移超阈值 / 到期重发」目前由 `repeat_of` 的 mission 检查 + `Reassess` 窗口共同决定，
    没有独立的位移阈值；卡里原列的三条重发条件里，这一条尚未单独实现。
  - `Superseded`（被别的写入者抢走命令）仍缺，正是 T02 的内容。
  - 默认键位（T06）与实机验证仍缺。
  - 真实对局里 `SameFrame` 抑制计数为 0（每帧只决策一次，符合预期）。
- 顺带修的既有缺口：`transitions.rs::apply_map_load_result` 原本不重置任何 AutoPlay
  状态，`state.rs` 却注释「每局默认关」。本卡补齐 F06（每局状态独立）。
