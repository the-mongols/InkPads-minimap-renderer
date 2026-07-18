import discord
from discord import app_commands
from discord.ext import commands
import os
import asyncio
import aiohttp
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
WOWS_EXTRACTED_DIR = os.getenv('WOWS_EXTRACTED_DIR')
RENDERER_FONT_PATH = os.getenv('RENDERER_FONT_PATH')

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
    
    # Auto-detect OS suffix (.exe on Windows, empty on Linux/macOS)
    suffix = '.exe' if os.name == 'nt' else ''
    
    # Common locations relative to bot.py
    search_paths = [
        Path(f'minimap_renderer{suffix}'),
        Path(f'../target/release/minimap_renderer{suffix}'),
        Path(f'../minimap_renderer{suffix}'),
        Path(f'../scripts/minimap_renderer{suffix}')
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

# Concurrency Queue
MAX_CONCURRENT_RENDERS = int(os.getenv('MAX_CONCURRENT_RENDERS', 1))
render_semaphore = None
render_queue = []

class InkpadsBot(commands.Bot):
    def __init__(self):
        intents = discord.Intents.default()
        intents.message_content = True
        super().__init__(command_prefix="!", intents=intents)

    async def setup_hook(self):
        global render_semaphore
        render_semaphore = asyncio.Semaphore(MAX_CONCURRENT_RENDERS)
        
        # Sync slash commands
        logger.info("Syncing slash commands...")
        synced = await self.tree.sync()
        logger.info(f"Synced {len(synced)} commands.")
        
        # Adjust session timeout to allow slow connections to upload without timing out
        if hasattr(self.http, "_HTTPClient__session") and self.http._HTTPClient__session:
            self.http._HTTPClient__session._timeout = aiohttp.ClientTimeout(
                total=900, connect=None, sock_read=None, sock_connect=60
            )
            logger.info("Adjusted HTTP client session timeout to 900 seconds.")

bot = InkpadsBot()

@bot.event
async def on_ready():
    logger.info(f'--- InkPads Tactical Bot 2.0 Ready ---')
    logger.info(f'Logged in as: {bot.user}')
    logger.info(f'Renderer EXE: {RENDERER_EXE}')
    logger.info(f'---------------------------------------')
    
    # Clear guild commands to remove legacy guild-level command registrations
    for guild in bot.guilds:
        try:
            bot.tree.clear_commands(guild=guild)
            await bot.tree.sync(guild=guild)
            logger.info(f"Cleared guild-level slash commands for: {guild.name} ({guild.id})")
        except Exception as e:
            logger.warning(f"Could not clear guild commands for {guild.name} ({guild.id}): {e}")
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

def clean_ship_name(raw_name):
    if not raw_name:
        return ""
    # Strip prefix before hyphen or underscore
    if "-" in raw_name:
        parts = raw_name.split("-", 1)
        name = parts[1]
    elif "_" in raw_name:
        parts = raw_name.split("_", 1)
        name = parts[1]
    else:
        name = raw_name
    return name.replace("_", " ").replace("-", " ").title()

def get_map_display_name(raw_map):
    if not raw_map:
        return ""
    
    key = raw_map
    if key.startswith("spaces/"):
        key = key[7:]
    
    mapping = {
        "00_CO_ocean": "Ocean",
        "01_solomon_islands": "Solomon Islands",
        "03_big_race": "Big Race",
        "04_Archipelago": "Archipelago",
        "05_Ring": "Ring",
        "08_Neighbors": "Neighbors",
        "08_NE_passage": "Strait",
        "10_NE_big_race": "Big Race",
        "13_Border_Empire": "Empire's Border",
        "13_OC_new_dawn": "New Dawn",
        "14_Atlantic": "The Atlantic",
        "15_NE_north": "North",
        "16_OC_bees_to_honey": "Hotspot",
        "17_NA_fault_line": "Fault Line",
        "18_NE_ice_islands": "Islands of Ice",
        "19_OC_prey": "Trap",
        "20_NE_two_brothers": "Two Brothers",
        "22_tierra_del_fuego": "Land of Fire",
        "23_shards": "Shards",
        "23_Shards": "Shards",
        "25_sea_hope": "Sea of Fortune",
        "28_naval_mission": "Tears of the Desert",
        "33_GH_gold_harbor": "Riprap",
        "33_new_tierra": "Polar",
        "34_OC_guam": "Guam",
        "34_OC_islands": "Islands",
        "35_NE_north_winter": "Northern Lights",
        "37_FC_mountain_range": "Mountain Range",
        "37_Ridge": "Mountain Range",
        "38_Canada": "Shatter",
        "38_J_warrior": "Warrior's Path",
        "40_Okinawa": "Okinawa",
        "41_Conquest": "Trident",
        "42_Neighbors": "Neighbors",
        "44_Path_warrior": "Warrior's Path",
        "45_Zigzag": "Loop",
        "46_Estuary": "Estuary",
        "47_Sleeping_Giant": "Sleeping Giant",
        "50_Gold_harbor": "Haven",
        "51_Greece": "Greece",
        "52_Britain": "Crash Zone Alpha",
        "52_crashed_harbor": "Crash Zone Alpha",
        "53_Shoreside": "Northern Waters",
        "53_waterline": "Waterline",
        "54_Faroe": "The Faroe Islands",
        "55_Seychelles": "Seychelles",
        "56_AngleWings": "Sunset Isle",
        "56_AngelWings": "Angel's Wings",
        "Canada": "Shatter",
        "Conquest": "Trident",
        "r01_military_navigation": "Riposte",
        "s06_Atoll": "Narai",
    }
    
    if key in mapping:
        return mapping[key]
        
    # Fallback: clean up internal directory name
    parts = key.split("_")
    cleaned_parts = [p.capitalize() for p in parts if not p.isdigit()]
    return " ".join(cleaned_parts)

def format_date_time(raw_dt):
    if not raw_dt:
        return ""
    from datetime import datetime
    try:
        dt = datetime.strptime(raw_dt, "%d.%m.%Y %H:%M:%S")
        month = str(dt.month)
        day = str(dt.day)
        year = str(dt.year)[-2:]
        time_str = dt.strftime("%I:%M %p").lstrip("0")
        return f"{month}/{day}/{year} {time_str}"
    except Exception:
        return raw_dt


async def send_webhook_payload(replay_path, red_replay_path, session_id):
    if not WEBHOOK_URL:
        return
    
    max_retries = 3
    headers = get_webhook_headers()
    loop = asyncio.get_event_loop()
    
    for attempt in range(1, max_retries + 1):
        if not replay_path.exists():
            logger.error(f"[{session_id}] Cannot upload payload: replay file {replay_path} not found.")
            break
            
        try:
            logger.info(f"[{session_id}] Webhook transmission attempt {attempt}/{max_retries} initiated.")
            
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
                        logger.warning(f"[{session_id}] Error closing file handle: {ce}")
            
            if resp.status_code in (200, 201, 202):
                logger.info(f"[{session_id}] Webhook transmission successful.")
                return
            elif resp.status_code in (400, 401, 403, 404):
                logger.error(f"[{session_id}] Webhook rejected with status {resp.status_code}.")
                break
            else:
                logger.warning(f"[{session_id}] Webhook failed with status {resp.status_code}.")
                
        except Exception as e:
            logger.error(f"[{session_id}] Exception during webhook transmission: {e}")
        
        if attempt < max_retries:
            backoff = 2 ** attempt
            logger.info(f"[{session_id}] Retrying webhook transmission in {backoff} seconds.")
            await asyncio.sleep(backoff)
            
    logger.error(f"[{session_id}] Webhook transmission failed after {max_retries} attempts.")



@bot.tree.command(name="render", description="Render a WoWS replay into a tactical video")
@app_commands.allowed_installs(guilds=True, users=True)
@app_commands.allowed_contexts(guilds=True, dms=True, private_channels=True)
@app_commands.describe(
    replay="The primary (Green) .wowsreplay file",
    red_replay="Optional secondary (Red) .wowsreplay file from the opposing team",
    show_trails="Display ship movement trails (heatmap)",
    show_config="Show detection and weapon range circles",
    cpu_mode="Use CPU encoding (slower, but safer if GPU is busy)",
    discord_layout="Optimize layout elements and statistics for Discord embeds (default: True)",
    layout_preset="Gutter size preset (default: B: Compromise 16:10)"
)
@app_commands.choices(layout_preset=[
    app_commands.Choice(name="Original (256px)", value="Original"),
    app_commands.Choice(name="A: Widescreen 16:9 (928px)", value="A"),
    app_commands.Choice(name="B: Compromise 16:10 (720px)", value="B"),
    app_commands.Choice(name="C: Discord-Maximized (448px)", value="C")
])
async def render(
    interaction: discord.Interaction, 
    replay: discord.Attachment,
    red_replay: discord.Attachment = None,
    show_trails: bool = False,
    show_config: bool = False,
    cpu_mode: bool = False,
    discord_layout: bool = True,
    layout_preset: app_commands.Choice[str] = None
):
    if not replay.filename.endswith('.wowsreplay'):
        await interaction.response.send_message("The provided file is not a valid .wowsreplay format.", ephemeral=True)
        return

    # Acknowledge and defer
    await interaction.response.defer(ephemeral=False)
    
    # Create unique session ID
    session_id = str(uuid.uuid4())[:8]
    replay_path = TEMP_DIR / f"{session_id}_green.wowsreplay"
    red_replay_path = TEMP_DIR / f"{session_id}_red.wowsreplay" if red_replay else None
    output_path = TEMP_DIR / f"{session_id}.mp4"

    logger.info(f"[{session_id}] Initiating render pipeline. Primary: {replay.filename}")
    
    # Send initial status embed
    embed = discord.Embed(
        title="Processing Replay",
        description=f"File: {replay.filename}\nStatus: Initializing render engine...",
        color=0x808080 # Grey
    )
    # Handle Queue Status
    if render_semaphore.locked():
        queue_pos = len(render_queue) + 1
        render_queue.append(session_id)
        embed.description = f"File: `{replay.filename}`\n\nStatus: [Queued] Position: {queue_pos}\nWaiting for available resources..."
        await interaction.edit_original_response(embed=embed)
        logger.info(f"[{session_id}] Render request queued at position {queue_pos}.")
    else:
        render_queue.append(session_id)
        await interaction.edit_original_response(embed=embed)

    webhook_task = None
    
    try:
        # Block until we acquire a render slot
        await render_semaphore.acquire()
        if session_id in render_queue:
            render_queue.remove(session_id)
            
        # Update embed now that we are executing
        embed.description = f"File: `{replay.filename}`\nStatus: Initializing render engine..."
        await interaction.edit_original_response(embed=embed)
        
        # 1. Download
        await replay.save(replay_path)
        if red_replay:
            await red_replay.save(red_replay_path)
        
        # Parse header
        header = parse_replay_header(replay_path)
        raw_ship = header.get("playerVehicle", "")
        raw_map = header.get("mapName", "") or header.get("mapDisplayName", "")
        raw_dt = header.get("dateTime", "")

        ship_name = clean_ship_name(raw_ship)
        map_name = get_map_display_name(raw_map)
        formatted_dt = format_date_time(raw_dt)
        logger.info(f"[{session_id}] Metadata extracted: Ship={ship_name}, Map={map_name}")

        # Update state to Rendering
        embed.title = "Rendering Minimap"
        embed.color = 0x3498DB # Blue
        
        details = []
        if ship_name: details.append(f"**Ship:** {ship_name}")
        if map_name: details.append(f"**Map:** {map_name}")
        if formatted_dt: details.append(f"**Date:** {formatted_dt}")
        info_line = " | ".join(details)
        
        embed.description = f"{info_line}\n\nStatus: [░░░░░░░░░░] 0%\nRendering..."
        await interaction.edit_original_response(embed=embed)

        # 2. Early verification and Webhook handover
        if WEBHOOK_URL:
            try:
                match_group = str(header.get("matchGroup", "")).lower()
                game_type = str(header.get("gameType", "")).lower()
                is_clan_battle = any(x in match_group or x in game_type for x in ("clan", "cvc", "cw"))
                
                if is_clan_battle:
                    webhook_task = asyncio.create_task(send_webhook_payload(replay_path, red_replay_path, session_id))
            except Exception as e:
                logger.warning(f"[{session_id}] Early verification failed: {e}")

        # 3. Build CLI Command
        guild_limit_bytes = 10 * 1024 * 1024
        if interaction.guild:
            guild_limit_bytes = max(interaction.guild.filesize_limit, guild_limit_bytes)
        target_size_mib = int((guild_limit_bytes * 0.95) / (1024 * 1024))

        cmd = [str(RENDERER_EXE)]
        if WOWS_EXTRACTED_DIR:
            cmd.extend(["--extracted-dir", WOWS_EXTRACTED_DIR])
        else:
            cmd.extend(["-g", str(GAME_DIR)])
        cmd.extend(["-o", str(output_path), "--max-size-mib", str(target_size_mib), "--codec", "h264"])
        if RENDERER_FONT_PATH:
            cmd.extend(["--font", RENDERER_FONT_PATH])
        cmd.append(str(replay_path))

        if red_replay:
            cmd.extend(["--red-replay", str(red_replay_path), "--no-chat", "--no-kill-feed", "--no-stats-panel"])
        if show_trails: cmd.append("--show-trails")
        if show_config: cmd.append("--show-ship-config")
        if cpu_mode: cmd.append("--cpu")
        if discord_layout: cmd.append("--discord-layout")

        preset_val = layout_preset.value if layout_preset else "B"
        if preset_val == "A": cmd.extend(["--stats-panel-width", "928"])
        elif preset_val == "B": cmd.extend(["--stats-panel-width", "720"])
        elif preset_val == "C": cmd.extend(["--stats-panel-width", "448"])

        # 4. Render
        process = await asyncio.create_subprocess_exec(*cmd, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
        
        async def update_progress():
            try:
                for i in range(1, 10):
                    await asyncio.sleep(12) # Roughly 120s total render time, updated less frequently
                    bar = '█' * i + '░' * (10 - i)
                    embed.description = f"{info_line}\n\nStatus: [{bar}] {i*10}%\nRendering..."
                    await asyncio.shield(interaction.edit_original_response(embed=embed))
            except asyncio.CancelledError:
                pass
                
        prog_task = asyncio.create_task(update_progress())
        stdout, stderr = await process.communicate()
        prog_task.cancel()
        try:
            await prog_task
        except asyncio.CancelledError:
            pass

        if process.returncode == 0:
            # 5. Compress if needed
            if output_path.stat().st_size > int(guild_limit_bytes * 0.98):
                compressed_path = TEMP_DIR / f"{session_id}_compressed.mp4"
                c_proc = await asyncio.create_subprocess_exec("ffmpeg", "-y", "-i", str(output_path), "-vcodec", "libx264", "-crf", "22", "-preset", "fast", str(compressed_path))
                await c_proc.communicate()
                if compressed_path.exists():
                    output_path.unlink()
                    output_path = compressed_path

            # 6. Upload
            embed.description = f"{info_line}\n\nStatus: Uploading..."
            await interaction.edit_original_response(embed=embed)
            
            file = discord.File(output_path, filename=f"tactical_{replay.filename.replace('.wowsreplay', '.mp4')}")
            embed.title = "Render Complete"
            embed.color = 0x2ECC71 # Green
            embed.description = info_line
            await interaction.edit_original_response(embed=embed, attachments=[file])
        else:
            logger.error(f"[{session_id}] Render process failed with code {process.returncode}")
            logger.error(f"STDOUT:\n{stdout.decode('utf-8', errors='ignore')}")
            logger.error(f"STDERR:\n{stderr.decode('utf-8', errors='ignore')}")
            embed.title = "Render Failed"
            embed.color = 0xE74C3C # Red
            embed.description = "The render process encountered an error. Please verify the replay file."
            await interaction.edit_original_response(embed=embed)

    except Exception as e:
        logger.exception(f"[{session_id}] Error during render process")
        await interaction.followup.send("An internal error occurred during the render.")
    finally:
        if webhook_task:
            try:
                await webhook_task
            except Exception as we:
                logger.error(f"[{session_id}] Error awaiting webhook: {we}")
                
        try:
            if replay_path.exists(): replay_path.unlink()
            if red_replay_path and red_replay_path.exists(): red_replay_path.unlink()
            if output_path.exists(): output_path.unlink()
            logger.info(f"[{session_id}] Cleaned up temp files.")
        except Exception as ce:
            logger.warning(f"[{session_id}] Cleanup failure: {ce}")
        finally:
            # Always release the semaphore slot for the next user
            if render_semaphore:
                render_semaphore.release()

@bot.tree.command(name="ping", description="Check bot status")
@app_commands.allowed_installs(guilds=True, users=True)
@app_commands.allowed_contexts(guilds=True, dms=True, private_channels=True)
async def ping(interaction: discord.Interaction):
    await interaction.response.send_message(f"Pong! Latency: {round(bot.latency * 1000)}ms")

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
