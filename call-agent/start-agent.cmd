@echo off
rem 启动悦悦 LiveKit Agent worker（日志写入 logs\agent.out.log）
setlocal
set PYTHONUNBUFFERED=1
cd /d %~dp0
if not exist logs mkdir logs
.venv\Scripts\python.exe agent_main.py start >> logs\agent.out.log 2>&1
