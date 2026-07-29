import discord
from discord import app_commands
from discord.ext import commands
import os
import asyncio
import aiohttp
import uuid
import requests
import json
import re
from pathlib import Path
from collections import Counter
from dotenv import load_dotenv
import logging

# Load configuration
env_path = Path(__file__).parent / '.env'
load_dotenv(dotenv_path=env_path)
TOKEN = os.getenv('DISCORD_TOKEN')
WOWS_PATH = os.getenv('WOWS_PATH', 'C:\\Games\\World_of_Warships')
# Default to a few likely locations for the renderer
RENDERER_PATH = os.getenv('RENDERER_PATH')
WOWS_EXTRACTED_DIR = os.getenv('WOWS_EXTRACTED_DIR')
RENDERER_FONT_PATH = os.getenv('RENDERER_FONT_PATH')
FORCE_CPU = os.getenv('FORCE_CPU', 'false').lower() in ('true', '1', 'yes')
RENDERER_CODEC = os.getenv('RENDERER_CODEC')
ENABLE_INKPADS_LAYOUT = os.getenv('ENABLE_INKPADS_LAYOUT', 'false').lower() in ('true', '1', 'yes')
TOURNAMENT_LISTEN_CHANNEL_ID = os.getenv('TOURNAMENT_LISTEN_CHANNEL_ID')

LAYOUT_CHOICES = [
    app_commands.Choice(name="A: Default (16:10)", value="A"),
    app_commands.Choice(name="B: Widescreen (16:9)", value="B"),
]
if ENABLE_INKPADS_LAYOUT:
    LAYOUT_CHOICES.append(app_commands.Choice(name="C: InkPads (Clan Battles)", value="C"))



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
    
    # Sync slash commands per guild for immediate availability in all joined servers
    for guild in bot.guilds:
        try:
            if os.getenv('CLEAR_GUILD_COMMANDS', 'false').lower() in ('true', '1', 'yes'):
                bot.tree.clear_commands(guild=guild)
                await bot.tree.sync(guild=guild)
                logger.info(f"Cleared & synced guild-level slash commands for: {guild.name} ({guild.id})")
            else:
                bot.tree.copy_global_to(guild=guild)
                synced = await bot.tree.sync(guild=guild)
                logger.info(f"Synced {len(synced)} slash commands directly to guild: {guild.name} ({guild.id})")
        except Exception as e:
            logger.warning(f"Could not sync commands for guild {guild.name} ({guild.id}): {e}")

@bot.event
async def on_message(message: discord.Message):
    # Ignore messages from self
    if message.author == bot.user:
        return

    # If TOURNAMENT_LISTEN_CHANNEL_ID is set, filter by channel ID
    if TOURNAMENT_LISTEN_CHANNEL_ID:
        try:
            if str(message.channel.id) != str(TOURNAMENT_LISTEN_CHANNEL_ID).strip():
                return
        except Exception:
            return

    content = message.content.strip()

    # Also check if JSON is attached as a file
    json_text = content
    if not json_text and message.attachments:
        for att in message.attachments:
            if att.filename.endswith('.json') or att.content_type == 'application/json':
                try:
                    json_bytes = await att.read()
                    json_text = json_bytes.decode('utf-8', errors='ignore')
                    break
                except Exception:
                    pass

    if not json_text:
        return

    # 1. Check code blocks first
    if "```" in json_text:
        match = re.search(r'```(?:json)?\s*(\{[\s\S]*?\})\s*```', json_text)
        if match:
            json_text = match.group(1)

    # 2. If it doesn't start with '{', search for embedded JSON object {...}
    if not json_text.startswith("{"):
        match = re.search(r'(\{[\s\S]*\})', json_text)
        if match:
            json_text = match.group(1)

    try:
        data = json.loads(json_text)
    except Exception as e:
        logger.warning(f"Failed to parse JSON message in tournament channel: {e}. Raw content: {repr(message.content)}")
        return

    callback_url = data.get("callbackUrl")
    target_channel_id = data.get("targetChannelId")
    replays = data.get("replays", [])

    if not callback_url or not target_channel_id or not replays:
        logger.warning("Tournament payload missing required fields (callbackUrl, targetChannelId, replays)")
        return

    logger.info(f"Received valid tournament render payload for target channel {target_channel_id} with {len(replays)} replays.")

    # Schedule background task to process tournament render
    task = asyncio.create_task(process_tournament_render(message, callback_url, target_channel_id, replays))
    background_tasks.add(task)
    task.add_done_callback(background_tasks.discard)

async def process_tournament_render(message: discord.Message, callback_url: str, target_channel_id: str, replays: list):
    session_id = str(uuid.uuid4())[:8]
    logger.info(f"[{session_id}] Starting tournament render execution.")

    target_channel = bot.get_channel(int(target_channel_id))
    if not target_channel:
        try:
            target_channel = await bot.fetch_channel(int(target_channel_id))
        except Exception as e:
            logger.error(f"[{session_id}] Could not resolve target channel {target_channel_id}: {e}")
            return

    green_path = None
    red_path = None
    output_path = TEMP_DIR / f"{session_id}.mp4"

    try:
        async with aiohttp.ClientSession() as session:
            for idx, rdata in enumerate(replays):
                url = rdata.get("replay")
                tag = rdata.get("tag", "").upper()
                if not url:
                    continue
                
                dest_path = TEMP_DIR / f"{session_id}_{'green' if idx == 0 else 'red'}.wowsreplay"
                logger.info(f"[{session_id}] Downloading replay [{tag}] from {url}...")
                async with session.get(url, timeout=aiohttp.ClientTimeout(total=120)) as resp:
                    if resp.status == 200:
                        content = await resp.read()
                        dest_path.write_bytes(content)
                        if idx == 0:
                            green_path = dest_path
                        else:
                            red_path = dest_path
                    else:
                        logger.error(f"[{session_id}] Failed to download replay from {url}: status {resp.status}")

        if not green_path or not green_path.exists():
            logger.error(f"[{session_id}] Primary replay download failed or empty.")
            return

        # Acquire semaphore for rendering
        await render_semaphore.acquire()
        try:
            # Build CLI command
            cmd = [str(RENDERER_EXE)]
            if WOWS_EXTRACTED_DIR:
                cmd.extend(["--extracted-dir", WOWS_EXTRACTED_DIR])
            else:
                cmd.extend(["-g", str(GAME_DIR)])

            codec = RENDERER_CODEC.lower() if RENDERER_CODEC and RENDERER_CODEC.lower() in ("h264", "h265", "av1") else ("h264" if FORCE_CPU else "h265")
            cmd.extend(["-o", str(output_path), "--max-size-mib", "24", "--codec", codec])
            if RENDERER_FONT_PATH:
                cmd.extend(["--font", RENDERER_FONT_PATH])
            cmd.append(str(green_path))

            if red_path and red_path.exists():
                cmd.extend(["--red-replay", str(red_path), "--no-chat", "--no-kill-feed", "--no-stats-panel"])
            if FORCE_CPU:
                cmd.append("--cpu")

            # Optional layout selection from payload or environment
            layout_opt = str(data.get("layout") or data.get("preset") or ("C" if ENABLE_INKPADS_LAYOUT else "A")).upper()
            if layout_opt in ("C", "INKPADS"):
                cmd.extend(["--inkpads-layout", "--aspect-ratio-16-9", "--stats-panel-width", "928"])
            elif layout_opt in ("B", "WIDESCREEN"):
                cmd.extend(["--discord-layout", "--aspect-ratio-16-9", "--stats-panel-width", "928"])

            logger.info(f"[{session_id}] Executing renderer CLI command (layout: {layout_opt})...")
            process = await asyncio.create_subprocess_exec(*cmd, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
            stdout, stderr = await process.communicate()

            if process.returncode != 0:
                logger.error(f"[{session_id}] Tournament render failed with code {process.returncode}: {stderr.decode('utf-8', errors='ignore')}")
                return

            # Extract header info for embed formatting
            header = parse_replay_header(green_path)
            raw_ship = header.get("playerVehicle", "")
            raw_map = header.get("mapName", "") or header.get("mapDisplayName", "")
            raw_dt = header.get("dateTime", "")
            match_group = header.get("matchGroup", "")
            game_type = header.get("gameType", "")
            scenario = header.get("scenario", "")
            logic = header.get("logic", "") or header.get("gameLogic", "")

            is_dual = bool(red_path and red_path.exists())
            mode_name = get_game_mode_display_name(match_group, game_type, scenario, logic)
            if not mode_name:
                mode_name = "Training Battle"

            ship_name = clean_ship_name(raw_ship)
            map_name = get_map_display_name(raw_map)
            formatted_dt = format_date_time(raw_dt)
            opponent_clan = get_opponent_clan(header, min_players=4)

            err_text = stderr.decode('utf-8', errors='ignore') if stderr else ""
            winner_name = extract_winning_team(err_text)
            f_clan, e_clan = get_team_clans(header)

            embed_color = 0x2ECC71
            if winner_name == "Alpha Team":
                embed_color = 0x2ECC71
            elif winner_name == "Bravo Team":
                embed_color = 0xE74C3C
            elif winner_name == "Draw":
                embed_color = 0xF1C40F

            embed_title = f"[{f_clan}] vs [{e_clan}]" if (f_clan and e_clan) else "Tournament Match Render Complete"

            details = []
            if mode_name and map_name: details.append(f"**{mode_name}:** {map_name}")
            elif map_name: details.append(f"**Map:** {map_name}")

            if is_dual:
                if winner_name:
                    details.append(f"**Victory:** {winner_name}")
            else:
                if ship_name:
                    details.append(f"**Ship:** {ship_name}")

            if formatted_dt: details.append(f"**Date:** {formatted_dt}")
            info_line = " | ".join(details)

            embed = discord.Embed(
                title=embed_title,
                description=info_line,
                color=embed_color
            )

            file = discord.File(output_path, filename=f"tactical_match_{session_id}.mp4")
            posted_msg = await target_channel.send(embed=embed, file=file)
            logger.info(f"[{session_id}] Posted rendered match to target channel {target_channel_id}, message ID: {posted_msg.id}")

            # Send callback back to wows-tournaments.com
            async with aiohttp.ClientSession() as session:
                payload = {
                    "messageId": str(posted_msg.id),
                    "channelId": str(target_channel_id)
                }
                logger.info(f"[{session_id}] Sending callback to {callback_url} with payload {payload}")
                async with session.post(callback_url, json=payload, timeout=aiohttp.ClientTimeout(total=30)) as cb_resp:
                    logger.info(f"[{session_id}] Callback HTTP response status: {cb_resp.status}")

        finally:
            render_semaphore.release()

    except Exception as e:
        logger.exception(f"[{session_id}] Exception in tournament render pipeline: {e}")
    finally:
        # Temp files cleanup
        for p in (green_path, red_path, output_path):
            if p and p.exists():
                try:
                    p.unlink()
                except Exception:
                    pass


def parse_version_string(version_str):
    """Parses a version string like '15, 5, 0, 12668706' into a dict."""
    if not version_str:
        return None
    parts = [p.strip() for p in version_str.split(",")]
    if len(parts) < 4:
        return None
    try:
        return {
            "major": int(parts[0]),
            "minor": int(parts[1]),
            "patch": int(parts[2]),
            "build": int(parts[3]),
            "version_tuple": (int(parts[0]), int(parts[1]), int(parts[2])),
            "version_str": f"{parts[0]}.{parts[1]}.{parts[2]}",
        }
    except ValueError:
        return None

def load_game_versions_toml():
    """Reads game_versions.toml line-by-line and maps build_number -> version_string."""
    mapping = {}
    toml_path = Path(__file__).parent.parent / "game_versions.toml"
    if not toml_path.exists():
        toml_path = Path(__file__).parent / "game_versions.toml"
    if toml_path.exists():
        try:
            content = toml_path.read_text(encoding="utf-8")
            current_build = None
            for line in content.splitlines():
                line = line.strip()
                if line.startswith("[versions."):
                    m = re.match(r'\[versions\.(\d+)\]', line)
                    if m:
                        current_build = int(m.group(1))
                elif line.startswith("version") and current_build is not None:
                    m = re.search(r'version\s*=\s*["\']([^"\']+)["\']', line)
                    if m:
                        mapping[current_build] = m.group(1)
                        current_build = None
        except Exception as e:
            logger.warning(f"Could not parse game_versions.toml: {e}")
    return mapping

def get_supported_versions():
    """Scans WOWS_EXTRACTED_DIR and WOWS_PATH/bin to list all supported versions and builds."""
    supported = []
    
    # 1. Scan WOWS_EXTRACTED_DIR
    extracted_dir = os.getenv('WOWS_EXTRACTED_DIR')
    if extracted_dir:
        extracted_path = Path(extracted_dir)
        if extracted_path.exists() and extracted_path.is_dir():
            for p in extracted_path.iterdir():
                if p.is_dir():
                    name = p.name
                    parts = name.split('_')
                    if len(parts) == 2:
                        v_str, b_str = parts
                        v_parts = v_str.split('.')
                        try:
                            v_tuple = tuple(int(x) for x in v_parts)
                            b_num = int(b_str)
                            supported.append({"version": v_tuple, "build": b_num})
                            continue
                        except ValueError:
                            pass
                    # Fallback: check metadata.toml
                    meta_path = p / "metadata.toml"
                    if meta_path.exists():
                        try:
                            version = None
                            build = None
                            with open(meta_path, "r", encoding="utf-8") as f:
                                for line in f:
                                    line = line.strip()
                                    if line.startswith("version"):
                                        m = re.search(r'version\s*=\s*["\']([^"\']+)["\']', line)
                                        if m: version = m.group(1)
                                    elif line.startswith("build"):
                                        m = re.search(r'build\s*=\s*(\d+)', line)
                                        if m: build = int(m.group(1))
                            if version and build:
                                v_parts = version.split('.')
                                v_tuple = tuple(int(x) for x in v_parts)
                                supported.append({"version": v_tuple, "build": build})
                        except Exception:
                            pass

    # 2. Scan WOWS_PATH/bin
    wows_path = os.getenv('WOWS_PATH', 'C:\\Games\\World_of_Warships')
    if wows_path:
        bin_path = Path(wows_path) / "bin"
        if bin_path.exists() and bin_path.is_dir():
            versions_map = load_game_versions_toml()
            for p in bin_path.iterdir():
                if p.is_dir() and p.name.isdigit():
                    build_num = int(p.name)
                    v_tuple = None
                    if build_num in versions_map:
                        v_str = versions_map[build_num]
                        try:
                            v_tuple = tuple(int(x) for x in v_str.split('.'))
                        except ValueError:
                            pass
                    supported.append({"version": v_tuple, "build": build_num})
                    
    return supported

def validate_replay_version(version_str):
    """Checks if a replay version is supported, older, or newer.
    Returns: (is_supported, error_message, is_newer)
    """
    parsed = parse_version_string(version_str)
    if not parsed:
        return True, None, False  # Let renderer attempt if we can't parse it
        
    supported = get_supported_versions()
    if not supported:
        return True, None, False  # No local version info to check against
        
    replay_build = parsed["build"]
    replay_ver = parsed["version_tuple"]
    
    # Exact match by build or version tuple
    if any(s["build"] == replay_build for s in supported):
        return True, None, False
        
    if replay_ver and any(s["version"] == replay_ver for s in supported if s["version"]):
        return True, None, False
        
    # Categorize mismatch
    valid_builds = [s["build"] for s in supported]
    valid_vers = [s["version"] for s in supported if s["version"]]
    
    is_newer = False
    is_older = False
    
    if valid_builds:
        if replay_build > max(valid_builds):
            is_newer = True
        elif replay_build < min(valid_builds):
            is_older = True
            
    if not is_newer and not is_older and valid_vers and replay_ver:
        if replay_ver > max(valid_vers):
            is_newer = True
        elif replay_ver < min(valid_vers):
            is_older = True
            
    ver_display = parsed["version_str"]
    if is_newer:
        msg = f"This replay is from a newer version of World of Warships (v{ver_display}) than what the renderer currently supports. The renderer is being updated to support this new build; please check back in a few days!"
        return False, msg, True
    elif is_older:
        msg = f"This replay is from an older version of World of Warships (v{ver_display}) which is no longer supported by the renderer."
        return False, msg, False
    else:
        msg = f"This replay version (v{ver_display}) is not supported by the renderer."
        return False, msg, False

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

def extract_winning_team(stderr_text: str) -> str:
    if not stderr_text:
        return ""
    m = re.search(r'WINNING_TEAM:\s*([A-Za-z0-9 ]+)', stderr_text)
    if m:
        return m.group(1).strip()
    return ""

def get_battle_type_title(match_group, game_type):
    group = str(match_group).strip().lower() if match_group else ""
    gtype = str(game_type).strip().lower() if game_type else ""
    if group == "pvp":
        return "Random Battle"
    elif group == "ranked":
        return "Ranked Battle"
    elif group in ("clan", "cvc", "cw"):
        return "Clan Battle"
    elif group in ("cooperative", "coop"):
        return "Co-op Battle"
    elif group == "brawl":
        return "Brawl"
    elif group == "pve" or "operation" in gtype or "scenario" in gtype:
        return "Operation"
    elif group in ("training", "trainingroom", "training_room", "custom"):
        return "Training Battle"
    elif match_group:
        return str(match_group).strip().title()
    elif game_type:
        return str(game_type).strip().title()
    return "Battle"

def get_team_clans(header):
    """
    Extracts majority clan tag for Team 0 (friendly) and Team 1 (enemy).
    Returns (friendly_clan, enemy_clan).
    """
    if not isinstance(header, dict):
        return "", ""
    vehicles = header.get("vehicles", [])
    if not vehicles or not isinstance(vehicles, list):
        return "", ""

    team_clans = {0: Counter(), 1: Counter()}

    for v in vehicles:
        if not isinstance(v, dict):
            continue
        relation = v.get("relation")
        clan = v.get("clanTag") or v.get("clanAbbrev") or v.get("clan_tag") or v.get("clan")
        name = v.get("name", "")
        if not clan and name.startswith("[") and "]" in name:
            clan = name[1:name.index("]")]
        if not clan:
            continue
        clan = str(clan).strip()
        if not clan:
            continue

        team_id = 0 if relation in (0, 1) else 1
        team_clans[team_id][clan] += 1

    f_clan = team_clans[0].most_common(1)[0][0] if team_clans[0] else ""
    e_clan = team_clans[1].most_common(1)[0][0] if team_clans[1] else ""

    return f_clan, e_clan

def get_game_mode_display_name(match_group, game_type, scenario="", logic=""):
    group = str(match_group).strip().lower() if match_group else ""
    gtype = str(game_type).strip().lower() if game_type else ""
    scen = str(scenario).strip().lower() if scenario else ""
    log = str(logic).strip().lower() if logic else ""

    combined = f"{gtype} {scen} {log}"

    # Specific match mode / rule set detection from replay metadata
    if "clandomination" in combined or "clan_domination" in combined or "clan domination" in combined:
        return "ClanDomination"
    elif "domination" in combined:
        return "Domination"
    elif "armsrace" in combined or "arms_race" in combined or "arms race" in combined:
        return "Arms Race"
    elif "epicenter" in combined:
        return "Epicenter"
    elif "airship" in combined or "escort" in combined:
        return "Airship Escort"
    elif "convoy" in combined:
        return "Convoy"
    elif "asymmetric" in combined:
        return "Asymmetric Battle"
    elif "standard" in combined:
        return "Standard Battle"

    # Match group fallbacks
    if group == "pvp":
        return "Random Battle"
    elif group in ("cooperative", "coop"):
        return "Co-op Battle"
    elif group == "ranked":
        return "Ranked Battle"
    elif group in ("clan", "cvc", "cw"):
        return "Clan Battle"
    elif group == "brawl":
        return "Brawl"
    elif group in ("training", "trainingroom", "training_room", "custom"):
        return "Training Battle"
    elif group == "pve" or "operation" in gtype or "scenario" in gtype:
        return "Operation"
    elif group == "event":
        return "Event Mode"
    elif game_type:
        return str(game_type).strip().title()
    elif match_group:
        return str(match_group).strip().title()
    return ""

def get_opponent_clan(header, min_players=4):
    """Extracts opponent clan tag from replay header vehicles list."""
    if not isinstance(header, dict):
        return ""
    vehicles = header.get("vehicles", [])
    if not vehicles or not isinstance(vehicles, list):
        return ""
    
    # 1. First try by relation == 2 (Enemy team)
    enemy_clan_counts = {}
    all_team_clans = {} # team_id -> Counter(clanTag)
    
    for v in vehicles:
        if not isinstance(v, dict):
            continue
        relation = v.get("relation")
        
        clan = v.get("clanTag") or v.get("clanAbbrev") or v.get("clan_tag") or v.get("clan")
        name = v.get("name", "")
        if not clan:
            if name.startswith("[") and "]" in name:
                clan = name[1:name.index("]")]
        
        if not clan:
            continue
            
        clan = str(clan).strip()
        if not clan:
            continue
            
        if relation == 2:
            enemy_clan_counts[clan] = enemy_clan_counts.get(clan, 0) + 1
            
        team_id = 0 if relation in (0, 1) else 1
        if team_id not in all_team_clans:
            all_team_clans[team_id] = {}
        all_team_clans[team_id][clan] = all_team_clans[team_id].get(clan, 0) + 1
    
    if enemy_clan_counts:
        sorted_clans = sorted(enemy_clan_counts.items(), key=lambda x: x[1], reverse=True)
        top_clan, count = sorted_clans[0]
        if count >= min_players:
            return top_clan
        # If min_players condition fails, still return top enemy clan if at least 1 exists
        return top_clan

    # 2. Fallback: Check enemy team (team_id 1 when player is team 0)
    enemy_team = 1
    if enemy_team in all_team_clans and all_team_clans[enemy_team]:
        sorted_clans = sorted(all_team_clans[enemy_team].items(), key=lambda x: x[1], reverse=True)
        return sorted_clans[0][0]
        
    return ""





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



async def _render_impl(
    interaction: discord.Interaction, 
    replay: discord.Attachment,
    red_replay: discord.Attachment = None,
    show_trails: bool = False,
    show_config: bool = False,
    cpu_mode: bool = False,
    layout_preset: app_commands.Choice[str] = None
):
    # Acknowledge and defer immediately (Discord requires responses within 3 seconds)
    try:
        await interaction.response.defer(ephemeral=False)
    except (discord.NotFound, discord.HTTPException) as e:
        if getattr(e, 'code', 0) == 10062 or "10062" in str(e):
            logger.error("Failed to defer interaction: Unknown Interaction (Error 10062). "
                         "This is usually caused by latency, slow file upload, or your Windows system clock running out of sync with Discord's servers. "
                         "Please ensure your system time is synchronized (Settings > Time & Language > Date & time > Sync now).")
        raise e

    if not replay.filename.endswith('.wowsreplay'):
        await interaction.followup.send("The provided file is not a valid .wowsreplay format.", ephemeral=True)
        return
    
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
        
        # Early Version Validation
        version_str = header.get("clientVersionFromExe", "")
        is_supported, err_msg, is_newer = validate_replay_version(version_str)
        if not is_supported:
            logger.warning(f"[{session_id}] Replay version unsupported: {version_str} - {err_msg}")
            embed.title = "Unsupported Version"
            embed.color = 0xE67E22 # Orange
            embed.description = err_msg
            await interaction.edit_original_response(embed=embed)
            return

        raw_ship = header.get("playerVehicle", "")
        raw_map = header.get("mapName", "") or header.get("mapDisplayName", "")
        raw_dt = header.get("dateTime", "")
        match_group = header.get("matchGroup", "")
        game_type = header.get("gameType", "")
        scenario = header.get("scenario", "")
        logic = header.get("logic", "") or header.get("gameLogic", "")

        mode_name = get_game_mode_display_name(match_group, game_type, scenario, logic)
        ship_name = clean_ship_name(raw_ship)
        map_name = get_map_display_name(raw_map)
        formatted_dt = format_date_time(raw_dt)
        is_clan_battle = (mode_name == "Clan Battle") or any(x in str(match_group).lower() or x in str(game_type).lower() for x in ("clan", "cvc", "cw"))
        opponent_clan = get_opponent_clan(header, min_players=4 if is_clan_battle else 4)
        
        logger.info(f"[{session_id}] Metadata extracted: Mode={mode_name}, Ship={ship_name}, Map={map_name}, Opponent={opponent_clan or 'N/A'}")

        # Update state to Rendering
        embed.title = "Rendering Minimap"
        embed.color = 0x3498DB # Blue
        
        is_dual = bool(red_replay)
        details = []
        if mode_name and map_name:
            details.append(f"**{mode_name}:** {map_name}")
        elif map_name:
            details.append(f"**Map:** {map_name}")
        if not is_dual and ship_name:
            details.append(f"**Ship:** {ship_name}")
        if formatted_dt: details.append(f"**Date:** {formatted_dt}")
        if opponent_clan: details.append(f"**Opponent:** [{opponent_clan}]")
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
        codec = RENDERER_CODEC.lower() if RENDERER_CODEC and RENDERER_CODEC.lower() in ("h264", "h265", "av1") else None
        if not codec:
            codec = "h264" if (cpu_mode or FORCE_CPU) else "h265"
        cmd.extend(["-o", str(output_path), "--max-size-mib", str(target_size_mib), "--codec", codec])
        if RENDERER_FONT_PATH:
            cmd.extend(["--font", RENDERER_FONT_PATH])
        cmd.append(str(replay_path))

        if red_replay:
            cmd.extend(["--red-replay", str(red_replay_path), "--no-chat", "--no-kill-feed", "--no-stats-panel"])
        if show_trails: cmd.append("--show-trails")
        if show_config: cmd.append("--show-ship-config")
        if cpu_mode or FORCE_CPU: cmd.append("--cpu")

        preset_val = layout_preset.value if layout_preset else "A"
        if preset_val == "A":
            # Option A: Standard Default (16:10) - no layout flags
            pass
        elif preset_val == "B":
            cmd.extend(["--discord-layout", "--aspect-ratio-16-9", "--stats-panel-width", "928"])
        elif preset_val == "C":
            cmd.extend(["--inkpads-layout", "--aspect-ratio-16-9", "--stats-panel-width", "928"])

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
            logger.info(f"STDERR:\n{stderr.decode('utf-8', errors='ignore')}")
            # 5. Compress if needed
            if output_path.stat().st_size > int(guild_limit_bytes * 0.98):
                compressed_path = TEMP_DIR / f"{session_id}_compressed.mp4"
                c_proc = await asyncio.create_subprocess_exec("ffmpeg", "-y", "-i", str(output_path), "-vcodec", "libx264", "-crf", "22", "-preset", "fast", str(compressed_path))
                await c_proc.communicate()
                if compressed_path.exists():
                    output_path.unlink()
                    output_path = compressed_path

            # 6. Extract exact clan metadata via analyzer if Clan Battle
            if is_clan_battle:
                try:
                    from analyzer import ReplayAnalyzer
                    analyzer_inst = ReplayAnalyzer(str(replay_path))
                    analyzer_inst.run()
                    meta = analyzer_inst.get_metadata()
                    enemy_clan_tag = meta.get("enemy_clan")
                    if enemy_clan_tag and enemy_clan_tag != "Unknown":
                        opponent_clan = enemy_clan_tag
                        logger.info(f"[{session_id}] Extracted enemy clan from analyzer stream: [{opponent_clan}]")
                except Exception as ae:
                    logger.warning(f"[{session_id}] Failed to extract enemy clan via analyzer: {ae}")

            err_text = stderr.decode('utf-8', errors='ignore') if stderr else ""
            winner_name = extract_winning_team(err_text)
            f_clan, e_clan = get_team_clans(header)

            # Determine dynamic color for all matches (Victory = Green, Defeat = Red, Draw = Gold)
            if winner_name == "Alpha Team":
                embed_color = 0x2ECC71  # Green
            elif winner_name == "Bravo Team":
                embed_color = 0xE74C3C  # Red
            elif winner_name == "Draw":
                embed_color = 0xF1C40F  # Gold
            else:
                embed_color = 0x2ECC71  # Default Green

            details = []
            if is_clan_battle or (f_clan and e_clan):
                # Title format for Clan Battles: [CLAN1] vs [CLAN2]
                if f_clan and e_clan:
                    embed_title = f"[{f_clan}] vs [{e_clan}]"
                elif e_clan:
                    embed_title = f"Clan Battle vs [{e_clan}]"
                else:
                    embed_title = "Clan Battle Render Complete"

                # Subtext for Clan Battles: **Victory / Defeat:** map | **Ship:** ship | **Date:** date
                if not is_dual:
                    if winner_name == "Alpha Team":
                        result_label = "Victory"
                    elif winner_name == "Bravo Team":
                        result_label = "Defeat"
                    elif winner_name == "Draw":
                        result_label = "Draw"
                    else:
                        result_label = "Clan Battle"
                    details.append(f"**{result_label}:** {map_name}")
                else:
                    if mode_name and map_name: details.append(f"**{mode_name}:** {map_name}")
                    elif map_name: details.append(f"**Map:** {map_name}")
                    if winner_name: details.append(f"**Victory:** {winner_name}")
            else:
                # Title format for General Renders: <Battle Type> Render Complete (e.g. Random Battle Render Complete)
                battle_type_title = get_battle_type_title(match_group, game_type)
                embed_title = f"{battle_type_title} Render Complete"

                # Subtext for General Renders: **<Battle Mode>:** map | **Ship:** ship | **Date:** date
                if mode_name and map_name:
                    details.append(f"**{mode_name}:** {map_name}")
                elif map_name:
                    details.append(f"**Map:** {map_name}")
                if is_dual and winner_name:
                    details.append(f"**Victory:** {winner_name}")

            if not is_dual and ship_name:
                details.append(f"**Ship:** {ship_name}")
            if formatted_dt:
                details.append(f"**Date:** {formatted_dt}")

            info_line = " | ".join(details)

            # 7. Upload
            embed.description = f"{info_line}\n\nStatus: Uploading..."
            await interaction.edit_original_response(embed=embed)
            
            file = discord.File(output_path, filename=f"tactical_{replay.filename.replace('.wowsreplay', '.mp4')}")
            embed.title = embed_title
            embed.color = embed_color
            embed.description = info_line
            await interaction.edit_original_response(embed=embed, attachments=[file])
        else:
            logger.error(f"[{session_id}] Render process failed with code {process.returncode}")
            logger.error(f"STDOUT:\n{stdout.decode('utf-8', errors='ignore')}")
            err_text = stderr.decode('utf-8', errors='ignore')
            logger.error(f"STDERR:\n{err_text}")
            
            # Check for version mismatch markers in stderr
            if "REPLAY_VERSION_NEWER" in err_text:
                embed.title = "Unsupported Version"
                embed.color = 0xE67E22 # Orange
                # Extract the formatted message from the stderr report
                lines = [l.strip() for l in err_text.splitlines() if "REPLAY_VERSION_NEWER" in l]
                # Strip rootcause/anyhow error wrappers if present (e.g. "Error: REPLAY_VERSION_NEWER: ...")
                clean_msg = lines[0] if lines else ""
                if "REPLAY_VERSION_NEWER:" in clean_msg:
                    clean_msg = clean_msg.split("REPLAY_VERSION_NEWER:", 1)[1].strip()
                embed.description = clean_msg or "This replay is from a newer version of World of Warships than what the renderer currently supports. The renderer is being updated to support this new build; please check back in a few days!"
            elif "REPLAY_VERSION_OLDER" in err_text:
                embed.title = "Unsupported Version"
                embed.color = 0xE67E22 # Orange
                lines = [l.strip() for l in err_text.splitlines() if "REPLAY_VERSION_OLDER" in l]
                clean_msg = lines[0] if lines else ""
                if "REPLAY_VERSION_OLDER:" in clean_msg:
                    clean_msg = clean_msg.split("REPLAY_VERSION_OLDER:", 1)[1].strip()
                embed.description = clean_msg or "This replay is from an older version of World of Warships which is no longer supported by the renderer."
            elif "REPLAY_VERSION_UNSUPPORTED" in err_text:
                embed.title = "Unsupported Version"
                embed.color = 0xE67E22 # Orange
                lines = [l.strip() for l in err_text.splitlines() if "REPLAY_VERSION_UNSUPPORTED" in l]
                clean_msg = lines[0] if lines else ""
                if "REPLAY_VERSION_UNSUPPORTED:" in clean_msg:
                    clean_msg = clean_msg.split("REPLAY_VERSION_UNSUPPORTED:", 1)[1].strip()
                embed.description = clean_msg or "This replay version is not supported by the renderer."
            else:
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

if FORCE_CPU:
    @bot.tree.command(name="render", description="Render a WoWS replay into a tactical video")
    @app_commands.allowed_installs(guilds=True, users=True)
    @app_commands.allowed_contexts(guilds=True, dms=True, private_channels=True)
    @app_commands.describe(
        replay="The primary (Green) .wowsreplay file",
        red_replay="(Optional) Attach a .wowsreplay file from the opposing team for a dual-render",
        show_trails="Display ship movement trails (heatmap)",
        show_config="Show detection and weapon range circles",
        layout_preset="Layout preset (default: A: Default 16:10)"
    )
    @app_commands.choices(layout_preset=LAYOUT_CHOICES)
    async def render(
        interaction: discord.Interaction, 
        replay: discord.Attachment,
        red_replay: discord.Attachment = None,
        show_trails: bool = False,
        show_config: bool = False,
        layout_preset: app_commands.Choice[str] = None
    ):
        await _render_impl(
            interaction=interaction,
            replay=replay,
            red_replay=red_replay,
            show_trails=show_trails,
            show_config=show_config,
            cpu_mode=False,
            layout_preset=layout_preset
        )
else:
    @bot.tree.command(name="render", description="Render a WoWS replay into a tactical video")
    @app_commands.allowed_installs(guilds=True, users=True)
    @app_commands.allowed_contexts(guilds=True, dms=True, private_channels=True)
    @app_commands.describe(
        replay="The primary (Green) .wowsreplay file",
        red_replay="(Optional) Attach a .wowsreplay file from the opposing team for a dual-render",
        show_trails="Display ship movement trails (heatmap)",
        show_config="Show detection and weapon range circles",
        cpu_mode="Use CPU encoding (slower, but safer if GPU is busy)",
        layout_preset="Layout preset (default: A: Default 16:10)"
    )
    @app_commands.choices(layout_preset=LAYOUT_CHOICES)
    async def render(
        interaction: discord.Interaction, 
        replay: discord.Attachment,
        red_replay: discord.Attachment = None,
        show_trails: bool = False,
        show_config: bool = False,
        cpu_mode: bool = False,
        layout_preset: app_commands.Choice[str] = None
    ):
        await _render_impl(
            interaction=interaction,
            replay=replay,
            red_replay=red_replay,
            show_trails=show_trails,
            show_config=show_config,
            cpu_mode=cpu_mode,
            layout_preset=layout_preset
        )

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
