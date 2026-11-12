@echo off
rem Starts Bobby with no window at all (logs in _logs\bobby.log). Stop it with stop.cmd.
rem ponytail: double-clicking flashes this cmd window for an instant; use a shortcut set to "Minimized" to avoid even that.
powershell -NoProfile -WindowStyle Hidden -Command "Start-Process node -ArgumentList 'start.js','--headless' -WorkingDirectory '%~dp0' -WindowStyle Hidden"
