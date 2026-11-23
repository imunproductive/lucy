@echo off
setlocal
title Lucy server
cd /d "%~dp0"
"%~dp0lucy.exe" serve %*
if errorlevel 1 goto failed
exit /b 0
:failed
echo.
echo The server did not start. Run "lucy.exe doctor" in this folder to find the problem.
pause
exit /b 1
