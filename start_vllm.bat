@echo off
chcp 65001 >nul
setlocal

if not defined VLLM_PORT set "VLLM_PORT=8000"
if not defined VLLM_HOST set "VLLM_HOST=127.0.0.1"

set "SCRIPT=start_vllm.sh"
if "%~1"=="diffusiongemma" set "SCRIPT=start_vllm_diffusiongemma.sh"

cd /d "%~dp0"

for /f "usebackq delims=" %%a in (`wsl wslpath -u "%CD%\%SCRIPT%"`) do set "WSL_SCRIPT=%%a"

echo [INFO] Starting vLLM via WSL2: %SCRIPT%
echo [INFO] Press Ctrl+C to stop
echo [INFO] Linux launcher writes timestamped logs under the project logs directory
echo.

wsl -u root env "VLLM_PORT=%VLLM_PORT%" "VLLM_HOST=%VLLM_HOST%" "MODEL_PATH=%MODEL_PATH%" bash "%WSL_SCRIPT%"
set "EXIT_CODE=%errorlevel%"

if %EXIT_CODE% neq 0 (
    echo.
    echo [ERROR] vLLM exited with code %EXIT_CODE%.
    pause
)
