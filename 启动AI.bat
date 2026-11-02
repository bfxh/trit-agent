@echo off
chcp 65001 >nul
cd /d "%~dp0"
title trit-agent - 前台模式（临时用）

echo.
echo   AI 控制台（前台模式）
echo   -------------------
echo   F8   开关 AI —— 按一下接管 / 再按一下交还
echo   F9   急停              F10  切视角
echo   F6/F7 单关攻击/移动
echo.

REM ---- 后台守护优先 ----
REM 守护进程（开机自启）已经在跑的话，就别开第二个 —— 两个进程同时
REM 轮询 F8 会互相抵消（按一次被切两次 = 看起来毫无反应）。
target\release\trit-agent.exe status 2>nul | findstr /C:"运行中" >nul
if not errorlevel 1 (
    echo   [提示] 后台守护进程已经在运行。
    echo          直接进游戏按 F8 就行，不用开这个窗口。
    echo.
    echo   想用前台模式请先停掉守护:
    echo       target\release\trit-agent.exe stop
    echo.
    pause
    exit /b 0
)

echo   [预检] 正在检查窗口和传感器...
echo.
target\release\trit-agent.exe probe
echo.

REM ---- 数据包启用检查 ----
REM 新世界 / 新建的存档，数据包常常是「拷进去了但没启用」——
REM 表现为传感器一条数据都不吐、AI 永远 hp=?、角色站桩不动。
REM 游戏没开时直接帮用户改 level.dat；游戏开着就提示敲命令。
if exist scripts\fix_world.py (
    python scripts\fix_world.py
    echo.
)

target\release\trit-agent.exe run
echo.
echo   AI 已退出。
pause
