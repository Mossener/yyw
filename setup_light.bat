@echo off
setlocal
cd /d "%~dp0"

echo ========================================
echo   Stem Studio - light setup
echo   Requires: Python 3.10+ on PATH
echo ========================================
echo.

echo [1/3] Installing Demucs + torchcodec...
pip install demucs torchcodec --quiet
if errorlevel 1 (
    echo ERROR: pip install failed. Is Python on PATH?
    pause
    exit /b 1
)
echo OK.

echo.
echo [2/3] Downloading FFmpeg DLLs...
for /f "tokens=*" %%a in ('python -c "import torchcodec; print(torchcodec.__path__[0])"') do set TCDIR=%%a

if exist "%TCDIR%\avcodec-62.dll" (
    echo FFmpeg DLLs already present, skip.
) else (
    if not exist "runtime\7za.exe" (
        mkdir runtime 2>nul
        curl -L -o "runtime\7za.zip" "https://www.7-zip.org/a/7za920.zip"
        powershell -Command "Expand-Archive -LiteralPath 'runtime\7za.zip' -DestinationPath 'runtime' -Force"
        del "runtime\7za.zip"
    )
    curl -L -o "runtime\ffmpeg.7z" "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-full-shared.7z"
    runtime\7za.exe x "runtime\ffmpeg.7z" -o"runtime\ffmpeg" -y >nul
    for /d %%d in (runtime\ffmpeg\*) do (
        if exist "%%d\bin\avcodec-*.dll" (
            copy /y "%%d\bin\av*.dll" "%TCDIR%\" >nul
            copy /y "%%d\bin\sw*.dll" "%TCDIR%\" >nul 2>nul
            copy /y "%%d\bin\*.dll" "tools\" >nul
        )
    )
    del "runtime\ffmpeg.7z"
    echo FFmpeg DLLs installed.
)

echo.
echo [3/3] Done.
echo Launch with: stem-studio.exe
echo Or: run_portable.bat
pause
