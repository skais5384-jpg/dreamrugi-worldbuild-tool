@echo off
setlocal
set "BASE=%~dp0"
for /f "usebackq delims=" %%I in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VS=%%I"
if not defined VS exit /b 1
call "%VS%\VC\Auxiliary\Build\vcvars64.bat"
if errorlevel 1 exit /b 1
if not exist "%BASE%source\hunspell-1.7.3" tar -xf "%BASE%source\hunspell-1.7.3.tar.gz" -C "%BASE%source"
if errorlevel 1 exit /b 1
pushd "%BASE%source\hunspell-1.7.3"
cl /nologo /EHsc /std:c++17 /utf-8 /O2 /DHUNSPELL_STATIC /I src\hunspell /Fe:"%BASE%resources\hunspell-runner.exe" src\hunspell\*.cxx "%BASE%hunspell-runner.cpp"
set "STATUS=%ERRORLEVEL%"
popd
exit /b %STATUS%
