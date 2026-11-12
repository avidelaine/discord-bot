@echo off
rem Starts Bobby's three parts, each in its own minimized window (nothing pops up over a game).
cd /d "%~dp0"
set PYTHONIOENCODING=utf-8
rem Each part restarts by itself 3 s after a crash. The LLM logs to _logs\llm.log.
start "Bobby - LLM" /min cmd /k "for /l %%i in (0,0,1) do (_bin\llama\llama-server.exe -m models\Qwen3-4B-Instruct-2507-Q4_K_M.gguf -ngl 99 -c 4096 -np 1 -fa on --host 127.0.0.1 --port 8080 --log-file _logs\llm.log & echo LLM stopped, restarting in 3 s & timeout /t 3 >nul)"
start "Bobby - voice" /min cmd /k "for /l %%i in (0,0,1) do (voice\.venv\Scripts\python.exe voice\server.py & echo Voice service stopped, restarting in 3 s & timeout /t 3 >nul)"
start "Bobby - bot" /min cmd /k "cd bot && for /l %%i in (0,0,1) do (node bot.js & echo Bot stopped, restarting in 3 s & timeout /t 3 >nul)"
