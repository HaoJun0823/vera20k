# T02 任务「被别的写入者取代」判定

| 项 | 值 |
|---|---|
| 状态 | **阻塞：需设计决策**（未开始，勿直接照原方案实现） |
| 层 | 4 单位意图与执行 |
| 依赖 | T01（已完成），但真正的依赖是下面的架构缺口 |
| 负责人 | （待 owner 拍板方案后领） |
| 完成证据 | 无 |

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

## 5 阻塞证据（2026-11-10 实测，本卡原方案不成立）

**`has_pending_replacement` 不能作为「被抢」的信号**，两条独立证据：

1. `autoplay/src/claims.rs` 模块头写得很明确：`CommandEnvelope` 只带 owner、due frame
   和 payload，**没有字段记录谁下的命令**；两条写同一 mission 的 envelope 到达仿真后
   **不可区分**。并且「this crate cannot stop some other writer - the native team AI,
   a trigger - from reaching the same unit, because it never sees their orders」。
   也就是说，引擎侧根本没有「别人抢了」这个可观测事实。
2. `autoplay/src/tests/handover.rs:247-259` 已经承认「queued 是否可观测取决于 promote
   顺序」，所以测试只敢断言「提交后要么 queued 要么已经在跑，不会什么都没有」。
   即使能观测到 queued，`T01` 确认过自己刚发的命令也会进 queued，直接判必然误报。

T01 的 `Dispatcher` 解决了「这条命令是不是我发的」，但**没有解决「有没有别人下过命令」**——
因为别人的命令根本不经过 AutoPlay，也不会留下可区分的痕迹。

### 三个候选方案（需要 owner 拍板，勿自行假设）

| 方案 | 做法 | 代价 |
|---|---|---|
| **A** | 给 `vera20k-core` 的 `CommandEnvelope` 加来源字段，`Arbiter` 记录每个单位最后写入者；`Superseded` 读它 | 最根本，但**改引擎核心、超出 autoplay crate 边界**，需按 AGENTS.md 走原生证据 |
| **B** | 客户端显式通知：玩家点击/选中/命令单位时调 `tasks.finish(unit, Superseded)` | 改动在 app 层，与 F08「编队单位不托管」同一处，可一起做；但要找全玩家命令入口 |
| **C** | 用 `Dispatcher` 的 `expected` 与当前 mission 不符做启发式判定 | **不推荐**：单位自然完成任务后 mission 也会变，会把「干完了」误判成「被抢」 |

**建议**：先做 B（app 层，与 F08/T06 同层），A 留作后续引擎链。B 落地后本卡的解法
要重写，不是照现在的「queued 检测」实现。

其余前置事实：
- `claims.rs` 已有来源优先级（玩家 > 脚本 > AutoPlay），`arbiter.rs` 有具名拒绝，
  但那是「下发时让步」，不是「事后发现被抢」。
- 平静对局（Arena.mmx 600/3000 帧）里可能**没有任何单位被抢**，`Superseded` 计数为 0
  属正常；若如此，收尾必须如实写「只有构造场景的测试覆盖，没有真实对局证据」。

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
