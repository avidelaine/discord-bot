@echo off
rem Starts Bobby's three parts, each in its own window.
cd /d "%~dp0"
set PYTHONIOENCODING=utf-8
start "Bobby - LLM" _bin\llama\llama-server.exe -m models\Qwen3-4B-Instruct-2507-Q4_K_M.gguf -ngl 99 -c 4096 -np 1 -fa on --host 127.0.0.1 --port 8080
start "Bobby - voice" voice\.venv\Scripts\python.exe voice\server.py
start "Bobby - bot" cmd /k "cd bot && node bot.js"
