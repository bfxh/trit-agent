@echo off
chcp 65001 >nul
cd /d "%~dp0"
title trit-agent - 保持这个窗口开着，F8 才有反应

echo.
echo   AI 控制台
echo   ---------
echo   F8   交还 / 重新接管       F9   急停
echo   F10  切视角                F6/F7 单关攻击/移动
echo.
echo   关掉这个窗口 = AI 彻底停止（F8 也不会有反应）
echo.

REM ---- 预检：先跑一次 probe，把「能不能用」讲清楚再进主循环 ----
REM 这一步很值钱：以前「AI 不动」只能靠猜到底是没接管、没感知、
REM 还是数据包没启用。现在直接报。
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
