@echo off
setlocal
pushd "%~dp0" || exit /b 1
set "BENCH_STAGE=%~1"
if not defined BENCH_STAGE set "BENCH_STAGE=smoke"
if /i "%BENCH_STAGE%"=="smoke" goto stage_ok
if /i "%BENCH_STAGE%"=="dev" goto stage_ok
if /i "%BENCH_STAGE%"=="check" goto check
echo Usage: %~nx0 [smoke^|dev^|check]
goto failed

:stage_ok
if /i "%BENCH_STAGE%"=="smoke" set "BENCH_STAGE=smoke"
if /i "%BENCH_STAGE%"=="dev" set "BENCH_STAGE=dev"
call :check_setup
if errorlevel 1 goto failed

set "BENCH_ID="
for /f "delims=" %%I in ('python -B -c "from datetime import datetime; print(datetime.now().isoformat(timespec='microseconds').replace(chr(58), '-'))"') do set "BENCH_ID=%%I"
if not defined BENCH_ID goto failed
set "BENCH_LABEL=local-%BENCH_STAGE%-%BENCH_ID%"
set "BENCH_DIR=%~dp0target\autoplay-results\%BENCH_LABEL%"
echo Building the current v4 worktree...
python -B -m tools.cargo_run -- build -p vera20k-core --profile fast --bin autoplay-match
if errorlevel 1 goto failed

set "BENCH_EXE="
for /f "delims=" %%I in ('python -B -m tools.cargo_run --resolve autoplay-match --profile fast') do set "BENCH_EXE=%%I"
if not exist "%BENCH_EXE%" (
    echo Could not resolve the new autoplay-match build.
    goto failed
)

echo Results: "%BENCH_DIR%"
python -B -m tools.autoplay_benchmark run --suite all --stage "%BENCH_STAGE%" --run-dir "%BENCH_DIR%" --label "%BENCH_LABEL%" --binary "%BENCH_EXE%" --jobs 16
if errorlevel 1 goto failed
python -B -m tools.autoplay_benchmark compare --suite "%BENCH_STAGE%" --run-dir "%BENCH_DIR%" --output "%BENCH_DIR%\vs-v41.csv"
if errorlevel 1 goto failed
if /i "%BENCH_STAGE%"=="dev" goto gate
goto done

:gate
python -B -m tools.autoplay_acceptance --run-dir "%BENCH_DIR%" --output "%BENCH_DIR%\dev-gate.json"
if errorlevel 1 (
    if not exist "%BENCH_DIR%\dev-gate.json" goto failed
    echo Dev gate failed. See "%BENCH_DIR%\dev-gate.json".
    popd
    pause
    exit /b 2
)
goto done

:check
call :check_setup
if errorlevel 1 goto failed
python -B -m tools.autoplay_benchmark run --help >nul
if errorlevel 1 goto failed
python -B -m tools.autoplay_acceptance --help >nul
if errorlevel 1 goto failed
echo Benchmark setup OK.
popd
exit /b 0

:check_setup
python -B -c "import tomllib" >nul 2>&1
if errorlevel 1 (
    echo Python 3.11 or later is required.
    exit /b 1
)
if not defined RA2_DIR (
    for /f "delims=" %%I in ('python -B -c "from pathlib import Path; from tools.prepare_checkout import configured_ra2_dir; print(configured_ra2_dir(Path('config.toml'))[0] or '')"') do set "RA2_DIR=%%I"
)
if not exist "%RA2_DIR%\ra2md.mix" (
    echo Set RA2_DIR or update the ra2_dir setting in config.toml.
    exit /b 1
)
if not exist "autoplay\benchmarks\v41.json" (
    echo Could not find autoplay\benchmarks\v41.json in the worktree.
    exit /b 1
)
exit /b 0

:done
echo Results saved in "%BENCH_DIR%".
popd
pause
exit /b 0

:failed
echo Benchmark could not finish. See the error above.
if defined BENCH_DIR echo Run directory: "%BENCH_DIR%"
popd
if /i not "%BENCH_STAGE%"=="check" pause
exit /b 1
