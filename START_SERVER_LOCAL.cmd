@echo off
setlocal
title Lucy server
cd /d "%~dp0"
where cargo >nul 2>nul || (
  echo Rust is not installed. Install it from https://rustup.rs, then start again.
  pause
  exit /b 1
)
cargo build --release --locked -p lucy || goto failed
"target\release\lucy.exe" serve %*
if errorlevel 1 goto failed
exit /b 0
:failed
echo.
echo The server did not start. Run "target\release\lucy.exe doctor" to find the problem.
pause
exit /b 1
