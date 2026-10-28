@echo off
rem ============================================================
rem  fantermo build wrapper (cmd side)
rem  Sets up the MSVC build environment (vcvars64), checks the Zig
rem  version required by libghostty-vt, then runs cargo.
rem  Usage: dev.bat build / dev.bat run / dev.bat test
rem  From git-bash, call via dev.sh.
rem ============================================================
setlocal
rem Zig version required by the libghostty-vt rev in Cargo.toml
set "ZIG_REQUIRED=0.17.0"

rem MSYS launch drops env vars whose name contains "(" -- restore it
set "ProgramFiles(x86)=C:\Program Files (x86)"
rem winget puts its command aliases here; shells started before an install miss it
set "PATH=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer;%LOCALAPPDATA%\Microsoft\WinGet\Links;%PATH%"

if not defined ZIG set "ZIG=zig"
set "ZIG_FOUND="
for /f "usebackq tokens=*" %%v in (`"%ZIG%" version 2^>nul`) do set "ZIG_FOUND=%%v"
if not "%ZIG_FOUND%"=="%ZIG_REQUIRED%" (
  echo [dev.bat] Zig %ZIG_REQUIRED% is required, found "%ZIG_FOUND%" ^(ZIG=%ZIG%^) 1>&2
  exit /b 1
)

rem Locate VS dynamically via vswhere (edition/version independent)
set "VSINSTALL="
for /f "usebackq tokens=*" %%i in (`vswhere -latest -products * -property installationPath 2^>nul`) do set "VSINSTALL=%%i"
if not defined VSINSTALL (
  echo [dev.bat] Visual Studio / Build Tools not found 1>&2
  exit /b 1
)

call "%VSINSTALL%\VC\Auxiliary\Build\vcvars64.bat" >nul
if errorlevel 1 (
  echo [dev.bat] vcvars64 init failed 1>&2
  exit /b 1
)

cargo %*
