@echo off
setlocal EnableExtensions EnableDelayedExpansion
set "ROOT_DIR=%~dp0"
set "FRONTEND_DIR=%ROOT_DIR%frontend"
set "BACKEND_DIR=%ROOT_DIR%backend"
set "BACKEND_BIN=%BACKEND_DIR%\target\release\danbooru-download-tool-pro.exe"
set "LOCK_STAMP=%FRONTEND_DIR%\node_modules\.danbooru-launcher-package-lock.json"
cd /d "%ROOT_DIR%"

if not defined HOST set "HOST=127.0.0.1"
if not defined PORT set "PORT=8888"
if not defined DATA_DIR set "DATA_DIR=%ROOT_DIR%"
if not defined STATIC_DIR set "STATIC_DIR=%FRONTEND_DIR%\dist"
rem The application starts without allocating GPU memory. Set START_VLLM=1 only
rem for unattended launches; normal interactive loading is done from Settings.
if not defined START_VLLM set "START_VLLM=0"
if not defined VLLM_PORT set "VLLM_PORT=8000"
set "VLLM_PREFERRED_PORT=%VLLM_PORT%"
set "VLLM_STATE_FILE=%ROOT_DIR%logs\vllm.state.json"

where node >nul 2>nul || (echo [ERROR] Node.js not found & exit /b 1)
where npm >nul 2>nul || (echo [ERROR] npm not found & exit /b 1)
where cargo >nul 2>nul || (echo [ERROR] Rust cargo not found & exit /b 1)
node -e "const [major, minor] = process.versions.node.split('.').map(Number); const supported = (major === 20 && minor >= 19) || (major >= 22 && (major > 22 || minor >= 12)); process.exit(supported ? 0 : 1)" >nul 2>nul || (echo [ERROR] Node.js 20.19+ or 22.12+ is required & exit /b 1)

if "%START_VLLM%"=="0" (
  echo [WARN] vLLM auto-start disabled by START_VLLM=0
) else if not "%START_VLLM%"=="1" (
  echo [WARN] START_VLLM must be 0 or 1; skipping vLLM auto-start
) else (
  where wsl >nul 2>nul
  if errorlevel 1 (
    echo [WARN] WSL2 not found; continuing without vLLM
  ) else if not exist "%ROOT_DIR%start_vllm.bat" (
    echo [WARN] start_vllm.bat not found; continuing without vLLM
  ) else (
    set "VLLM_ACTION="
    set "VLLM_SELECTED_PORT="
    set "VLLM_PORT_STATE="
    for /f "tokens=1-3 delims=:" %%A in ('node "%ROOT_DIR%scripts/select-vllm-port.mjs" "%VLLM_PORT%" "%VLLM_STATE_FILE%"') do (
      set "VLLM_ACTION=%%A"
      set "VLLM_SELECTED_PORT=%%B"
      set "VLLM_PORT_STATE=%%C"
    )
    if not defined VLLM_SELECTED_PORT (
      echo [WARN] Unable to select a vLLM port; continuing without vLLM
    ) else (
      set "VLLM_PORT=!VLLM_SELECTED_PORT!"
      set "VLLM_BASE_URL=http://127.0.0.1:!VLLM_PORT!/v1"
      if "!VLLM_ACTION!"=="ready" (
        echo [OK] A compatible model server is ready at !VLLM_BASE_URL!
      ) else if "!VLLM_ACTION!"=="loading" (
        echo [OK] vLLM is already loading at !VLLM_BASE_URL!
      ) else (
        if "!VLLM_PORT_STATE!"=="conflict" echo [WARN] Port !VLLM_PREFERRED_PORT! belongs to another service; vLLM will use port !VLLM_PORT!
        echo [INFO] Starting vLLM in a visible window on port !VLLM_PORT!...
        start "Danbooru Tool vLLM" "%ComSpec%" /d /c call ""%ROOT_DIR%start_vllm.bat""
      )
    )
  )
)

set "NEED_NPM_CI=1"
if exist "%FRONTEND_DIR%\node_modules" if exist "%LOCK_STAMP%" (
  fc /b "%FRONTEND_DIR%\package-lock.json" "%LOCK_STAMP%" >nul 2>nul
  if not errorlevel 1 set "NEED_NPM_CI=0"
)

if "%NEED_NPM_CI%"=="1" (
  echo [INFO] Installing frontend dependencies...
  pushd "%FRONTEND_DIR%"
  call npm ci --silent || (popd & exit /b 1)
  copy /y package-lock.json "%LOCK_STAMP%" >nul || (popd & exit /b 1)
  popd
) else (
  echo [OK] Frontend dependencies are up to date
)

echo [INFO] Building frontend...
pushd "%FRONTEND_DIR%"
call npm run build || (popd & exit /b 1)
popd

echo [INFO] Building backend...
pushd "%BACKEND_DIR%"
cargo build --release --locked || (popd & exit /b 1)
echo [INFO] Starting http://%HOST%:%PORT%
"%BACKEND_BIN%"
set EXIT_CODE=%ERRORLEVEL%
popd
exit /b %EXIT_CODE%
