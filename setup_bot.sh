#!/bin/bash

echo "========================================"
echo "  InkPads Bot Setup Utility (Linux)"
echo "========================================"

# 1. Check for Python
if ! command -v python3 &> /dev/null
then
    echo "[ERROR] Python 3 not found. Please install Python 3.8+."
    exit 1
fi

# 2. Install Dependencies
echo "[1/3] Installing Python dependencies..."
python3 -m pip install -r inkpads-bot/requirements.txt
if [ $? -ne 0 ]; then
    echo "[ERROR] Failed to install dependencies."
    exit 1
fi

# 3. Initialize .env
echo "[2/3] Initializing configuration..."
if [ ! -f inkpads-bot/.env ]; then
    cp inkpads-bot/.env.example inkpads-bot/.env
    echo "[SUCCESS] Created inkpads-bot/.env from template."
else
    echo "[INFO] inkpads-bot/.env already exists. Skipping."
fi

# 4. Check for Renderer
echo "[3/3] Checking for renderer binary..."
if [ -f target/release/minimap_renderer ]; then
    echo "[INFO] Found renderer in target/release."
elif [ -f inkpads-bot/minimap_renderer ]; then
    echo "[INFO] Found renderer in bot folder."
else
    echo "[WARNING] No renderer binary found. You will need to compile the project"
    echo "          using 'cargo build --release' or place the minimap_renderer binary"
    echo "          inside the inkpads-bot folder before running."
fi

echo ""
echo "========================================"
echo "  Setup Complete!"
echo "========================================"
echo ""
echo "1. Edit 'inkpads-bot/.env' and add your DISCORD_TOKEN."
echo "2. Run the bot with: python3 inkpads-bot/bot.py"
echo ""
