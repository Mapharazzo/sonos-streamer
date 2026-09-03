@echo off
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0start-sonos-tab-caster.ps1"
if errorlevel 1 pause
