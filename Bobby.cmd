@echo off
rem Opens Bobby's control app: starts the app server (hidden) if needed, then shows the UI in its own Edge window.
rem Closing the window does not stop Bobby or the server; stop.cmd stops both.
rem ponytail: port 4100 is hard-coded here (APP_PORT only applies to node app/server.js).
cd /d "%~dp0"
powershell -NoProfile -Command "$u='http://127.0.0.1:4100/api/status'; try { Invoke-WebRequest $u -UseBasicParsing -TimeoutSec 1 | Out-Null } catch { Start-Process node -ArgumentList 'app/server.js' -WorkingDirectory '%~dp0' -WindowStyle Hidden; for ($i=0; $i -lt 25; $i++) { Start-Sleep -Milliseconds 200; try { Invoke-WebRequest $u -UseBasicParsing -TimeoutSec 1 | Out-Null; break } catch {} } }"
set "EDGE=%ProgramFiles(x86)%\Microsoft\Edge\Application\msedge.exe"
if exist "%EDGE%" ( start "" "%EDGE%" --app=http://127.0.0.1:4100 --window-size=1280,860 ) else ( start "" http://127.0.0.1:4100 )
