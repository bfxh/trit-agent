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
target\release\trit-agent.exe run
echo.
echo   AI 已退出。
pause
