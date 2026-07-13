@echo off
REM Build the Rust kessel_core cdylib with CUDA-accelerated llama.cpp on Windows.
REM
REM Requires: VS Build Tools (MSVC + Ninja), an up-to-date rustup MSVC toolchain,
REM and an NVIDIA CUDA Toolkit. NOTE: CUDA 13+ dropped Pascal (GTX 10xx, sm_61),
REM so this pins a CUDA 12.x toolkit and targets sm_61 by default.
REM
REM Override per machine:
REM   set CUDA_VER=v12.9                  (toolkit folder under the CUDA install)
REM   set CUDA_ARCH=61                    (GPU compute capability; 1060 = 61)
REM
REM Usage:  scripts\build-win-cuda.bat
REM Output: crates\target\release\kessel_core.dll  (CUDA-enabled cdylib, for kessel.exe)
REM         crates\target\release\kessel-cli.exe   (CUDA-enabled Rust CLI: REPL + app-server)

setlocal

if "%CUDA_VER%"==""  set "CUDA_VER=v12.9"
if "%CUDA_ARCH%"=="" set "CUDA_ARCH=61"

set "CUDA_ROOT=%ProgramFiles%\NVIDIA GPU Computing Toolkit\CUDA\%CUDA_VER%"
if not exist "%CUDA_ROOT%\bin\nvcc.exe" (
  echo ERROR: nvcc not found at "%CUDA_ROOT%\bin\nvcc.exe"
  echo Set CUDA_VER to an installed Pascal-capable toolkit ^(e.g. v12.9^).
  exit /b 1
)

REM Enter the MSVC x64 dev environment (cl.exe is nvcc's host compiler).
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
for /f "usebackq delims=" %%i in (`"%VSWHERE%" -products * -all -latest -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VSINSTALL=%%i"
if not defined VSINSTALL (
  echo ERROR: Visual Studio C++ tools not found.
  exit /b 1
)
call "%VSINSTALL%\VC\Auxiliary\Build\vcvars64.bat" || exit /b 1

REM Point CUDA at the chosen toolkit and put nvcc on PATH.
set "CUDA_PATH=%CUDA_ROOT%"
set "CUDA_PATH_%CUDA_VER:.=_%=%CUDA_ROOT%"

REM Ninja generator + bundled ninja; rustup's MSVC cargo (not chocolatey GNU).
set "CMAKE_GENERATOR=Ninja"
set "PATH=%USERPROFILE%\.cargo\bin;%CUDA_ROOT%\bin;%VSINSTALL%\Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja;%PATH%"

REM Build CUDA kernels only for the target GPU (much faster than the full list).
set "CMAKE_CUDA_ARCHITECTURES=%CUDA_ARCH%"
REM Tolerate an MSVC newer than the toolkit officially lists.
set "NVCC_APPEND_FLAGS=-allow-unsupported-compiler"

echo Building with CUDA %CUDA_VER% for sm_%CUDA_ARCH% ...
cd /d "%~dp0..\crates" || exit /b 1
REM Build the cdylib and the Rust CLI in ONE invocation with the same feature.
REM Building them separately would resolve kessel-core with `local` for the CLI
REM (its default) and overwrite the CUDA kessel_core.dll with a CPU one.
cargo build --release -p kessel-core -p kessel-cli --no-default-features --features kessel-core/cuda,kessel-cli/cuda
exit /b %ERRORLEVEL%
