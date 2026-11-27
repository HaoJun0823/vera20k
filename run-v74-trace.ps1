# V74-R0 只读诊断启动脚本
#
# 作用：开启 V74 诊断（每 90 tick 记一次 v74_* 行），追加 -speedcontrol 参数后启动游戏。
# 诊断只读取现有状态，不改策略、不下命令；但会增加日志开销，
# 不要用它的 wall-clock 吞吐去和普通版本比较。
#
# 用法（在项目根目录）：
#   .\run-v74-trace.ps1                    # 诊断 + -speedcontrol 启动
#   .\run-v74-trace.ps1 -PassThru "-win"   # 再附带其它游戏参数
#
# 关闭诊断（重要：设为 0 也仍然开启，必须删除变量）：
#   Remove-Item Env:AUTOPLAY_TRACE_V74

param(
    # 可执行文件位置；路径里的 hash 目录若变化，改这里即可。
    [string]$Exe = 'E:\Projects\vera20k\target\owned-worktrees\1cd1e4a5ab9c9602\fast\vera20k.exe',
    # 游戏工作目录；日志会写进它的 logs\ 子目录。默认是项目根目录。
    [string]$WorkDir = $PSScriptRoot,
    # 需要原样透传给游戏的额外参数，例如 "-win"。
    [string]$PassThru = ''
)

# 1. 诊断开关：只要变量存在就开启，任意值（含 "0"）都算开启。
$env:AUTOPLAY_TRACE_V74 = "1"

# 2. 游戏参数：始终追加 -speedcontrol（原版零售开关，战役保留无上限速度）。
$gameArgs = @('-speedcontrol')
if ($PassThru -ne '') {
    $gameArgs += $PassThru
}

if (-not (Test-Path -LiteralPath $Exe)) {
    Write-Error "找不到游戏程序：$Exe"
    exit 1
}
if (-not (Test-Path -LiteralPath $WorkDir)) {
    Write-Error "找不到工作目录：$WorkDir"
    exit 1
}

Write-Host "程序   : $Exe"
Write-Host "工作目录: $WorkDir"
Write-Host "参数   : $($gameArgs -join ' ')"
Write-Host "诊断   : AUTOPLAY_TRACE_V74=$env:AUTOPLAY_TRACE_V74"
Write-Host "日志   : 写入 $WorkDir\logs\ 下的 .log 文件（grep 'v74_' 查看）"
Write-Host "关闭诊断: Remove-Item Env:AUTOPLAY_TRACE_V74"
Write-Host "----"

Push-Location -LiteralPath $WorkDir
try {
    & $Exe @gameArgs
}
finally {
    Pop-Location
}
