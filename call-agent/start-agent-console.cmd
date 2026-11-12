@echo off
rem 启动悦悦 LiveKit Agent worker（前台控制台输出，Ctrl+C 退出）
setlocal
cd /d %~dp0
.venv\Scripts\python.exe agent_main.py start
