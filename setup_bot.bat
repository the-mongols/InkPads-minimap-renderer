@echo off
SETLOCAL EnableDelayedExpansion

echo ========================================
echo   InkPads Bot Setup Utility
echo ========================================

:: 1. Check for Python
python --version >nul 2>&1
if %errorlevel% neq 0 (
    echo [ERROR] Python not found. Please install Python 3.8+ and add it to your PATH.
    pause
    exit /b 1
)

:: 2. Install Dependencies
echo [1/3] Installing Python dependencies...
pip install -r inkpads-bot\requirements.txt
if %errorlevel% neq 0 (
    echo [ERROR] Failed to install dependencies.
    pause
    exit /b 1
)

:: 3. Initialize .env
echo [2/3] Initializing configuration...
if not exist inkpads-bot\.env (
    copy inkpads-bot\.env.example inkpads-bot\.env
    echo [SUCCESS] Created inkpads-bot\.env from template.
) else (
    echo [INFO] inkpads-bot\.env already exists. Skipping.
)

:: 4. Check for Renderer
echo [3/3] Checking for renderer binary...
if exist target\release\minimap_renderer.exe (
    echo [INFO] Found renderer in target\release.
) else if exist inkpads-bot\minimap_renderer.exe (
    echo [INFO] Found renderer in bot folder.
) else (
    echo [WARNING] No renderer binary found. You will need to compile the project
    echo           using 'cargo build --release' or place minimap_renderer.exe
    echo           inside the inkpads-bot folder before running.
)

echo.
echo ========================================
echo   Setup Complete!
echo ========================================
echo.
echo 1. Edit 'inkpads-bot\.env' and add your DISCORD_TOKEN.
echo 2. Run the bot with: python inkpads-bot\bot.py
echo.
pause
