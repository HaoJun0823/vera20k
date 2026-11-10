# T05 战略第一刀：按战局生产

| 项 | 值 |
|---|---|
| 状态 | 待办 |
| 层 | 1 战略 |
| 依赖 | T01 |
| 负责人 | （谁领谁填） |
| 完成证据 | 提交号 + 实测数字（完成时填） |

## 1 要做什么

**只做第一刀**：按当前缺口决定造什么——缺电补电厂、缺钱补矿厂/矿车、
按战局生产至少两类战斗单位。不做分基地、围墙、科技树、跨阵营生产、超武
（那些是后续卡，见 `INDEX.md` §3）。目标是让「战略层」这一层第一次有实际产出。

## 2 游戏里能看到什么

AutoPlay 接管的房屋会按需补电厂/矿厂、出不止一种兵，不再只有守家和打最近的敌人。

## 3 合格线（可数）

- 真实对局中 `ledger.produced` 出现本方新增建筑/单位，且战斗单位**至少两类**。
- 缺电时电厂优先级高于战斗单位；有测试固定这条边界。
- 生产命令走 `Arbiter::admit` → `issue_local_command` 玩家通道，不直接写仿真状态。
- 新增的战略模块是**独立层 1**，只被 `frame.rs` 调用；层 4 不反向依赖层 1 的内部状态。
- 全量 `vera20k-autoplay` 测试通过；客户端 `--profile fast` 编译成功。

## 4 不许复发

- **P16** 兵种单一：本卡直接针对，合格线里写死「至少两类」。
- **P07/P26** 在家挂机：生产出来的单位要能被任务板接管，不能造完就晾着。
- **P05** 高频发指令：生产命令同样过 T01 的去重追踪，不许每帧刷建造队列。
- **P01** 拆东墙补西墙：本卡只加生产，不改战斗决策；T03/T04 的行为不许退化。
- **P21** 狂造工程师拖垮经济：本卡不造工程师（那是 T07），且要有花费上限意识。

## 5 依赖与前置事实

**2026-11-10 实测补充（开工前先看，省得重查）**：

- `Command::QueueProduction { type_id: InternedId }`（`command.rs:1384`）在
  `world_commands.rs:2111` 走 `production::enqueue_by_type(self, rules,
  command_owner, &type_s, frame_effects, false)`。**payload 不命名 owner**，
  作用在 event header 的 house 上（源码注释：「Production events act on the
  event's own house; the payload names no owner」）。
- 这跟 `Arbiter::admit` 的形状有冲突：`admit_with_source`（`arbiter.rs:113`）必须传
  `subject: u64`，会 `sim.entities().get(subject)` 检查存在与归属，再用
  `control.may_command(owner, subsystem)` 判权限。生产命令**没有天然 subject**，
  接入层 1 时第一个要定的就是「拿哪个本方对象当准入锚点」，或给 Arbiter 加
  一条 house 级准入路径（不要绕过 Arbiter 直接写 `Command`）。
- `RuleSet::all_objects()`（`ruleset.rs:10903`）可枚举全部 `ObjectType`，
  但那是「全部类型」不是「这一方此刻能造什么」；还需按 prerequisite / 工厂 /
  阵营过滤。`is_build_tech(&str)`（`:10143`）可能是入口，需实测确认。
- `RuleSet::warhead(&str)`（`:7295`）/`warheads_iter()`（`:10878`）是 pub，
  `WarheadType::Verses` 是伤害/装甲克制的数据源——这也是 T04 第二刀的前置。

- 依赖 T01：没有去重追踪，每帧重复下建造命令会触发 P05 那一类问题。
- `ledger.rs` 已有生产/损失/资产/资金四账本，按 **object id** 计流量
  （「造一个、丢一个」仍是生产 1，按类型存量差会漏掉）；`Snapshot` 带房屋现金余额。
  **不要在战略层另建一份资金或单位计数**——读账本与快照。
- `src/sim/` 是平铺的原生 AI 模块（`ai_build_need.rs`、`ai_base_building.rs` 等），
  v3 没有 `sim/strategy/`；本卡在 `autoplay` crate 内新建层 1 模块，
  **不重写原生队伍 AI**（[`../DESIGN.md`](../DESIGN.md) §2 铁律 / `docs/plan.md` §七「原生 AI 与
  AutoPlay 的最终分工」、`docs/handoff.md` §九「与评审的关键结论」）。
- v2 判定（`docs/v2-assessment.md`）：基础建造链可靠可复用，但「长期只造一种最便宜的」
  是 v2 的毛病，别照搬它的配比。

## 6 验证命令

```powershell
cd E:\Projects\vera20k-autoplay-v3
$env:RA2_DIR = "I:\SteamLibrary\steamapps\common\Command & Conquer Red Alert II"
$env:VERA20K_REQUIRE_RETAIL_INI = "1"
python -m tools.cargo_run -- test -p vera20k-autoplay --lib
python -m tools.cargo_run -- build --profile fast -p vera20k-autoplay --bin autoplay-match
& "E:\Projects\vera20k-autoplay\target\owned-worktrees\072666c56a10730e\fast\autoplay-match.exe"
```

预期：`produced` 里出现新增条目、战斗单位 ≥ 2 类；
3000 帧长局 `credits`/`net_flow` 不出现失控；客户端编译通过。

## 7 接口边界

能碰：`autoplay/src/` 新增 `strategy/`（层 1）、`frame.rs`（调用）、`lib.rs`（导出）、
`autoplay/src/tests/`、`autoplay/src/bin/autoplay-match.rs`（报告）。
不许碰：`src/sim/` 的原生 AI、`vera20k-core` 生产状态写入、客户端热键、
T03/T04 的战术决策。命令只能经 `Arbiter::admit` 产出。

## 8 收尾要写什么

做到哪、下一张卡、`produced` 明细与战斗单位种类数、账本数字、未决项
（尤其「分基地/围墙/科技树仍缺」要如实写）；同步更新 `INDEX.md`。
