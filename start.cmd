@echo off
rem Lance les trois briques de Bobby, chacune dans sa fenêtre.
cd /d "%~dp0"
set PYTHONIOENCODING=utf-8
start "Bobby - LLM" _bin\llama\llama-server.exe -m models\Qwen3-4B-Instruct-2507-Q4_K_M.gguf -ngl 99 -c 4096 -np 1 -fa on --host 127.0.0.1 --port 8080
start "Bobby - voix" voice\.venv\Scripts\python.exe voice\server.py
start "Bobby - bot" cmd /k "cd bot && node bot.js"
