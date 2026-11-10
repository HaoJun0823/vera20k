# T02 任务「被别的写入者取代」判定

| 项 | 值 |
|---|---|
| 状态 | 待办 |
| 层 | 4 单位意图与执行 |
| 依赖 | T01 |
| 负责人 | （谁领谁填） |
| 完成证据 | 提交号 + 实测数字（完成时填） |

## 1 要做什么

任务除了「失效」（单位消失/换手/出战场/开关关闭）和「窗口到期」(Reassess) 之外，
补上具名的 `TaskEnd::Superseded`：当玩家、原生队伍 AI 或触发器抢走某个单位的命令权时，
AutoPlay 认出来、结束自己的任务、不再重复派活。

## 2 游戏里能看到什么

玩家手动接管某个单位后，AutoPlay 不再跟它抢着下令；
被原生队伍脚本征走的单位不会被 AutoPlay 反复拉回来。

## 3 合格线（可数）

- 测试能区分两种情形：**自己刚发的命令**不算被取代，**别人发的**才算。
- `TaskEnd::Superseded` 有计数；构造一个「玩家抢走单位」的场景，断言 AutoPlay 不再给它派活。
- 已有的失效与 Reassess 判定不退化：`ended={Reassess: ...}` 仍出现，`active=13` 稳定。
- 全量 `vera20k-autoplay` 测试通过，数量不减少。

## 4 不许复发

- **P01** 拆东墙补西墙：不能为了做出 `Superseded` 计数，把已有结束原因弄坏。
- **P07/P26** 在家挂机：认出「被接管」不等于「永久放弃」，窗口到期仍要重新评估。
- **P13** 把被占领/心控的自己人当敌人打：判定要读 `eligibility` 与 `availability` 的心控链接，
  不能只看 id。
- **P28** 命令无效无动作：不要把「命令没生效」误判成「被别人取代」。

## 5 依赖与前置事实

- **核心难点**：`handover.rs::has_pending_replacement` 读引擎的 queued mission，但**AutoPlay
  自己刚发的命令也会进 queued mission**，直接判会 100% 误报（`docs/handoff.md` 已记录）。
  所以必须先有 T01 的追踪记录，能回答「这条 queued 是不是我发的」。
- `claims.rs` 已有来源优先级（玩家 > 脚本 > AutoPlay），`arbiter.rs` 有具名拒绝，
  但那是「下发时让步」，不是「事后发现被抢」。本卡补的是后者。
- 平静对局（Arena.mmx 600/3000 帧）里可能**没有任何单位被抢**，`Superseded` 真实对局计数为 0
  属正常。若如此，收尾必须如实写「只有构造场景的测试覆盖，没有真实对局证据」，不要含糊过去。

## 6 验证命令

```powershell
cd E:\Projects\vera20k-autoplay-v3
$env:RA2_DIR = "I:\SteamLibrary\steamapps\common\Command & Conquer Red Alert II"
$env:VERA20K_REQUIRE_RETAIL_INI = "1"
python -m tools.cargo_run -- test -p vera20k-autoplay --lib
& "E:\Projects\vera20k-autoplay\target\owned-worktrees\072666c56a10730e\fast\autoplay-match.exe"
```

预期：新增测试通过；真实对局 `active=13`、`ended` 不退化；
`Superseded` 计数若为 0，在收尾里写明原因。

## 7 接口边界

能碰：`autoplay/src/idle.rs`（`TaskEnd` 增变体、`lapsed` 条件）、`handover.rs`、
`frame.rs`、`autoplay/src/tests/`、`autoplay/src/bin/autoplay-match.rs`（报告）。
不许碰：`vera20k-core` 的 mission 状态写入、客户端热键、`Simulation` 内部。
一个状态一个主人：任务记录仍归 `TaskBoard`，不另建一份「被抢」副本。

## 8 收尾要写什么

做到哪、下一张卡、`Superseded` 在真实对局是否触发（如实）、测试数、未决项；
同步更新 `INDEX.md` 状态与完成证据。
