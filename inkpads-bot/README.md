# InkPads Minimap Renderer Discord Bot

A Discord bot to receive `.wowsreplay` files from users, and return high-quality single or dual-render video outputs utilizing the renderer and analysis engine.

## Features
- **`/render`**: Upload a replay to generate a tactical MP4 video.
- **Dual-Replay Sync**: Upload a second replay from the opposing team to generate a unified "Spectator View".
- **Customizable**: Toggle ship movement trails and detection/weapon ranges.
- **CPU/GPU Modes**: High-speed GPU encoding by default, with a CPU fallback for VPS environments.

## Setup Instructions

### 1. Requirements
- **Python 3.8+**
- **FFmpeg** (Must be in your system PATH/installed via package manager on VPS)
- **World of Warships** installation (for game assets)

### 2. Configuration
1. Copy `.env.example` to `.env`.
2. Edit `.env` and configure the following variables:
   - **`DISCORD_TOKEN`**: Your Discord bot token.
   - **`WOWS_PATH`**: Path to the World of Warships directory (for local rendering using the game assets).
   - **`WOWS_EXTRACTED_DIR`**: Optional. Path to the pre-extracted game asset directory. Useful for VPS environments with storage constraints to avoid mounting the full game directory.
   - **`RENDERER_FONT_PATH`**: Optional. Path to a custom `.ttf` font file to override the primary font face (e.g. `WarHeliosCondCBold.ttf`).
   - **`FORCE_CPU`**: Optional. Set to `true` to force software CPU encoding instead of GPU encoding. Essential for VPS hosts that lack a dedicated GPU.
   - **`RENDERER_CODEC`**: Optional. Override the video encoder codec (options: `h264`, `h265`, `av1`). Defaults to `h264` when `FORCE_CPU` is enabled, and `h265` otherwise.

### 3. Installation & Run

#### On Windows (Local Dev):
1. Run `setup_bot.bat` from the root directory to initialize the environment and install dependencies.
2. Build the renderer:
   ```cmd
   build_renderer.bat
   ```
3. Run the bot:
   ```cmd
   python inkpads-bot/bot.py
   ```

#### On Linux (VPS Deployment):
1. Initialize the setup script:
   ```bash
   chmod +x setup_bot.sh
   ./setup_bot.sh
   ```
2. Build the renderer:
   ```bash
   chmod +x build_renderer.sh
   ./build_renderer.sh
   ```
3. Run the bot:
   ```bash
   python3 inkpads-bot/bot.py
   ```

### 4. Updating the Bot & Renderer (VPS/Linux)
To update the repository, rebuild the renderer (which will automatically compile and copy the new binary to the bot directory), and restart the bot service:
1. Pull the latest changes:
   ```bash
   git pull
   ```
2. Build the renderer:
   ```bash
   ./build_renderer.sh
   ```
3. Restart the systemd service:
   ```bash
   sudo systemctl restart inkpads-bot.service
   ```

### 5. Running as a Daemon (systemd on VPS)
To keep the bot running persistently on a Linux VPS, use the provided `inkpads-bot.service` template:
1. Copy the service template to the systemd folder:
   ```bash
   sudo cp inkpads-bot/inkpads-bot.service /etc/systemd/system/inkpads-bot.service
   ```
2. Edit `/etc/systemd/system/inkpads-bot.service` to update the paths, user, and group for your VPS.
3. Reload systemd, enable, and start the service:
   ```bash
   sudo systemctl daemon-reload
   sudo systemctl enable inkpads-bot.service
   sudo systemctl start inkpads-bot.service
   ```
4. Monitor logs with:
   ```bash
   journalctl -u inkpads-bot.service -f
   ```

## How to use in Discord
Once the bot is running and invited to your server:
1. Type `/render`.
2. Attach your `.wowsreplay` file.
3. (Optional) Attach a second replay for dual-view.
4. Set options like `show_trails` to `True` if desired.
