@echo off
rem Starts Bobby's three parts, each in its own minimized window (nothing pops up over a game).
cd /d "%~dp0"
set PYTHONIOENCODING=utf-8
start "Bobby - LLM" /min _bin\llama\llama-server.exe -m models\Qwen3-4B-Instruct-2507-Q4_K_M.gguf -ngl 99 -c 4096 -np 1 -fa on --host 127.0.0.1 --port 8080
start "Bobby - voice" /min voice\.venv\Scripts\python.exe voice\server.py
start "Bobby - bot" /min cmd /k "cd bot && node bot.js"
