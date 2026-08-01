@echo off
setlocal
cd /d "%~dp0"

where npm >nul 2>nul || (echo [ERROR] npm not found & exit /b 1)
where cargo >nul 2>nul || (echo [ERROR] Rust cargo not found & exit /b 1)

if not exist frontend\dist\index.html (
  echo [INFO] Building frontend once...
  pushd frontend
  call npm ci --silent || (popd & exit /b 1)
  call npm run build || (popd & exit /b 1)
  popd
)

echo [INFO] Starting debug backend on http://127.0.0.1:8888
pushd backend
cargo run --locked
set EXIT_CODE=%ERRORLEVEL%
popd
exit /b %EXIT_CODE%
