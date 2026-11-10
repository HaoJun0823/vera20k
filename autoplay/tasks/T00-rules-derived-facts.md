# T00 规则派生量进入决策链（T03/T04/T05 共同前置）

| 项 | 值 |
|---|---|
| 状态 | 待办 |
| 层 | 底座（跨 1/3/4 层的只读投影） |
| 依赖 | 无 |
| 负责人 | （谁领谁填） |
| 完成证据 | 提交号 + 实测数字（完成时填） |

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
- 提供一个按房屋/阵营枚举「当前可造类型」的查询，供 T05 使用；
  该查询不得依赖硬编码的类型名列表。
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
- `RuleSet` 有 `type_cost` / `cost_of`（`ruleset.rs:7173` / `:7125`）、
  `building_dimensions`（`:9097`），但 grep `pub fn .*types` 只找到 particle / voxel /
  insignia / overlay 等目录，**没有「枚举所有可造单位/建筑类型」的公开接口**。
  这一项可能需要在 `vera20k-core` 侧加查询，属于跨 crate 改动，动手前先确认 owner。
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

## 8 收尾要写什么

做到哪、解锁了哪几张卡（T03/T04/T05）、每条派生量的测试数值、
「是否需要改 vera20k-core」的结论、真实对局基线是否不变、未决项；
同步把 `INDEX.md` 状态改成「已完成」并填完成证据。
