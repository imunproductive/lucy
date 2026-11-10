@echo off
setlocal
title ORPHEUS server
cd /d "%~dp0"
where cargo >nul 2>nul || (
  echo Rust is not installed. Install it from https://rustup.rs, then start again.
  pause
  exit /b 1
)
cargo build --release --locked -p orpheus || goto failed
"target\release\orpheus.exe" serve %*
if errorlevel 1 goto failed
exit /b 0
:failed
echo.
echo The server did not start. Run "target\release\orpheus.exe doctor" to find the problem.
pause
exit /b 1
