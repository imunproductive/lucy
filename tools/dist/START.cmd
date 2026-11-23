@echo off
setlocal
title Lucy
cd /d "%~dp0"
rem Without a command, lucy asks which mode to start; "START.cmd join --server 1.2.3.4:3724" skips the question.
"%~dp0lucy.exe" %*
if errorlevel 1 goto failed
exit /b 0
:failed
echo.
echo Lucy did not start. Run "lucy.exe doctor" in this folder to find the problem.
pause
exit /b 1
