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
rem Without a command, orpheus asks which mode to start; "START.cmd join --server 1.2.3.4:3724" skips the question.
"target\release\orpheus.exe" %*
if errorlevel 1 goto failed
exit /b 0
:failed
echo.
echo ORPHEUS did not start. Run "target\release\orpheus.exe doctor" to find the problem.
pause
exit /b 1
