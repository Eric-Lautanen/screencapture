@echo off
setlocal
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0fetch-ffmpeg.ps1" %*
exit /b %ERRORLEVEL%
