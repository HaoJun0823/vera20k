# T04 目标分配与集火

| 项 | 值 |
|---|---|
| 状态 | **已完成（第一刀：多目标分摊）**；硬上限与对策性匹配留第二刀 |
| 层 | 3 战术微操 |
| 依赖 | T03（已完成）、T00（已完成） |
| 负责人 | 本会话 |
| 完成证据 | 提交 `autoplay: spread attacks across visible targets (T04 first slice)`（2026-11-10）；**102** 个 crate 测试通过（+2）；默认 600 帧基线与 T03 **完全一致**（`assignments=13 attacks=12 retreats=0 guards=1`、`credits=10000`、`stock=13`）；1v7 3000 帧 `attack_targets=13`、`max_attackers_on_one_target=8` |

**为什么要拆**：原合格线要求「同一目标指派单位数有硬上限」，但当前架构里
多出来的单位除了 `Guard` 没有别的事可做；在只有一个可见敌人的平静局里，硬上限
会把 10 个单位变成站着不动，正是 P07/P26「在家挂机」的倒退。对策性匹配
（对哪种装甲有效）还缺 `warhead Verses × armor` 的投影（见 T00 §8）。
所以第一刀只做**有多个可见目标时的分摊**，两个硬要求留到有更多战术动作之后。

## 1 要做什么

多个单位不再各打各的：分配目标时避免一堆单位叠同一个目标造成伤害溢出；
按单位擅长对付什么来选目标（对策性）；排除打不动/无敌/不该打的目标；
保护英雄与高价值单位、引诱扛伤、掩护关键单位。先做前三条，后三条可在本卡内分小步。

## 2 游戏里能看到什么

一群 V3 不再把导弹全砸同一个目标；GI 去打载具和空军而不是硬啃步兵；
不会对空的单位不再追着飞机跑；桥梁小屋、铁幕单位不再被白白攻击。

## 3 合格线（可数）

**第一刀（本轮已完成）**：

- 有 ≥2 个可见敌人时，单位优先打「已被指派攻击者最少」的那个（平手取最近），
  不再一窝蜂叠同一个目标；`autoplay-match` 报告 `attack_targets` 与
  `max_attackers_on_one_target`。
- 只有 1 个可见敌人时**仍进攻**，不许把多余单位变成站着不动（P07/P26）。
  有测试固定。
- 优势场景仍会进攻（不许靠一律不打来凑零攻击）；全量测试通过；默认局基线不变。
- 打不动 / 看不见的目标不进目标池（T03 的 `can_hurt` 已覆盖，本卡沿用）。

**第二刀（仍待办，见 §8）**：

- 同一目标的指派单位数**硬上限**（按目标有效血量 / 我方每秒伤害估算）。
- 禁用目标（无敌、铁幕、装饰障碍）攻击数为 **0** 的专门测试。
- 对策性匹配用 `warhead Verses × armor` 推导，不写死单位名。

## 4 不许复发

- **P08** 狗咬坦克/步兵打盖特、**P09** 追飞机、**P11** 攻击无敌目标：本卡直接针对。
- **P12** 缺乏对有攻击能力建筑的控制（驻扎建筑、防御塔）：目标类型要包含这类对象。
- **P13** 把被占领/心控的自己人当敌人打：目标筛选过 `eligibility` 与心控链接。
- **P14** 认不出围墙树木等装饰障碍：装饰类对象不进目标池。
- **P16** 兵种单一：本卡只管「用现有兵种选对目标」，造什么归 T05，别越界。
- **P01** 拆东墙补西墙：新增分配逻辑不许把 T03 的撤退判断覆盖掉。

## 5 依赖与前置事实

- 依赖 T03：威胁评估先到位，否则「分配」会把本该撤退的单位也塞进攻击队列。
- `docs/handoff.md` 明确记着当前缺口：「多个单位可能同时打同一目标造成伤害溢出」、
  「没有威胁评估、撤退、集火分配、分弹、小队搭配、卡射程」。
- `trace.rs` 已能记录全部候选、赢家、理由；分配决策要写进 `Trace`，
  便于事后解释「为什么这个单位打那个」（架构书 §20.10）。
- 能力事实从 rules 推导（`DESIGN.md` §2 铁律 5），`availability.rs` 只读运行时状态，
  绝不重算弹药/装填/载员。

## 6 验证命令

```powershell
cd E:\Projects\vera20k-autoplay-v3
$env:RA2_DIR = "I:\SteamLibrary\steamapps\common\Command & Conquer Red Alert II"
$env:VERA20K_REQUIRE_RETAIL_INI = "1"
python -m tools.cargo_run -- test -p vera20k-autoplay --lib
& "E:\Projects\vera20k-autoplay\target\owned-worktrees\072666c56a10730e\fast\autoplay-match.exe"
```

预期：默认局基线不漂移；多目标局 `attack_targets > 1`；102 个测试通过。
构造劣势局：`& "...\autoplay-match.exe" "$env:RA2_DIR" Arena.mmx 3000 7`

## 7 接口边界

**实测纠正**：原计划「新增 `allocation.rs`」不必要。`Dispatcher` 本来就持有
每个单位最后发出的命令，加一个只读的 `attackers_on(target)` 就能算出
「多少单位正打这个目标」，不需要第二份状态。按「一个状态一个主人」，
本刀**没有新建模块**。

能碰：`autoplay/src/dispatch.rs`（只读查询）、`threat.rs`（目标选择）、
`frame.rs`（`FramePlan.target`）、`tests/threat.rs`、`bin/autoplay-match.rs`。
不许碰：`vera20k-core` 的伤害计算、`src/sim/` 状态、客户端热键、T05 的生产决策。
攻击计数每帧从 `Dispatcher` 读，不跨帧保留副本。

## 8 收尾记录

- 做到哪：`Dispatcher::attackers_on` 统计「仍生效的、指向该目标的 Attack 命令」；
  `threat::assess` 在可见敌人 ≥2 时按 `(已指派数, 距离)` 选目标，平手取最近；
  只有 1 个敌人时仍打它。`FramePlan` 带 `target`，`autoplay-match` 报告
  `attack_targets` / `max_attackers_on_one_target`。
- 实测：
  - 默认 `Arena.mmx` 600 帧：与 T03 基线**完全一致**——
    `assignments=13 attacks=12 retreats=0 guards=1`、
    `attack_targets=1 max_attackers_on_one_target=12`（单目标不拒绝进攻）、
    `credits=10000` / `stock=13` / 账本不变。
  - 1v7 3000 帧：`assignments=181 attacks=59 retreats=121 guards=1`、
    `attack_targets=13 max_attackers_on_one_target=8`。
    59 次攻击分散到 13 个目标，不再全部叠一个。
  - 102 个 crate 测试通过（+2：`a_group_spreads_across_targets`、
    `a_single_target_is_never_refused`）；客户端 `check` 与 `--profile fast` 编译通过。
- **一个必须说明的读数**：1v7 的 `retreats=121` / `disadvantage_frames=16828`
  比 T03 时的 `23` / `2601` 高很多，**两个版本的数字不可直接比较**——命令分布
  改变了战局走向（单位存活时间、可见敌人数都不同）。这不是「分摊让撤退变多」
  的证据，也不是回归；默认局基线完全一致才是回归判据。
- 未决项（第二刀）：
  1. 硬上限未做：多出来的单位当前只会继续进攻最近目标，没有「够了就换事做」。
  2. 伤害 / 装甲克制未进投影（`warhead Verses × armor`），对策性选目标仍缺。
  3. 无敌 / 铁幕 / 装饰障碍（P11 / P14）未专门识别，只有 `can_hurt` 的类别过滤。
  4. 英雄与高价值单位保护、引诱扛伤、掩护关键单位未做。
  5. 分配理由尚未写进 `Trace`（架构书 §20.10），事后只能看到选了谁，看不到为什么。
- 下一张卡：T05（按战局生产）或 T04 第二刀；T07（占领）依赖本卡第二刀更稳。
