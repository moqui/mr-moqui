@echo off
rem moq on Windows: Git Bash runs the moq script. PowerShell and cmd use this launcher.
setlocal
set "BASH="
if exist "%ProgramFiles%\Git\bin\bash.exe" set "BASH=%ProgramFiles%\Git\bin\bash.exe"
if not defined BASH if exist "%ProgramFiles%\Git\usr\bin\bash.exe" set "BASH=%ProgramFiles%\Git\usr\bin\bash.exe"
if not defined BASH if exist "%LocalAppData%\Programs\Git\bin\bash.exe" set "BASH=%LocalAppData%\Programs\Git\bin\bash.exe"
if not defined BASH (
    echo Git for Windows is required to run moq. Install it from https://git-scm.com/download/win
    exit /b 1
)
"%BASH%" "%~dp0moq" %*
