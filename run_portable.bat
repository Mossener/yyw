@echo off
cd /d "%~dp0"
set PATH=%~dp0runtime\python;%~dp0runtime\python\Scripts;%PATH%
set PYTHONIOENCODING=utf-8
set PYTHONUTF8=
start "" "%~dp0yyw.exe"
