# T03 威胁评估与撤退

| 项 | 值 |
|---|---|
| 状态 | 已完成 |
| 层 | 3 战术微操 |
| 依赖 | T01（已完成）、T00（已完成） |
| 负责人 | 本会话 |
| 完成证据 | 提交 `autoplay: break off when the odds are against a unit (T03)`（2026-11-10）；**100** 个 crate 测试通过（+6）；默认 600 帧基线 `assignments=13 attacks=12 retreats=0 guards=1`、`active=13`、`ended={Reassess: 39}`、账本不变；**1v7 实战** 3000 帧 `retreats=23`、`disadvantage_frames=2601`、`attacks=64` |

## 1 要做什么

给「打最近的可见敌人」补上判断：打得过就打、打不过就跑、跑了没意义就鱼死网破。
先做最小可用的一版——按可见敌人的火力/射程与我方生存能力估算，劣势时撤退，
并绕开明显的防御塔威胁；不做风筝/躲弹/卡射程（那些在后续高级微操卡）。

## 2 游戏里能看到什么

单位不再排队往正面防御塔里冲；明显打不过时会脱离；被围死无处可退时仍会拼，
而不是原地发呆或一直逃跑。

## 3 合格线（可数）

- 基线（T00 后、`75697c959`）：600 帧 `assignments=16 attacks=16 guards=0`，
  对同一可见敌人一律攻击。
- 面对火力优势目标时攻击命令数下降、撤退命令有独立计数并在 `autoplay-match`
  报告里可见（`retreats` / `disadvantage_frames`）。**实测**：默认局 `attacks` 16→12、
  出现 1 次 `Guard`（`can_hurt` 过滤掉打不动的目标）；1v7 局 `retreats=23`。
- 三条边界各一个测试：优势→攻击、劣势→撤退、无路可退→仍攻击。
- 威胁判断走 rules 推导（射程、伤害、能否对空/对地），不写死单位名；
  看不见的敌人不参与评估（不许用 `enemy()`）。
- 全量 `vera20k-autoplay` 测试通过；客户端 `--profile fast` 仍能编译。

## 4 不许复发

- **P03** 不会避让威胁、时好时坏、**P23** 不会绕路（正面全是塔仍打正面）：本卡直接针对。
- **P08** 狗咬坦克/步兵打盖特送死、**P22** 克制冲锋送死：劣势判断必须生效。
- **P09** 不会对空的单位追着飞机跑：目标筛选要查能否对空。
- **P11** 攻击无敌目标（桥梁小屋、铁幕单位）：威胁评估同时排除打不动的目标。
- **P14** 认不出围墙树木等装饰障碍：撤退路径要考虑可通行性，别退进死角。
- **P01** 拆东墙补西墙：不许靠「一律不进攻」把攻击数压到 0 来假装变好；
  优势场景必须仍然进攻。

## 5 依赖与前置事实

- `frame.rs::plan_frame` 现在只做一件事：无任务单位挑最近的 `Snapshot::enemy_visible()`
  发 `Command::Attack`，没有可见敌人发 `Command::Guard`。
  `Command::Attack` 在 `world_commands.rs:1986` 转成 `MissionType::Attack`，不是空命令。
- 依赖 T01：去重追踪就绪后再加撤退决策，否则「撤退」和「攻击」会每帧互相覆盖（P05）。
- `availability.rs` 只读单位能力；`Snapshot` 区分「引擎知道」与「玩家可见」。
  威胁评估**只用可见对象**，用 `enemy()` 是作弊（`DESIGN.md` §2 铁律 4）。
- 引擎的原生队伍 AI 照常打两边，本卡只管 AutoPlay 接管的房屋。

## 6 验证命令

```powershell
cd E:\Projects\vera20k-autoplay-v3
$env:RA2_DIR = "I:\SteamLibrary\steamapps\common\Command & Conquer Red Alert II"
$env:VERA20K_REQUIRE_RETAIL_INI = "1"
python -m tools.cargo_run -- test -p vera20k-autoplay --lib
python -m tools.cargo_run -- build --profile fast -p vera20k-autoplay --bin autoplay-match
& "E:\Projects\vera20k-autoplay\target\owned-worktrees\072666c56a10730e\fast\autoplay-match.exe"
```

预期：`attacks` 与 `guards` 不再是非此即彼，`retreats` 有计数；
3000 帧长局确认目标随战局变化、不盲目开火。

## 7 接口边界

能碰：`autoplay/src/frame.rs`、新增 `threat.rs`、`autoplay/src/tests/`、
`autoplay/src/bin/autoplay-match.rs`（报告计数）、`autoplay/src/lib.rs`（导出）。
不许碰：`vera20k-core` 的伤害/射程计算（只读其规则与 `availability`）、
`src/sim/` 状态写入、客户端热键。决策只能产出经 `Arbiter::admit` 的命令。

## 8 收尾记录

- 做到哪：新增 `threat.rs`（`Verdict::{Engage, Retreat, Hold}`、`assess`），
  `frame.rs` 的 `combat_order` 改为按裁决出命令，`OrderKind` 加 `Retreat`，
  `expected_mission(Retreat) -> MissionType::Move`（`Command::Move` 在
  `world_commands.rs:1815` 有真实执行路径）。撤退格子按「远离威胁的主轴」
  移动 4 格并 clamp 到 `sim.session().map_width/height()`，保证不出战场。
  `autoplay-match` 加 `retreats` / `disadvantage_frames` 两个读数，
  并加可选第 4 个参数「对手数」（默认 2，与基线完全一致）用于构造劣势场景。
- 判断依据（全部来自 T00 的规则派生量，不写死单位名）：主武器射程格数、
  能否对空/对地、血量 / 满血、可见敌我数量、曼哈顿格距；整数运算。
  三种劣势：受创且在敌人射程内、被远程压制（它能打我我打不到它）、数量劣势。
  打不动的目标（含飞机 vs 无对空武器）不进目标池；看不见的敌人一律不参与。
- 实测数字：
  - 默认 `Arena.mmx` 600 帧：`assignments=13 attacks=12 retreats=0 guards=1`、
    `disadvantage_frames=0`、`active=13`、`ended={Reassess: 39}`、
    `credits=10000` / `stock=13` / `produced=6 destroyed=3` 全部不变。
    `attacks` 16→12 是 `can_hurt` 过滤的直接效果（治 P09 / P11 的一半）。
  - 1v4 3000 帧：仍平静，`retreats=0`（不是一律不进攻）。
  - **1v7 3000 帧**：`houses=10 objects=317`、`visible_enemies=316`、
    `assignments=88 attacks=64 retreats=23 guards=1`、
    `disadvantage_frames=2601`、`active=0 ended={ObjectGone: 13, Reassess: 207}`。
    劣势真实触发撤退，且 `attacks=64` 证明优势帧仍在进攻（P01 未复发）。
- 哪些仍只有单元测试：三条边界（优势/劣势/无路可退）与「看不见的敌人」、
  「无对空不追飞机」共 6 个测试；1v7 提供了撤退的真实对局证据。
- 未决项（如实留白）：
  1. **伤害 / 装甲克制未进投影**（`warhead Verses × armor`），所以「能不能伤到」
     目前只看目标类别与能否对空对地；无敌 / 铁幕目标（P11）尚未识别。
  2. 无限射程哨兵（`range_leptons = -512`）目前记 `None`（当作 0 处理），
     远程压制判断会低估这类敌人。
  3. 撤退是直线远离 4 格，**没有绕路 / 可通行性判断**（P03 / P23 / P14 只治了一半：
     不再正面硬冲，但不会绕开侧面防御塔、也不检查退路是否被围墙堵死）。
  4. 风筝 / 躲弹 / 卡射程 / 集火分配 / 小队搭配仍是后续卡。
  5. 1v7 局本方 13 个单位全部损失（`ObjectGone: 13`）——会撤不等于会赢，
     强度是交付后的迭代。
- 下一张卡：**T04**（目标分配与集火），或 T05（按战局生产）。
