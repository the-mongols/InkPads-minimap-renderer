# packages/renderer/inkpads-bot/analyzer.py
# UNIFIED SHREDDER: Captures high-fidelity tactical events and performance metrics in a single pass.
import json
import os
import sys
import subprocess
import logging
from collections import Counter, defaultdict
from typing import List, Dict, Any, Optional
import io

# Universal Unicode Shield managed at process level

# Setup logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s [%(levelname)s] %(message)s',
    handlers=[logging.StreamHandler(sys.stderr)]
)
logger = logging.getLogger("analyzer")

class ReplayAnalyzer:
    def __init__(self, input_path: str):
        self.input_path = input_path
        self.events: List[Dict[str, Any]] = []
        # entity_id -> {name, team, clan, ship_id}
        self.ships: Dict[int, Dict[str, Any]] = {}
        # entity_id -> {label, index, team_id}
        self.zones: Dict[int, Dict[str, Any]] = {}
        self.zone_team_state: Dict[int, int] = {}
        self.zone_progress: Dict[int, float] = {}

        self.sunk_ships = set()
        self.battle_start_clock: Optional[float] = None
        self.is_started = False
        self.player_team = 0
        self.match_result = "UNKNOWN"
        self.arena_id = None
        self.match_date = None

        # Advantage Tracking
        self.team_scores = [0, 0]
        self.last_advantage_level = "Even"
        self.last_advantage_team = 0 # 1 or 2 (team 0 or 1)

        self.team_clan_counts = defaultdict(Counter)
        self._seen = set()
        self._active_consumables = defaultdict(set)

        # Track metrics for all players
        self.player_stats = defaultdict(lambda: {
            "damage": 0,
            "received": 0,
            "spotting": 0,
            "potential": 0,
            "current_health": 0
        })

    def _elapsed(self, clock: float) -> float:
        if self.battle_start_clock is None:
            return 0.0
        return max(0.0, clock - self.battle_start_clock)

    def add_event(self, clock: float, etype: str, desc: str, team: Optional[int] = None, metadata: Optional[Dict[str, Any]] = None):
        elapsed = self._elapsed(clock)
        # Apply 14x time-scaling for VOD synchronization
        video_ts = round(elapsed / 14.0, 2)
        
        bucket = int(elapsed / 2)
        key = (etype, desc[:30], bucket)
        if key in self._seen:
            return
        self._seen.add(key)
        
        self.events.append({
            "time": round(elapsed, 2),
            "video_timestamp": video_ts,
            "type": etype,
            "desc": desc,
            "team": team,
            "metadata": metadata or {}
        })

    def _calculate_advantage(self, clock: float):
        s0, s1 = self.team_scores
        diff = abs(s0 - s1)
        
        level = "Even"
        team = 0
        
        if diff > 300: level = "Absolute"
        elif diff > 150: level = "Strong"
        elif diff > 50: level = "Moderate"
        elif diff > 10: level = "Weak"
        
        if s0 > s1: team = 0
        elif s1 > s0: team = 1
        
        if level != self.last_advantage_level or (level != "Even" and team != self.last_advantage_team):
            self.last_advantage_level = level
            self.last_advantage_team = team
            t_str = f"Team {chr(65 + team)}" if level != "Even" else "Both Teams"
            
            # Resolve team affiliation of the gaining team
            gaining_clan = self._majority_clan(team) if level != "Even" else "None"
            gaining_affiliation = "friendly" if (level != "Even" and team == self.player_team) else ("enemy" if level != "Even" else "None")
            
            metadata = {
                "advantage_level": level,
                "gaining_clan_tag": gaining_clan or "Unknown",
                "gaining_team_affiliation": gaining_affiliation,
                "friendly_score": s0 if self.player_team == 0 else s1,
                "enemy_score": s1 if self.player_team == 0 else s0
            }
            
            self.add_event(clock, "MOMENTUM_SHIFT", f"{level} Advantage: {t_str} ({s0}-{s1})", team=team if level != "Even" else None, metadata=metadata)

    def run(self) -> List[Dict[str, Any]]:
        if not os.path.exists(self.input_path):
            return []

        raw_lines = self._run_replayshark()
        if not raw_lines:
            return []

        self._pass1_metadata(raw_lines)
        self._pass2_events(raw_lines)

        return sorted(self.events, key=lambda e: e["time"])

    def _run_replayshark(self) -> List[Dict[str, Any]]:
        shark_exe = os.getenv("REPLAYSHARK_EXE", "replayshark.exe")
        wows_path = os.getenv("WOWS_PATH", "C:\\Games\\World_of_Warships")
        cmd = [shark_exe, "-g", wows_path, "dump", self.input_path]
        objects = []
        try:
            proc = subprocess.Popen(
                cmd, stdout=subprocess.PIPE,
                text=True, encoding="utf-8", errors="replace"
            )
            for line in proc.stdout:
                line = line.strip()
                if not line: continue
                try:
                    objects.append(json.loads(line))
                except: pass
            proc.wait()
        except Exception as e:
            logger.error(f"replayshark execution error: {e}")
        return objects

    def _pass1_metadata(self, objects: List[Dict[str, Any]]):
        self.player_name = None
        for obj in objects:
            if not isinstance(obj, dict): continue
            
            if "playerName" in obj:
                self.player_name = obj["playerName"]
            if "playerResult" in obj:
                self.match_result = obj["playerResult"]
            if "dateTime" in obj:
                self.match_date = obj["dateTime"]

            payload = obj.get("payload")
            if not isinstance(payload, dict): continue

            arena = payload.get("OnArenaStateReceived")
            if isinstance(arena, dict):
                self.arena_id = arena.get("arena_id")
                for p in arena.get("player_states", []):
                    eid = p.get("entity_id")
                    team_id = p.get("team_id")
                    name = p.get("username") or "Unknown"
                    clan_tag = p.get("clan") or ""
                    ship_id = p.get("ship_id")
                    spa_id = p.get("spa_id")
                    ship_name = p.get("ship_name") or p.get("ship_type_name") or "Unknown"
                    max_health = p.get("maxHealth") or p.get("max_health") or 0

                    self.ships[eid] = {
                        "name": name,
                        "team": team_id,
                        "clan": clan_tag,
                        "ship_id": ship_id,
                        "spa_id": spa_id,
                        "ship_name": ship_name,
                        "max_health": max_health
                    }
                    if clan_tag and team_id is not None:
                        self.team_clan_counts[team_id][clan_tag] += 1

                # Resolve friendly team side early
                if self.player_name:
                    for s in self.ships.values():
                        if s.get("name") == self.player_name:
                            self.player_team = s.get("team")
                            break

                # League/Rating parsing
                pb_info = arena.get("pre_battles_info", {})
                self.league_data = {}
                for t_id, players in pb_info.items():
                    if players and isinstance(players, list):
                        p0 = players[0]
                        info_str = p0.get("info", "")
                        if '"mmInfo":' in info_str:
                            import re
                            try:
                                league = re.search(r'"league":\s*I64\((\d+)\)', info_str)
                                mm_rating = re.search(r'"mmRating":\s*F64\(([\d.]+)\)', info_str)
                                pub_rating = re.search(r'"publicRating":\s*I64\((\d+)\)', info_str)
                                self.league_data[int(t_id)] = {
                                    "league": int(league.group(1)) if league else None,
                                    "mm_rating": float(mm_rating.group(1)) if mm_rating else None,
                                    "public_rating": int(pub_rating.group(1)) if pub_rating else None
                                }
                            except: pass

            ec = payload.get("EntityCreate")
            if isinstance(ec, dict) and ec.get("entity_type") == "InteractiveZone":
                eid = ec.get("entity_id")
                props = ec.get("props", {})
                cp = props.get("componentsState", {}).get("controlPoint", {})
                idx = cp.get("index")
                label = chr(65 + idx) if idx is not None else "?"
                team = props.get("teamId", -1)
                self.zones[eid] = {"label": label, "index": idx, "team_id": team}
                self.zone_team_state[eid] = team

    def _pass2_events(self, objects: List[Dict[str, Any]]):
        for obj in objects:
            if not isinstance(obj, dict): continue
            clock = obj.get("clock", 0.0)
            payload = obj.get("payload", {})

            # Battle Start
            if not isinstance(payload, dict):
                if isinstance(payload, str) and "OnBattleStart" in payload and not self.is_started:
                    self.battle_start_clock = clock
                    self.is_started = True
                    self.add_event(clock, "SYSTEM", "Battle Started")
                continue

            ep_state = payload.get("EntityProperty")
            is_battle_stage_zero = False
            if isinstance(ep_state, dict):
                if ep_state.get("property") == "battleStage" and ep_state.get("value") == 0:
                    is_battle_stage_zero = True

            if ("OnBattleStart" in payload or is_battle_stage_zero) and not self.is_started:
                self.battle_start_clock = clock
                self.is_started = True
                self.add_event(clock, "SYSTEM", "Battle Started")

            ep = payload.get("EntityProperty")
            if isinstance(ep, dict):
                self._handle_entity_property(clock, ep)

            em = payload.get("EntityMethod")
            if isinstance(em, dict):
                self._handle_entity_method(clock, em)

    def _handle_entity_property(self, clock: float, ep: Dict[str, Any]):
        eid = ep.get("entity_id")
        prop = ep.get("property")
        val = ep.get("value")

        if eid in self.ships:
            if prop == "damageDealt":
                self.player_stats[eid]["damage"] = val
            elif prop == "damageSpotting":
                self.player_stats[eid]["spotting"] = val
            elif prop == "damagePotential":
                self.player_stats[eid]["potential"] = val
            elif prop == "health":
                prev_h = self.player_stats[eid].get("current_health", val)
                max_h = self.ships[eid].get("max_health", 0)
                if val < prev_h:
                    dmg = prev_h - val
                    self.player_stats[eid]["received"] += dmg
                    if max_h > 0:
                        pct = (dmg / max_h) * 100
                        if pct >= 30.0:
                            s = self.ships[eid]
                            metadata = {
                                "victim": {
                                    "username": s["name"],
                                    "ship_id": s["ship_id"],
                                    "ship_name": s["ship_name"],
                                    "clan_tag": s["clan"],
                                    "team_affiliation": "friendly" if s["team"] == self.player_team else "enemy"
                                },
                                "damage_amount": dmg,
                                "percent_health_lost": round(pct, 1)
                            }
                            self.add_event(clock, "CRITICAL_HIT", f"{s['name']} took critical damage: {dmg} ({pct:.1f}%)", team=s["team"], metadata=metadata)
                self.player_stats[eid]["current_health"] = val

        if prop == "team_scores" and isinstance(val, list) and len(val) >= 2:
            self.team_scores = val
            self._calculate_advantage(clock)

        if prop == "teamId" and eid in self.zones:
            prev_team = self.zone_team_state.get(eid, -1)
            new_team = int(val) if val is not None else -1
            self.zone_team_state[eid] = new_team
            if new_team != prev_team and new_team != -1:
                zone = self.zones[eid]
                t_str = f"Team {chr(65 + new_team)}"
                metadata = {
                    "zone_label": zone["label"],
                    "clan_tag": self._majority_clan(new_team) or "Unknown",
                    "team_affiliation": "friendly" if new_team == self.player_team else "enemy"
                }
                self.add_event(clock, "CAP", f"Zone {zone['label']} captured by {t_str}", team=new_team, metadata=metadata)

        if prop == "isAlive" and val == 0 and eid in self.ships:
            if eid not in self.sunk_ships:
                self.sunk_ships.add(eid)
                s = self.ships[eid]
                victim_meta = {
                    "username": s["name"],
                    "ship_id": s["ship_id"],
                    "ship_name": s["ship_name"],
                    "clan_tag": s["clan"],
                    "team_affiliation": "friendly" if s["team"] == self.player_team else "enemy"
                }
                metadata = {
                    "victim": victim_meta,
                    "killer": {
                        "username": "Unknown",
                        "ship_id": 0,
                        "ship_name": "Unknown",
                        "clan_tag": "Unknown",
                        "team_affiliation": "enemy"
                    }
                }
                self.add_event(clock, "KILL", f"{s['name']} sunk", team=s["team"], metadata=metadata)

    def _handle_entity_method(self, clock: float, em: Dict[str, Any]):
        eid = em.get("entity_id")
        method = em.get("method")
        args = em.get("args", [])

        if method == "kill" and eid in self.ships:
            if eid not in self.sunk_ships:
                self.sunk_ships.add(eid)
                s = self.ships[eid]
                killer_eid = args[8] if len(args) > 8 else None
                killer_name = self.ships.get(killer_eid, {}).get("name") if killer_eid else "Unknown"
                
                victim_meta = {
                    "username": s["name"],
                    "ship_id": s["ship_id"],
                    "ship_name": s["ship_name"],
                    "clan_tag": s["clan"],
                    "team_affiliation": "friendly" if s["team"] == self.player_team else "enemy"
                }
                
                k = self.ships.get(killer_eid)
                if k:
                    killer_meta = {
                        "username": k["name"],
                        "ship_id": k["ship_id"],
                        "ship_name": k["ship_name"],
                        "clan_tag": k["clan"],
                        "team_affiliation": "friendly" if k["team"] == self.player_team else "enemy"
                    }
                else:
                    killer_meta = {
                        "username": killer_name,
                        "ship_id": 0,
                        "ship_name": "Unknown",
                        "clan_tag": "Unknown",
                        "team_affiliation": "enemy"
                    }
                    
                metadata = {
                    "victim": victim_meta,
                    "killer": killer_meta
                }
                self.add_event(clock, "KILL", f"{s['name']} sunk by {killer_name}", team=s["team"], metadata=metadata)

        if method == "onConsumableActivated" and eid in self.ships:
            c_raw = str(args[0]).lower() if args else ""
            if "radar" in c_raw: # HYDRO EXCLUDED per user request
                if "RADAR" not in self._active_consumables[eid]:
                    self._active_consumables[eid].add("RADAR")
                    s = self.ships[eid]
                    metadata = {
                        "username": s["name"],
                        "ship_id": s["ship_id"],
                        "ship_name": s["ship_name"],
                        "clan_tag": s["clan"],
                        "team_affiliation": "friendly" if s["team"] == self.player_team else "enemy"
                    }
                    self.add_event(clock, "RADAR", f"{s['name']} activated Radar", team=s["team"], metadata=metadata)

        if method == "onConsumableDeactivated" and eid in self.ships:
            c_raw = str(args[0]).lower() if args else ""
            if "radar" in c_raw:
                self._active_consumables[eid].discard("RADAR")

    def _majority_clan(self, team_id: int) -> Optional[str]:
        counts = self.team_clan_counts.get(team_id)
        if not counts: return None
        # Filter out empty tags and find the most common one
        valid = {k: v for k, v in counts.items() if k and len(k) > 1}
        if not valid: return None
        return max(valid, key=valid.get)

    def get_metadata(self) -> Dict[str, Any]:
        # 1. Determine player's team side
        player_team = None
        if self.player_name:
            for eid, s in self.ships.items():
                if s.get("name") == self.player_name:
                    player_team = s.get("team")
                    break
        if player_team is None:
            player_team = 0

        friendly_team_id = player_team
        enemy_team_id = 1 - player_team

        f_clan = self._majority_clan(friendly_team_id)
        e_clan = self._majority_clan(enemy_team_id)
        l_info = getattr(self, "league_data", {}).get(friendly_team_id, {})

        # Improve result detection if still UNKNOWN
        if self.match_result == "UNKNOWN" or self.match_result == "unknown" or not self.match_result:
                
            # 2. Count survivors per team
            alive_count = {0: 0, 1: 0}
            total_count = {0: 0, 1: 0}
            for eid, s in self.ships.items():
                t = s.get("team")
                if t is not None:
                    total_count[t] = total_count.get(t, 0) + 1
                    if eid not in self.sunk_ships:
                        alive_count[t] = alive_count.get(t, 0) + 1
            
            # 3. Determine winner based on ship counts and survivors
            enemy_team = 1 - player_team
            
            p_alive = alive_count.get(player_team, 0)
            e_alive = alive_count.get(enemy_team, 0)
            p_total = total_count.get(player_team, 0)
            e_total = total_count.get(enemy_team, 0)
            
            if p_total > 0 and e_total > 0:
                # If enemy team is completely wiped out: VICTORY!
                if e_alive == 0 and p_alive > 0:
                    self.match_result = "VICTORY"
                # If friendly team is completely wiped out: DEFEAT!
                elif p_alive == 0 and e_alive > 0:
                    self.match_result = "DEFEAT"
                # If both have survivors, compare relative losses
                else:
                    friendly_loss_ratio = (p_total - p_alive) / p_total
                    enemy_loss_ratio = (e_total - e_alive) / e_total
                    if friendly_loss_ratio < enemy_loss_ratio:
                        self.match_result = "VICTORY"
                    elif friendly_loss_ratio > enemy_loss_ratio:
                        self.match_result = "DEFEAT"
                    else:
                        # Tie break by total alive ships
                        if p_alive > e_alive:
                            self.match_result = "VICTORY"
                        else:
                            self.match_result = "DEFEAT"
            else:
                self.match_result = "DEFEAT" # Default safety

        stats_summary = []
        for eid, s in self.ships.items():
            st = self.player_stats.get(eid, {})
            stats_summary.append({
                "account_id": s.get("spa_id"),
                "name": s["name"],
                "clan": s["clan"],
                "team": s["team"],
                "ship_name": s.get("ship_name") or "Unknown",
                "damage": st.get("damage", 0),
                "received": st.get("received", 0),
                "spotting": st.get("spotting", 0),
                "potential": st.get("potential", 0),
                "survived": eid not in self.sunk_ships
            })
        
        return {
            "friendly_clan": f_clan or "Unknown",
            "enemy_clan": e_clan or "Unknown",
            "match_title": f"[{f_clan or 'Unknown'}] vs [{e_clan or 'Unknown'}]",
            "match_date": self.match_date,
            "result": self.match_result,
            "arena_id": str(self.arena_id) if self.arena_id is not None else None,
            "league": l_info.get("league"),
            "mm_rating": l_info.get("mm_rating"),
            "public_rating": l_info.get("public_rating"),
            "player_stats": stats_summary
        }

if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(1)
    analyzer = ReplayAnalyzer(sys.argv[1])
    results = analyzer.run()
    metadata = analyzer.get_metadata()
    output_data = {"events": results, "metadata": metadata}
    
    # Always print to stdout for foundry_worker compatibility
    print(json.dumps(output_data))
    
    if len(sys.argv) >= 3:
        with open(sys.argv[2], "w", encoding="utf-8") as f:
            json.dump(output_data, f, ensure_ascii=False, indent=2)
