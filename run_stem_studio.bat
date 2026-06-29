@echo off
cd /d "%~dp0"

:: 激活 conda 环境
call D:\conda\Scripts\activate.bat D:\conda

:: UTF-8
set PYTHONIOENCODING=utf-8
set PYTHONUTF8=1

:: 启动
start "" "%~dp0stem-studio.exe"
