# InkPads Tactical Bot

A Discord bot to receive `.wowsreplay` files from users, and return high-quality single or dual-render video outputs utilizing the renderer and analysis engine.

## Features
- **`/render`**: Upload a replay to generate a tactical MP4 video.
- **Dual-Replay Sync**: Upload a second replay from the opposing team to generate a unified "Spectator View".
- **Customizable**: Toggle ship movement trails and detection/weapon ranges.
- **CPU/GPU Modes**: High-speed GPU encoding by default, with a CPU fallback for VPS environments.

## Setup Instructions

### 1. Requirements
- **Python 3.8+**
- **FFmpeg** (Must be in your system PATH)
- **World of Warships** installation (for game assets) | Curtailed snippet of game assets are WiP

### 2. Configuration
1. Copy `.env.example` to `.env`.
2. Edit `.env` and provide your **DISCORD_TOKEN**.
3. (Optional) Update `WOWS_PATH` if your game is installed in a non-standard location.

### 3. Installation
Run the setup script from the root directory or install manually:
```bash
pip install -r requirements.txt
```

### 4. Running the Bot
```bash
python bot.py
```

## How to use in Discord
Once the bot is running and invited to your server:
1. Type `/render`.
2. Attach your `.wowsreplay` file.
3. (Optional) Attach a second replay for dual-view.
4. Set options like `show_trails` to `True` if desired.
