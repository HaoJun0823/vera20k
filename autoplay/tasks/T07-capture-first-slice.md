# T07 占领第一刀（重中之重）

| 项 | 值 |
|---|---|
| 状态 | 待办 |
| 层 | 3 战术微操 |
| 依赖 | T04 |
| 负责人 | （谁领谁填） |
| 完成证据 | 提交号 + 实测数字（完成时填） |

## 1 要做什么

**许愿书里用户标为「重中之重」的 F40**：识别可占领目标（中立设施、敌方建造场），
派工程师去占领，占领成功要能数出来。第一刀**不做**「占领后打包成 MCV 撤离」(P20)、
跨阵营生产 (F09)、渗透 (F39)、分基地 (F10)——那些各拆一张后续卡。

## 2 游戏里能看到什么

AutoPlay 会派工程师去占中立建筑和敌方建造场，占领后该建筑换到己方名下；
不会把时间浪费在拆中立装饰物上。

## 3 合格线（可数）

- 真实对局里占领事件计数 > 0；`Ledger` 的 `transferred`（所有权换手）反映它，
  且**不记成生产**（`ledger.rs` 已把占领从生产里扣除）。
- 工程师数量有上限，不因占领目标多而狂造拖垮经济；有测试固定这个上限。
- 中立装饰物/围墙的拆除命令数不因本卡上升；被围墙围住的目标要能破墙进入（P25）。
- 目标只看**可见**对象（不作弊）；敌方区域内的中立设施要能识别并谨慎处理。
- 全量 `vera20k-autoplay` 测试通过；全量行为不退化（T03/T04 的撤退与分配仍生效）。

## 4 不许复发

- **P19** 太爱拆中立建筑和装饰物、**P25** 中立设施被围墙围住没人破墙：本卡直接针对。
- **P21** 分不清中立设施在敌方区域、占领后被拆、狂造工程师拖垮经济：工程师上限 + 区域判断。
- **P10** 占领工厂后把新建筑造在敌人脸上：本卡不含建造逻辑，但要留好接口边界，
  别让后续建造卡拿到「刚占下的前线位置」当默认基地。
- **P02** 中立闸门挡路不会打掉、卡门口挨打：派工程师的路径要能处理挡路障碍。
- **P20** 占领敌方建造场后不会打包成 MCV 撤离：本卡**明确不做**，收尾写成未决项，
  不要假装已解决。
- **P13** 把被占领的自己人仍判断为敌人：占领后 `eligibility` 要立刻反映新属主。

## 5 依赖与前置事实

- 依赖 T04：目标分配先到位，否则多个工程师会叠同一个目标、或把战斗单位也派去占领。
- `ledger.rs` 已区分「占领从生产里扣除、换手记为转移而非击杀」；占领事件可读，
  但 `docs/handoff.md` 记着 `ConstructionYardCaptured` 只覆盖一种建筑——
  **不要依赖单一事件做全部占领计数**，用快照对比 + 事件互补。
- `eligibility.rs` 每次重读世界、不缓存，对象换手后答案会变（有测试固定）。
- v2 从没做过占领（`docs/v2-assessment.md`），没有可照搬的实现；
  但 v2 工作树 `E:/Projects/vera20k-autoplay/docs/ai-layers.md` 记过
  `control::placement` 的选址搜索可作路径参考（v3 里没有这个文件）。
- 许愿书 §「一些思路」里「把所有可占领/渗透建筑拿下是有价值的，因为微操好、战损小」
  是本卡的方向依据，但**渗透不在本卡范围**。

## 6 验证命令

```powershell
cd E:\Projects\vera20k-autoplay-v3
$env:RA2_DIR = "I:\SteamLibrary\steamapps\common\Command & Conquer Red Alert II"
$env:VERA20K_REQUIRE_RETAIL_INI = "1"
python -m tools.cargo_run -- test -p vera20k-autoplay --lib
python -m tools.cargo_run -- build --profile fast -p vera20k-autoplay --bin autoplay-match
& "E:\Projects\vera20k-autoplay\target\owned-worktrees\072666c56a10730e\fast\autoplay-match.exe"
```

预期：报告中 `captures > 0`、`transferred` 有条目、`produced` 不虚增；
3000 帧长局工程师数量与 `net_flow` 不失控。

## 7 接口边界

能碰：`autoplay/src/frame.rs`、新增 `capture.rs`、`autoplay/src/tests/`、
`autoplay/src/bin/autoplay-match.rs`（报告 `captures`）、`autoplay/src/lib.rs`。
不许碰：`vera20k-core` 的占领状态写入（命令走玩家通道）、T05 的生产决策、
客户端热键、`retail_commands()` fixture。
一个状态一个主人：目标池每帧从 `Snapshot` 重建，不另存一份「已占领」清单。

## 8 收尾要写什么

做到哪、下一张卡（建议拆「占领后打包撤离 / 跨阵营生产 / 渗透」三张）、
`captures`/`transferred`/工程师数实测值、
**明确写出 P20 与 F09/F39/F10 仍未做**、其它未决项；同步更新 `INDEX.md`。
