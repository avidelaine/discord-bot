@echo off
rem Stops Bobby: the start.js process tree (if running) and the three windows of start.cmd.
cd /d "%~dp0"
if exist _logs\bobby.pid (
  set /p PID=<_logs\bobby.pid
  setlocal enabledelayedexpansion
  echo Stopping start.js ^(pid !PID!^)
  taskkill /T /F /PID !PID!
  del _logs\bobby.pid
  endlocal
) else echo No start.js running ^(no pid file^)
echo Closing start.cmd windows "Bobby - *"
taskkill /FI "WINDOWTITLE eq Bobby - *" /T /F
