# T06 四个默认键位 + 实机验证

| 项 | 值 |
|---|---|
| 状态 | 已完成（实机验证待用户） |
| 层 | 4 单位意图与执行（客户端侧） |
| 依赖 | 无（可与其它卡并行） |
| 负责人 | 本会话 |
| 完成证据 | 提交 `vera20k: give the four AutoPlay switches default keys (T06)`（2026-11-10）；热键回归 **28 passed / 0 failed**（原 25 + 新增 3）；客户端 `check` 与 `--profile fast` 编译通过 |

## 1 要做什么

给四个 AutoPlay 命令默认键位，并做一次实机验证：编译后直接按键就能开关，
不用先去 Options → Keyboard 手动绑定。同时确认「玩家不能操作战场时自动停止」(F07)
在客户端侧有明确行为。

## 2 游戏里能看到什么

打开游戏、开一局离线 Skirmish，按绑定键就有屏幕提示显示四个开关状态，
单位开始被接管；不用先去设置页找这四个命令。

## 3 合格线（可数）

- 四个命令各有默认键位，且**不与现有键冲突**（列出四个组合并说明查重方式）。
- 键盘页回归测试通过：`cargo test -p vera20k --lib -- hotkeys` 为 **28 个通过**
  （原 25 + 新增 3）；`retail_commands()` 的 87 条 fixture 不受影响。
- 实机验证：离线 Skirmish 里按全局键 → 屏幕提示状态变化 → 接管单位；
  联机对局**不注入** AutoPlay 命令（避免破坏同步）。**本会话无法运行 GUI，见 §8。**
- `cargo check -p vera20k --bin vera20k` 无错误；`--profile fast` 客户端编译成功。

## 4 不许复发

- **P07** 玩家不能操作战场时（battle control offline）应停止：本卡在客户端侧确认这条。
- **P26** 大部分单位在家发呆：实机验证要看单位是否真的被接管，不能只看提示文字。
- **P01** 拆东墙补西墙：加默认键不许改动四个开关的 toggle 语义
  （全局=四个一起，单按=只切一个）。

## 5 依赖与前置事实

- `890539cae` 已把四个命令接进 `vera20k` hotkey 分类，但**没有默认键**——
  这正是用户上次「按了没反应」的原因，v2 也是无默认键。
- `command!` 宏**末位是本地化参数，不是默认键位**；默认键得改 `hotkeys.rs` 的绑定逻辑。
  VeraX `AutoLoad` 的 Ctrl+D 是一处特判，可作参考但别照抄成第二份特判。
- `MatchState` 只有一个构造点（`src/app/initialize.rs:967`），每局默认关、状态不跨局；
  `sim_tick.rs` 在 `take_due_commands()` 前经 `issue_local_command` 注入（仅离线对局）。
- 客户端从 v3 工作树根目录启动时读到已配好 `ra2_dir` 的 `config.toml`，无需 `RA2_DIR`。
- 全量 `cargo test -p vera20k --lib` 有 **7 个既有失败**（繁中本地化 6 + 上游 pin 漂移 1），
  与本分支无关，**不要顺手改**，也不要把它当回归（`DESIGN.md` §5）。
- **实测查重**：读本机 `KeyboardMD.ini` 的 `[Hotkey]` 段（87 条），`0x300–0x3FF`
  （Ctrl+Shift 组合）完全空闲；已占用的是 Shift+数字、Ctrl+数字、Ctrl+F1..F4、
  Ctrl+Alt+数字。故选 `·`(192) 全局 + Ctrl+Shift+F / B / P = `838/834/848`。
  **2026-11-11 用户指定**：全局切换改用 `·`（backtick / VK_OEM_3 = 192），
  单手好按也好测；本机 `KeyboardMD.ini` 本来就有 `AutoPlay=192`，玩家绑定优先保留。
- **实测发现**：本机 `KeyboardMD.ini` 已有 `AutoPlay=192`、`AutoPlayForces=1216`
  两条手动绑定（v2 时期留下的），所以这两个命令尊重玩家已有绑定、不装默认；
  `AutoPlayBuild` / `AutoPlayAbilities` 用默认键。这解释了「设过键却只有部分有反应」。

## 6 验证命令

```powershell
cd E:\Projects\vera20k-autoplay-v3
python -m tools.cargo_run -- check -p vera20k --bin vera20k
python -m tools.cargo_run -- test -p vera20k --lib -- hotkeys
python -m tools.cargo_run -- build --profile fast -p vera20k --bin vera20k
& "E:\Projects\vera20k-autoplay\target\owned-worktrees\072666c56a10730e\fast\vera20k.exe"
```

预期：25 个 hotkey 测试通过、check 无错误、实机可复现开关与接管。
实机步骤与观察结果写进收尾（不提交截图或日志）。

## 7 接口边界

能碰：`src/app/input/hotkeys.rs`、`src/app/input/hotkeys/catalog.rs`、
相关 `src/app/input/` 测试、`docs/handoff.md`（更新说明）。
不许碰：`dispatch.rs` 的 toggle 逻辑、`sim_tick.rs` 的注入通道、`autoplay/src/` 的决策、
`retail_commands()` 的 87 条 fixture。
一个状态一个主人：`ControlState` 仍归 `MatchState`。

## 8 收尾记录

- 做到哪：`from_ini_bytes`（启动路径）按「命令未绑定 + 键未被占用」安装四个默认键；
  `reload_from_ini_bytes`（Cancel / Reset All）保持纯 INI 回放，不强制装键——
  这样三个原生绑定回归测试不用改断言，只把一个序列化期望补上四行。
  `catalog.rs` 的「无默认键」注释、`docs/handoff.md` 的说明同步更正。
- 默认键：**`·`（192）全局** / `Ctrl+Shift+F` 战斗（838） /
  `Ctrl+Shift+B` 生产（834） / `Ctrl+Shift+P` 能力（848）。
- 实测：28 个热键测试通过（新增 3 个：新配置有默认键、玩家绑定优先、
  键被占用时不抢占）；客户端 `check` 与 `--profile fast` 编译通过。
- **未完成 / 如实留白**：**实机验证没做**——本会话无法运行 GUI。离线 Skirmish 里
  按全局键后的屏幕提示、单位是否真的被接管、联机对局不注入命令，都需要用户实机确认。
- 下一张卡：T02 / T03 / T05 均可并行（T01 依赖已满足）。
