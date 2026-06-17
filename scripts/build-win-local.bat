@echo off
REM Build the Rust kessel_core cdylib with the `local` (in-process llama.cpp)
REM feature on Windows. Must run inside the MSVC environment so cmake/cc use
REM cl.exe + the Windows SDK instead of LLVM clang.
REM
REM Usage:  scripts\build-win-local.bat
REM Output: crates\target\release\kessel_core.dll

setlocal

REM Locate the Visual Studio Build Tools install and enter its x64 dev env.
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
for /f "usebackq delims=" %%i in (`"%VSWHERE%" -products * -all -latest -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VSINSTALL=%%i"
if not defined VSINSTALL (
  echo ERROR: Visual Studio C++ tools not found. Install with:
  echo   choco install visualstudio2022-workload-vctools
  exit /b 1
)
call "%VSINSTALL%\VC\Auxiliary\Build\vcvars64.bat" || exit /b 1

REM Use the Ninja generator with the MSVC cl.exe from vcvars. Ninja is
REM single-config, so llama-cpp-sys finds its static libs (common.lib, etc.) in
REM the flat build dir -- the Visual Studio (multi-config) generator buries them
REM under Release\ and the build breaks with "could not find native static
REM library `common`". Ninja ships with VS but isn't on the vcvars PATH, so add it.
set "CMAKE_GENERATOR=Ninja"

REM Use rustup's MSVC toolchain, not chocolatey's GNU cargo (which is first on
REM PATH). The C++ libs are built with MSVC cl.exe (above), so rustc must also be
REM MSVC or it looks for libcommon.a instead of common.lib and the link fails.
REM Prepend ~/.cargo/bin so `cargo` resolves to the rustup shim (default host
REM x86_64-pc-windows-msvc), plus VS's bundled ninja.
set "PATH=%USERPROFILE%\.cargo\bin;%VSINSTALL%\Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja;%PATH%"

cd /d "%~dp0..\crates" || exit /b 1
cargo build --release -p kessel-core --features local
exit /b %ERRORLEVEL%
