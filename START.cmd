@echo off
setlocal
title ORPHEUS
cd /d "%~dp0"
where cargo >nul 2>nul || (
  echo Rust is not installed. Install it from https://rustup.rs, then start again.
  pause
  exit /b 1
)
rem Builds orpheus.exe and owwfd_relay.dll side by side.
cargo build --release --locked --workspace || goto failed
"target\release\orpheus.exe" play %*
if errorlevel 1 goto failed
exit /b 0
:failed
echo.
echo ORPHEUS did not start. Run "target\release\orpheus.exe doctor" to find the problem.
pause
exit /b 1
