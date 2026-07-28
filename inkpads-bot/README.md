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
   - **`FORCE_CPU`**: Optional. Set to `true` to force software CPU encoding instead of GPU encoding. Essential for VPS hosts that lack a dedicated GPU. Note: Enabling this will automatically hide/disable the `cpu_mode` option in the `/render` slash command, as CPU encoding is forced globally.
   - **`RENDERER_CODEC`**: Optional. Override the video encoder codec (options: `h264`, `h265`, `av1`). Defaults to `h264` when `FORCE_CPU` is enabled, and `h265` otherwise.
   - **`ENABLE_INKPADS_LAYOUT`**: Optional. Set to `true` to enable extended layout preset options (e.g. `C: InkPads`) in `/render` command choices. Defaults to `false`.
   - **`TOURNAMENT_LISTEN_CHANNEL_ID`**: Optional. Discord Channel ID of the restricted hidden channel where `wows-tournaments.com` posts match render request payloads.

## WoWs-Tournaments Integration Workflow

The bot features automated integration with `wows-tournaments.com` for processing automated match replay render requests.

### Architecture Overview

```
 ┌──────────────────────┐          ┌──────────────────────┐          ┌──────────────────────┐
 │ wows-tournaments.com │ ───────> │  Hidden Discord Ch.  │ ───────> │  InkPads Render Bot  │
 └──────────────────────┘          └──────────────────────┘          └──────────┬───────────┘
            ▲                                                                   │
            │                         HTTP Callback                             │ Post Video
            └───────────────────────────────────────────────────────────────────┤ to Target Channel
                                     { messageId, channelId }                   ▼
                                                                     ┌──────────────────────┐
                                                                     │ Public Discord Ch.   │
                                                                     └──────────────────────┘
```

### 1. Payload Format
The website posts a JSON payload (either formatted raw, enclosed in backticks, or uploaded as a `.json` file attachment) into the configured `TOURNAMENT_LISTEN_CHANNEL_ID`:

```json
{
  "callbackUrl": "https://wows-tournaments.com/api/matches/set-render-message?secret=<SECRET>",
  "targetChannelId": "1095389535510208512",
  "replays": [
    {
      "tag": "TA",
      "replay": "https://wows-tournaments.com/api/matches/report/replay?id=7"
    },
    {
      "tag": "TB",
      "replay": "https://wows-tournaments.com/api/matches/report/replay?id=8"
    }
  ]
}
```

### 2. Processing Pipeline
1. **Listener & Ingestion**: The `on_message` handler in `bot.py` filters messages targeting `TOURNAMENT_LISTEN_CHANNEL_ID`, cleans formatting wrappers, and extracts the payload.
2. **Replay Acquisition**: Downloads primary (`TA` / Green) and secondary (`TB` / Red) team replays asynchronously to temporary workspace storage.
3. **Rendering**: Invokes the `minimap_renderer` CLI binary with dual-replay flags (`--red-replay`) to produce a unified match MP4 video.
4. **Discord Publication**: Uploads the resulting MP4 video along with match metadata (map, game mode, date/time, opponent clan) to `targetChannelId`.
5. **Callback Acknowledgment**: Performs an HTTP `POST` request to `callbackUrl` returning the published Discord message details:
   ```json
   {
     "messageId": "<DISCORD_MESSAGE_ID>",
     "channelId": "<TARGET_CHANNEL_ID>"
   }
   ```



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

### 5. Docker Deployment
As an alternative to systemd, the repository includes a multi-stage `Dockerfile` and `docker-compose.yml` for automated deployment:
1. Ensure Docker and Docker Compose are installed on the host.
2. Build and launch the bot container:
   ```bash
   docker compose up -d --build
   ```

### 6. Running as a Daemon (systemd on VPS)

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

### 6. Asset Extraction & File Path Case Sensitivity (Linux VPS Note)
When deploying pre-extracted game asset directories (`WOWS_EXTRACTED_DIR`) on Linux hosts with case-sensitive filesystems (e.g., ext4, xfs):
- Ensure that extracted asset directory trees maintain standard Wargaming internal casing (e.g., `gui/dogTags/` or `gui/dogtags/`).
- Player dog tag and emblem assets (`DT_Default.png`) are automatically looked up across standard path variants (`gui/dogTags/`, `gui/dog_tags/`, `gui/dogtags/`). If player emblems or dog tags fail to resolve on a Linux host, verify that the extraction tool did not alter folder or filename casing during extraction.

## How to use in Discord
Once the bot is running and invited to your server:
1. Type `/render`.
2. Attach your `.wowsreplay` file.
3. (Optional) Attach a second replay for dual-view.
4. Set options like `show_trails` to `True` if desired.
