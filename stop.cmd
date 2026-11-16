@echo off
rem Stops Bobby: the start.js process tree (if running), the three windows of start.cmd and the control app server.
rem "stop.cmd keep-app" leaves the app server alone (used by the app itself to stop a Bobby started elsewhere).
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
if /i "%1"=="keep-app" goto :eof
if exist _logs\app.pid (
  set /p APID=<_logs\app.pid
  setlocal enabledelayedexpansion
  echo Stopping app server ^(pid !APID!^)
  taskkill /T /F /FI "PID eq !APID!" /FI "IMAGENAME eq node.exe"
  del _logs\app.pid
  endlocal
)
