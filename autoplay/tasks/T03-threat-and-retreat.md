# T03 威胁评估与撤退

| 项 | 值 |
|---|---|
| 状态 | 待办 |
| 层 | 3 战术微操 |
| 依赖 | T01 |
| 负责人 | （谁领谁填） |
| 完成证据 | 提交号 + 实测数字（完成时填） |

## 1 要做什么

给「打最近的可见敌人」补上判断：打得过就打、打不过就跑、跑了没意义就鱼死网破。
先做最小可用的一版——按可见敌人的火力/射程与我方生存能力估算，劣势时撤退，
并绕开明显的防御塔威胁；不做风筝/躲弹/卡射程（那些在后续高级微操卡）。

## 2 游戏里能看到什么

单位不再排队往正面防御塔里冲；明显打不过时会脱离；被围死无处可退时仍会拼，
而不是原地发呆或一直逃跑。

## 3 合格线（可数）

- 基线：`6558ee2a7` 600 帧 `attacks=52 guards=0`，对同一可见敌人一律攻击。
- 面对防御塔/火力优势目标时，攻击命令数下降；撤退命令有独立计数并在
  `autoplay-match` 报告里可见（例如 `retreats=N`）。
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

## 8 收尾要写什么

做到哪、下一张卡（T04）、`attacks`/`guards`/`retreats` 实测数字、
哪些老毛病已有实测证据、哪些仍只有单元测试；同步更新 `INDEX.md`。
