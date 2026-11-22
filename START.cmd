@echo off
setlocal
title Lucy
cd /d "%~dp0"
where cargo >nul 2>nul || (
  echo Rust is not installed. Install it from https://rustup.rs, then start again.
  pause
  exit /b 1
)
rem Builds lucy.exe and owwfd_relay.dll side by side.
cargo build --release --locked --workspace || goto failed
rem Without a command, lucy asks which mode to start; "START.cmd join --server 1.2.3.4:3724" skips the question.
"target\release\lucy.exe" %*
if errorlevel 1 goto failed
exit /b 0
:failed
echo.
echo Lucy did not start. Run "target\release\lucy.exe doctor" to find the problem.
pause
exit /b 1
