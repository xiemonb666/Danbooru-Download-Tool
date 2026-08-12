@echo off
setlocal EnableExtensions
cd /d "%~dp0"

for /f "usebackq delims=" %%a in (`wsl wslpath -u "%CD%\stop_vllm.sh"`) do set "WSL_SCRIPT=%%a"
if not defined WSL_SCRIPT (
  echo [ERROR] Unable to locate stop_vllm.sh in WSL2. 1>&2
  exit /b 1
)

wsl -u root bash "%WSL_SCRIPT%"
exit /b %errorlevel%
