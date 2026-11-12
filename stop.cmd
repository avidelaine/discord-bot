@echo off
rem Stops Bobby: the start.js process tree (if running) and the three windows of start.cmd.
cd /d "%~dp0"
if exist _logs\bobby.pid (
  set /p PID=<_logs\bobby.pid
  setlocal enabledelayedexpansion
  echo Stopping start.js ^(pid !PID!^)
  rem only if that pid is still a node process: a stale pid file must not kill an unrelated program
  taskkill /T /F /FI "PID eq !PID!" /FI "IMAGENAME eq node.exe"
  del _logs\bobby.pid
  endlocal
) else echo No start.js running ^(no pid file^)
echo Closing start.cmd windows "Bobby - *"
taskkill /FI "WINDOWTITLE eq Bobby - *" /T /F
