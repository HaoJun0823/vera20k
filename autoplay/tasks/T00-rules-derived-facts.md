# T00 规则派生量进入决策链（T03/T04/T05 共同前置）

| 项 | 值 |
|---|---|
| 状态 | **已完成（第一刀：规则派生量）**；「可造类型枚举」仍待办 |
| 层 | 底座（跨 1/3/4 层的只读投影） |
| 依赖 | 无 |
| 负责人 | 本会话 |
| 完成证据 | 提交 `autoplay: carry rule-derived facts in the Snapshot (T00)`（2026-11-10）；94 测试通过（+1）；真实对局 600 帧基线**不变**（`assignments=16`、`active=13`、`ended={Reassess: 39}`、`credits=10000`、`stock=13`）；`rule_facts={"AMCV": (3000, None, false, false), "E1": (200, Some(4), false, true), "MTNK": (700, Some(5), false, true)}` |

## 1 要做什么

让 AutoPlay 的决策能回答「这个单位能打什么、射程多远、这个建筑/单位造价多少」，
**全部从 rules 读出，不写死单位名**（`DESIGN.md` §2 铁律 5）。现在这三张卡都卡在同一个
缺口上：

- T03 威胁评估要射程、伤害、能否对空/对地
- T04 对策性选目标要「我能不能伤到它」
- T05 按战局生产要造价、可造类型清单、缺电判断

## 2 游戏里能看到什么

**本卡本身看不到玩法变化**（只读投影）。但做完之后，T03 的「不再冲防御塔」、
T04 的「不再一堆单位叠同一目标」、T05 的「会按需补电厂/矿厂/兵种」才有数据可用。
按 `DESIGN.md` §6，这类地基卡要在收尾里写明「解锁了什么」，不能只报「读数更全了」。

## 3 合格线（可数）

- `Snapshot::Object` 带上至少三项规则派生量（主武器射程格数、能否对空、造价），
  由 `Snapshot::take` 从 `&RuleSet` 读出；`autoplay-match` 能打印它们。
- 每条派生量有一个规则测试固定数值（用生产代码路径读零售或 fixture rules，
  不手写期望值）。
- ~~提供一个按房屋/阵营枚举「当前可造类型」的查询~~ **本刀未做，见 §8**：
  `RuleSet::all_objects()` 是 pub（`ruleset.rs:10903`），但它列的是全部类型，不是
  「这一方此刻能造什么」，还需按 prerequisite / 工厂 / 阵营过滤。留给 T05 开工时的
  第一小步，不得用硬编码类型名列表绕过。
- `plan_frame` 或其下游能拿到这些量；**不新增第二个规则状态所有者**。
- 全量 `vera20k-autoplay` 测试通过（当前 93），真实对局基线数字不变
  （600 帧 `assignments=16`、`active=13`、`ended={Reassess: 39}`、账本不变）。

## 4 不许复发

- **P16** 兵种单一：T05 靠「可造类型清单」才能造不止一种兵，不能靠写死名字绕过。
- **P09** 不会对空的单位追着飞机跑：T03/T04 靠「能否对空」判断。
- **P03/P23** 不会避让威胁/绕路：T03 靠射程判断「防御塔能打到我」。
- **P01** 拆东墙补西墙：派生量只读、每次从 `Snapshot` 重建，不缓存、不写回引擎。

## 5 依赖与前置事实（2026-11-10 实测）

- `autoplay/src/world.rs:28` 的 `Object` 只有 `id / owner / type_name / category /
  cell / health / strength / in_playfield / visible`，**没有任何规则派生量**。
- `Snapshot::take(sim, rules, house)` **已经接收 `&RuleSet`**，但目前只用它读
  `entity.type_ref()` 之外的东西（见 `world.rs:90`）；规则数据没进投影。
- **实测纠正**：`RuleSet` 的接口比写卡时以为的齐全，全部 pub：
  `object(&str) -> Option<&ObjectType>`（`:6924`）、`all_objects()`（`:10903`）、
  `weapon(&str)`（`:7212`）、`projectile(&str)`（`:7429`）。
  `ObjectType` 有 `cost` / `weapon_list`；`WeaponType` 有 `range_leptons`（单位 leptons，
  `256` 为一格，见 `src/util/lepton.rs`）和 `projectile`；`ProjectileType` 有 `aa` / `ag`。
  `Snapshot::take` 本来就已用 `sim.object_type(entity.type_ref(), rules)` 取 `strength`，
  所以四个派生量**不需要改 `vera20k-core`**，只在 `autoplay/src/world.rs` 里补读即可。
  剩下缺的只是「这一方此刻能造什么」的过滤，那是 T05 的事。
- `Command::QueueProduction { type_id: InternedId }` 存在（`command.rs:1384`），
  且在 `world_commands.rs:2111` 有执行路径，不是空命令；T05 可走它。
- `Command::Move { entity_id, target_rx, target_ry }` 可用于 T03 的撤退
  （`command.rs:1334`），但撤退目标格子需要地图边界；`sim.session()` 有
  `map_width`/`map_height` 的内部用法（`world/mod.rs:4530`），外部可见性待确认。

## 6 验证命令

```powershell
cd E:\Projects\vera20k-autoplay-v3
$env:RA2_DIR = "I:\SteamLibrary\steamapps\common\Command & Conquer Red Alert II"
$env:VERA20K_REQUIRE_RETAIL_INI = "1"
python -m tools.cargo_run -- test -p vera20k-autoplay --lib
python -m tools.cargo_run -- build --profile fast -p vera20k-autoplay --bin autoplay-match
& "E:\Projects\vera20k-autoplay\target\owned-worktrees\072666c56a10730e\fast\autoplay-match.exe"
```

预期：93+ 测试通过；报告里能看到每种单位类型的射程/造价；
真实对局的 `assignments` / `active` / `ended` / 账本与 T01 基线一致（只读投影不改行为）。

## 7 接口边界

能碰：`autoplay/src/world.rs`、`autoplay/src/tests/world_and_trace.rs`、
`autoplay/src/bin/autoplay-match.rs`（报告）、`autoplay/src/lib.rs`。
若确需「枚举可造类型」，先在 `vera20k-core` 加只读查询并说明原生依据
（AGENTS.md 要求 native function address 就近引用）；**不要**在 autoplay crate 里
另建一份规则表或类型清单。
不许碰：`src/sim/` 的状态写入、引擎的弹药/装填规则、客户端热键、生产命令的语义。

## 8 收尾记录

- 做到哪：`Snapshot::Object` 新增 `cost` / `range_cells` / `can_target_air` /
  `can_target_ground`，全部由 `Snapshot::take` 从 `&RuleSet` 读（`world.rs`）。
  主武器取 `weapon_list` 第一个；射程由 `range_leptons / LEPTONS_PER_CELL_I32` 换算，
  无限哨兵与无武器记 `None`；对空/对地取主武器 projectile 的 `aa`/`ag`。
  `autoplay-match` 按类型去重打印 `rule_facts`。
- 解锁了什么：T03 现在能读射程与对空/对地；T04 能读「能不能伤到它」的一半；
  T05 能读造价。三张卡不再需要各自重查 rules。
- 测试：`a_snapshot_carries_rule_derived_facts` 断言 snapshot 里的 `cost` 等于
  `rules.object("MTNK").cost`（先断言该类型存在且 cost > 0，避免空真），
  并固定 MTNK 有射程、能对地、不能对空。94 个 crate 测试通过。
- 真实对局：600 帧所有基线数字**完全不变**——只读投影没有改变任何行为。
  `rule_facts` 读到零售真实值：AMCV 3000 / E1 200 射程 4 / MTNK 700 射程 5。
- **是否需要改 `vera20k-core`：不需要**（实测纠正了写卡时的猜测）。
- 未决项：
  1. 「这一方此刻能造什么」的过滤查询未做（T05 第一小步）。
  2. 伤害 / 装甲克制（`warhead Verses × armor`）还没进投影，T04 的对策性选目标仍缺这块。
  3. 派生量每帧从 `Snapshot` 重建，未缓存；若帧开销变大再评估。
- 下一张卡：T03 或 T05（两者现在都可开工）。
