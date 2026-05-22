import discord
from discord import app_commands
from discord.ext import commands
import os
import asyncio
import uuid
import requests
import json
from pathlib import Path
from dotenv import load_dotenv
import logging

# Load configuration
load_dotenv()
TOKEN = os.getenv('DISCORD_TOKEN')
WOWS_PATH = os.getenv('WOWS_PATH', 'C:\\Games\\World_of_Warships')
# Default to a few likely locations for the renderer
RENDERER_PATH = os.getenv('RENDERER_PATH')

# Webhook Configuration (optional early handover)
WEBHOOK_URL = os.getenv("WEBHOOK_URL")
WEBHOOK_SECRET = os.getenv("WEBHOOK_SECRET")
CF_CLIENT_ID = os.getenv('CF_ACCESS_CLIENT_ID')
CF_CLIENT_SECRET = os.getenv('CF_ACCESS_CLIENT_SECRET')

def get_webhook_headers():
    headers = {}
    if WEBHOOK_SECRET:
        headers["X-Webhook-Secret"] = WEBHOOK_SECRET
    if CF_CLIENT_ID and CF_CLIENT_SECRET:
        headers["CF-Access-Client-Id"] = CF_CLIENT_ID
        headers["CF-Access-Client-Secret"] = CF_CLIENT_SECRET
    return headers

def find_renderer():
    if RENDERER_PATH:
        p = Path(RENDERER_PATH).resolve()
        if p.exists(): return p
    
    # Common locations relative to bot.py
    search_paths = [
        Path('minimap_renderer.exe'),
        Path('../target/release/minimap_renderer.exe'),
        Path('../minimap_renderer.exe'),
        Path('../scripts/minimap_renderer.exe')
    ]
    
    for p in search_paths:
        full_path = (Path(__file__).parent / p).resolve()
        if full_path.exists():
            return full_path
    return None

RENDERER_EXE = find_renderer()
GAME_DIR = Path(WOWS_PATH).resolve()

# Logging setup
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger('InkpadsBot')

# Setup workspace
TEMP_DIR = Path('temp').resolve()
TEMP_DIR.mkdir(exist_ok=True)

# Keep strong references to background tasks to prevent garbage collection
background_tasks = set()

class InkpadsBot(commands.Bot):
    def __init__(self):
        intents = discord.Intents.default()
        intents.message_content = True
        super().__init__(command_prefix="!", intents=intents)

    async def setup_hook(self):
        # Sync slash commands
        logger.info("Syncing slash commands...")
        synced = await self.tree.sync()
        logger.info(f"Synced {len(synced)} commands.")

bot = InkpadsBot()

@bot.event
async def on_ready():
    logger.info(f'--- InkPads Tactical Bot 2.0 Ready ---')
    logger.info(f'Logged in as: {bot.user}')
    logger.info(f'Renderer EXE: {RENDERER_EXE}')
    logger.info(f'---------------------------------------')
def parse_replay_header(file_path):
    import struct
    try:
        with open(file_path, 'rb') as f:
            magic = f.read(4)
            if len(magic) < 4 or magic != b'\x12\x32\x34\x11':
                return {}
            
            block_count_bytes = f.read(4)
            if len(block_count_bytes) < 4:
                return {}
            
            header_size_bytes = f.read(4)
            if len(header_size_bytes) < 4:
                return {}
                
            header_size = struct.unpack('I', header_size_bytes)[0]
            
            header_json_bytes = f.read(header_size)
            if len(header_json_bytes) < header_size:
                return {}
                
            header_text = header_json_bytes.decode('utf-8', errors='ignore')
            return json.loads(header_text)
    except Exception as e:
        logger.warning(f"Error parsing replay header: {e}")
        return {}

async def send_webhook_payload(replay_path, red_replay_path, session_id):
    if not WEBHOOK_URL:
        return
    
    max_retries = 3
    headers = get_webhook_headers()
    loop = asyncio.get_event_loop()
    
    for attempt in range(1, max_retries + 1):
        if not replay_path.exists():
            logger.error(f"Cannot upload payload for session {session_id}: replay file {replay_path} does not exist.")
            break
            
        try:
            logger.info(f"Webhook handover attempt {attempt}/{max_retries} for session {session_id}...")
            
            files = {}
            fh_to_close = []
            try:
                f_green = open(replay_path, 'rb')
                fh_to_close.append(f_green)
                files['file'] = (replay_path.name, f_green, 'application/octet-stream')
                
                if red_replay_path and red_replay_path.exists():
                    f_red = open(red_replay_path, 'rb')
                    fh_to_close.append(f_red)
                    files['red_file'] = (red_replay_path.name, f_red, 'application/octet-stream')
                
                def post_payload():
                    return requests.post(WEBHOOK_URL, files=files, headers=headers, timeout=60)
                
                resp = await loop.run_in_executor(None, post_payload)
            finally:
                for fh in fh_to_close:
                    try:
                        fh.close()
                    except Exception as ce:
                        logger.warning(f"Error closing file handle during webhook attempt: {ce}")
            
            if resp.status_code in (200, 201, 202):
                logger.info(f"Webhook handover successful on attempt {attempt} for session {session_id}.")
                return
            elif resp.status_code in (400, 401, 403, 404):
                logger.error(f"Webhook handover rejected with status {resp.status_code} (non-retryable) for session {session_id}: {resp.text}")
                break
            else:
                logger.warning(f"Webhook handover failed with status {resp.status_code} for session {session_id}.")
                
        except Exception as e:
            logger.error(f"Exception during webhook handover attempt {attempt} for session {session_id}: {e}")
        
        if attempt < max_retries:
            backoff = 2 ** attempt
            logger.info(f"Retrying webhook handover for session {session_id} in {backoff} seconds...")
            await asyncio.sleep(backoff)
            
    logger.error(f"Webhook handover completely failed after {max_retries} attempts for session {session_id}.")



@bot.tree.command(name="render", description="Render a WoWS replay into a tactical video")
@app_commands.describe(
    replay="The primary (Green) .wowsreplay file",
    red_replay="Optional secondary (Red) .wowsreplay file from the opposing team",
    show_trails="Display ship movement trails (heatmap)",
    show_config="Show detection and weapon range circles",
    cpu_mode="Use CPU encoding (slower, but safer if GPU is busy)",
    discord_layout="Optimize layout elements and statistics for Discord embeds (default: True)"
)
async def render(
    interaction: discord.Interaction, 
    replay: discord.Attachment,
    red_replay: discord.Attachment = None,
    show_trails: bool = False,
    show_config: bool = False,
    cpu_mode: bool = False,
    discord_layout: bool = True
):
    if not replay.filename.endswith('.wowsreplay'):
        await interaction.response.send_message("❌ Error: File must be a `.wowsreplay` file.", ephemeral=True)
        return

    # Acknowledge and defer since rendering takes time
    await interaction.response.defer(ephemeral=False)
    
    # Create unique session ID
    session_id = str(uuid.uuid4())[:8]
    replay_path = TEMP_DIR / f"{session_id}_green.wowsreplay"
    red_replay_path = TEMP_DIR / f"{session_id}_red.wowsreplay" if red_replay else None
    output_path = TEMP_DIR / f"{session_id}.mp4"

    logger.info(f"Render Session {session_id}: Green={replay.filename}, Red={'None' if not red_replay else red_replay.filename}")
    
    # Send initial status
    status_msg = f"🚀 **Replay Rendering Started**\nFile: `{replay.filename}`"
    if red_replay:
        status_msg += f"\nSync File: `{red_replay.filename}`"
    status_msg += "\nProcessing..."
    await interaction.followup.send(status_msg)

    webhook_task = None
    try:
        # 1. Download
        await replay.save(replay_path)
        if red_replay:
            await red_replay.save(red_replay_path)
        
        # 2. Early verification and Webhook handover
        is_clan_battle = False
        if WEBHOOK_URL:
            try:
                header = parse_replay_header(replay_path)
                match_group = str(header.get("matchGroup", "")).lower()
                game_type = str(header.get("gameType", "")).lower()
                
                # Check for clan battle match group or game type
                is_clan_battle = any(x in match_group or x in game_type for x in ("clan", "cvc", "cw"))
                
                logger.info(f"Early verification for session {session_id}: matchGroup={match_group}, gameType={game_type} -> is_clan_battle={is_clan_battle}")
                
                if is_clan_battle:
                    webhook_task = asyncio.create_task(send_webhook_payload(replay_path, red_replay_path, session_id))
            except Exception as e:
                logger.warning(f"Early verification/webhook launch failed for session {session_id}: {e}")

        # 3. Build CLI Command
        cmd = [
            str(RENDERER_EXE),
            "-g", str(GAME_DIR),
            "-o", str(output_path),
            str(replay_path)
        ]
        
        if red_replay:
            cmd.extend(["--red-replay", str(red_replay_path)])
            # Dual-renders are for tactical overview: hide subjective UI elements
            cmd.extend(["--no-chat", "--no-kill-feed", "--no-stats-panel"])
        
        if show_trails: cmd.append("--show-trails")
        if show_config: cmd.append("--show-ship-config")
        if cpu_mode: cmd.append("--cpu")
        if discord_layout: cmd.append("--discord-layout")

        logger.info(f"Executing: {' '.join(cmd)}")

        # 4. Render
        process = await asyncio.create_subprocess_exec(
            *cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE
        )

        stdout, stderr = await process.communicate()
        logger.info(f"Render Session {session_id}: Process exited with code {process.returncode}")
        if stdout: logger.info(f"STDOUT: {stdout.decode()}")
        if stderr: logger.info(f"STDERR: {stderr.decode()}")

        if process.returncode == 0:
            # 5. Check Size and Compress if needed
            file_size = output_path.stat().st_size
            MAX_SIZE = int(9.8 * 1024 * 1024) # 9.8MB safe limit
            
            if file_size > MAX_SIZE:
                logger.info(f"Render Session {session_id}: File too large ({file_size/1024/1024:.1f}MB), compressing...")
                compressed_path = TEMP_DIR / f"{session_id}_compressed.mp4"
                compress_cmd = [
                    "ffmpeg", "-y", "-i", str(output_path),
                    "-vcodec", "libx264", "-crf", "22", "-preset", "fast",
                    str(compressed_path)
                ]
                
                c_process = await asyncio.create_subprocess_exec(
                    *compress_cmd,
                    stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.PIPE
                )
                await c_process.communicate()
                
                if compressed_path.exists():
                    orig_output_path = output_path
                    output_path = compressed_path
                    logger.info(f"Render Session {session_id}: Compressed to {output_path.stat().st_size/1024/1024:.1f}MB")
                    try:
                        orig_output_path.unlink(missing_ok=True)
                    except Exception as e:
                        logger.warning(f"Failed to delete original uncompressed file: {e}")

            # 6. Upload
            logger.info(f"Render Session {session_id}: Uploading result...")
            file = discord.File(output_path, filename=f"tactical_{replay.filename.replace('.wowsreplay', '.mp4')}")
            embed = discord.Embed(
                title="✨ Render Output Complete",
                description=f"Analysis of `{replay.filename}` is ready.",
                color=discord.Color.blue()
            )
            embed.add_field(name="Trails", value="Enabled" if show_trails else "Disabled", inline=True)
            embed.add_field(name="Ranges", value="Enabled" if show_config else "Disabled", inline=True)
            embed.add_field(name="Discord Layout", value="Enabled" if discord_layout else "Disabled", inline=True)
            
            await interaction.followup.send(embed=embed, file=file)
        else:
            stderr_text = stderr.decode()
            error_lines = [l for l in stderr_text.splitlines() if l.strip()]
            error_msg = error_lines[-1] if error_lines else "Unknown error"
            
            await interaction.followup.send(f"❌ **Render Failed**\n`{error_msg}`\n\n*Tip: If GPU encoding failed, try enabling `cpu_mode`.*")
            logger.error(f"STDOUT: {stdout.decode()}")
            logger.error(f"STDERR: {stderr_text}")

    except Exception as e:
        await interaction.followup.send(f"⚠️ **Internal Error**\n`{str(e)}`")
        logger.exception("Error during render process")
    finally:
        # Await early webhook upload to complete if running to prevent file deletion during upload
        if webhook_task:
            try:
                logger.info("Waiting for early webhook upload to complete before temp file cleanup...")
                await webhook_task
            except Exception as we:
                logger.error(f"Error awaiting webhook task: {we}")
                
        # Clean up temp files
        try:
            if replay_path.exists(): replay_path.unlink(missing_ok=True)
            if red_replay_path and red_replay_path.exists(): red_replay_path.unlink(missing_ok=True)
            if output_path.exists(): output_path.unlink(missing_ok=True)
            logger.info(f"Cleaned up temp files for session {session_id}")
        except Exception as ce:
            logger.warning(f"Failed to clean up temp files: {ce}")

@bot.tree.command(name="ping", description="Check bot status")
async def ping(interaction: discord.Interaction):
    await interaction.response.send_message(f"🏓 Pong! Latency: {round(bot.latency * 1000)}ms")

if __name__ == "__main__":
    if not TOKEN:
        print("❌ Error: DISCORD_TOKEN not found in .env file.")
    elif not RENDERER_EXE:
        print("❌ Error: Renderer binary not found!")
        print("Please ensure minimap_renderer.exe is in the same folder or set RENDERER_PATH in .env")
    elif not GAME_DIR.exists():
        print(f"⚠️ Warning: WoWs path not found at {GAME_DIR}")
        print("The bot will still start, but renders may fail unless a valid path is provided in .env")
        bot.run(TOKEN)
    else:
        bot.run(TOKEN)
