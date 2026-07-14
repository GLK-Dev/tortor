@echo off
title TorTor Build Script
echo =========================================
echo       TorTor Auto Compiler
echo =========================================
echo.
echo Select build type:
echo 1. Beta / Debug (Fast compile, slow execution)
echo 2. Release (Slow compile, fast execution)
echo.
set /p choice=Enter choice (1 or 2): 

if "%choice%"=="1" goto beta
if "%choice%"=="2" goto release
goto invalid

:beta
echo.
echo Building Beta (Debug)...
cargo build
echo.
echo Beta build complete! 
echo Executable is located at: target\debug\tortor.exe
echo.
pause
exit /b

:release
echo.
echo Building Release...
cargo build --release
echo.
echo Release build complete! 
echo Executable is located at: target\release\tortor.exe
echo.
pause
exit /b

:invalid
echo.
echo Invalid choice. Exiting...
echo.
pause
exit /b