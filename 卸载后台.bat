@echo off
chcp 65001 >nul
cd /d "%~dp0"
title 卸载 trit-agent 后台守护

echo.
echo   卸载后台守护
echo   -----------
echo   会做三件事:
echo     1. 删掉开机自启项
echo     2. 停掉正在跑的守护进程
echo     3. 删掉安装目录 %LOCALAPPDATA%\trit-agent
echo.
echo   不会动你的存档、数据包、配置（项目目录里的都保留）。
echo.

set DAEMON=target\release\trit-agent-daemon.exe
if not exist "%DAEMON%" (
    echo   没找到 %DAEMON%，可能没装过。
    echo.
    pause
    exit /b 0
)

"%DAEMON%" --uninstall
echo.
echo   卸载完成。
echo.
pause
