use clap::Parser;
use indicatif::ProgressBar;
use indicatif::ProgressStyle;
use rootcause::prelude::*;
use std::cell::Cell;
use std::fs::File;
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::info;
use tracing::warn;
use wowsunpack::data::Version;
use wowsunpack::game_data;
use wowsunpack::game_params::provider::GameMetadataProvider;
use wowsunpack::game_params::types::GameParamProvider;
use wowsunpack::game_params::types::Param;
use wowsunpack::vfs::VfsPath;

use wows_replays::ReplayFile;
use wows_replays::analyzer::Analyzer;
use wows_replays::analyzer::battle_controller::BattleController;
use wows_replays::analyzer::battle_controller::listener::BattleControllerState;
use wows_replays::game_constants::GameConstants;
use wows_replays::analyzer::decoder::Consumable;

use wows_minimap_renderer::assets::load_building_icons;
use wows_minimap_renderer::assets::load_consumable_icons;
use wows_minimap_renderer::assets::load_death_cause_icons;
use wows_minimap_renderer::assets::load_flag_icons;
use wows_minimap_renderer::assets::load_game_fonts;
use wows_minimap_renderer::assets::load_map_image;
use wows_minimap_renderer::assets::load_map_info;
use wows_minimap_renderer::assets::load_packed_image;
use wows_minimap_renderer::assets::load_plane_icons;
use wows_minimap_renderer::assets::load_powerup_icons;
use wows_minimap_renderer::assets::load_ribbon_icons;
use wows_minimap_renderer::assets::load_ship_icons;
use wows_minimap_renderer::config::RendererConfig;
use wows_minimap_renderer::drawing::ImageTarget;
use wows_minimap_renderer::renderer::MinimapRenderer;
use wows_minimap_renderer::video::DumpMode;
use wows_minimap_renderer::video::RenderStage;
use wows_minimap_renderer::video::VideoEncoder;
use wows_minimap_renderer::sync::{DualController, StateMerger};

/// Generates a minimap timelapse video from a WoWS replay
#[derive(Parser)]
#[command(name = "Minimap Renderer")]
struct Args {
    /// Path to the World of Warships game directory
    #[arg(short = 'g', long = "game", conflicts_with = "extracted_dir", required_unless_present_any = ["generate_config", "check_encoder", "extracted_dir"])]
    game_dir: Option<PathBuf>,

    /// Path to pre-extracted renderer data directory (alternative to --game)
    #[arg(long, conflicts_with = "game_dir", required_unless_present_any = ["generate_config", "check_encoder", "game_dir"])]
    extracted_dir: Option<PathBuf>,

    /// Output MP4 file path, for dumping may not be specified to dump to stdout instead
    #[arg(short, long, required_unless_present_any = ["generate_config", "check_encoder", "dump_frame", "dump_frames"])]
    output: Option<PathBuf>,

    /// Dump a single frame as PNG instead of rendering video (specify frame number, 'mid' for midpoint or 'last' for last frame)
    #[arg(long, conflicts_with = "dump_frames")]
    dump_frame: Option<String>,

    /// Dump all frames as PNGs instead of rendering video
    #[arg(long, conflicts_with = "dump_frame")]
    dump_frames: bool,

    /// Hide player names above ship icons
    #[arg(long)]
    no_player_names: bool,

    /// Hide ship names above ship icons
    #[arg(long)]
    no_ship_names: bool,

    /// Hide capture point zones
    #[arg(long)]
    no_capture_points: bool,

    /// Hide building markers
    #[arg(long)]
    no_buildings: bool,

    /// Hide turret direction indicators
    #[arg(long)]
    no_turret_direction: bool,

    /// Hide selected armament/ammo type below ship icons
    #[arg(long)]
    no_armament: bool,

    /// Hide the kill feed
    #[arg(long)]
    no_kill_feed: bool,

    /// Hide chat messages
    #[arg(long)]
    no_chat: bool,

    /// Show position trail heatmap (rainbow coloring)
    #[arg(long)]
    show_trails: bool,

    /// Hide trails for dead ships
    #[arg(long)]
    no_dead_trails: bool,

    /// Show speed-based trails (blue=slow, red=fast)
    #[arg(long)]
    show_speed_trails: bool,

    /// Show ship config range circles (detection, battery, etc.)
    #[arg(long)]
    show_ship_config: bool,

    /// Path to TOML config file
    #[arg(long)]
    config: Option<PathBuf>,

    /// Print default TOML config to stdout and exit
    #[arg(long)]
    generate_config: bool,

    /// Check encoder availability (GPU/CPU) and exit
    #[arg(long)]
    check_encoder: bool,

    /// Use CPU encoder (openh264) instead of GPU
    #[arg(long)]
    cpu: bool,

    /// Disable progress bar and use log output instead
    #[arg(long)]
    no_progress: bool,

    /// Path to a constants JSON file (from wows-constants repo) to override
    /// consumable IDs, battle stages, etc.
    #[arg(short = 'c', long = "constants")]
    constants: Option<PathBuf>,

    /// Recreate game_params.rkyv from VFS GameParams.data if deserialization fails
    /// (useful when the internal format changes between versions)
    #[arg(long)]
    recreate_game_params: bool,

    /// Hide the side statistics panel
    #[arg(long)]
    no_stats_panel: bool,

    /// Optimize layout elements and statistics for Discord embeds
    #[arg(long)]
    discord_layout: bool,

    /// Use 16:9 aspect ratio canvas (wider stats panel)
    #[arg(long = "aspect-ratio-16-9")]
    aspect_ratio_16_9: bool,

    /// Target output video duration in seconds (default: 60)
    #[arg(long)]
    duration: Option<f64>,

    /// Path to a second replay file from the opposing team for synchronized dual-render
    #[arg(long)]
    red_replay: Option<PathBuf>,

    /// Dump forensic events (kills, caps, radar) to a JSON file
    #[arg(long)]
    dump_events: Option<PathBuf>,

    /// The replay file to process
    #[arg(required_unless_present_any = ["generate_config", "check_encoder"])]
    replay: Option<PathBuf>,
}

fn main() -> Result<(), Report> {
    let args = Args::parse();

    tracing_subscriber::fmt().with_target(false).with_writer(std::io::stderr).init();

    // Handle --generate-config before anything else
    if args.generate_config {
        print!("{}", RendererConfig::generate_default_toml());
        return Ok(());
    }

    // Handle --check-encoder
    if args.check_encoder {
        let status = wows_minimap_renderer::check_encoder();
        print!("{status}");
        return Ok(());
    }

    let output = match &args.output {
        None => {
            // Checking here so that video rendering can safely unwrap() later
            if !args.dump_frames && args.dump_frame.is_none() {
                bail!("output is required");
            }
            None
        }
        Some(path) => path.to_str(),
    };
    let replay_path = args.replay.as_ref().expect("replay is required");

    let dump_mode = match args.dump_frame.as_deref() {
        Some("mid") => Some(DumpMode::Midpoint),
        Some("last") => Some(DumpMode::Last),
        Some(n) => Some(DumpMode::Frame(n.parse::<usize>().expect("invalid frame number"))),
        None => None,
    };

    info!("Parsing replay");
    let replay_file = ReplayFile::from_file(replay_path)?;
    let replay_version = Version::from_client_exe(&replay_file.meta.clientVersionFromExe);

    let mut red_replay_file = None;
    if let Some(ref red_path) = args.red_replay {
        info!("Parsing second replay (red team)");
        let red_file = ReplayFile::from_file(red_path)?;
        // Basic sanity check: must be same map and roughly same time
        if red_file.meta.mapName != replay_file.meta.mapName {
            bail!("Replays are from different maps: {} vs {}", replay_file.meta.mapName, red_file.meta.mapName);
        }
        red_replay_file = Some(red_file);
    }

    // Load game data from either a full game install or pre-extracted directory
    let resolved_extracted: Option<PathBuf> =
        args.extracted_dir.as_ref().map(|extracted| resolve_extracted_dir(extracted, &replay_version)).transpose()?;

    let (vfs_owned, specs, game_params, controller_game_params) = if let Some(ref resolved) = resolved_extracted {
        load_from_extracted(resolved, &replay_version, args.recreate_game_params)?
    } else {
        let game_dir = args.game_dir.as_ref().expect("game directory is required");
        load_from_game_dir(game_dir, &replay_version)?
    };
    let vfs = &vfs_owned;

    info!("Loading fonts and icons");
    let game_fonts = load_game_fonts(vfs);
    let ship_icons = load_ship_icons(vfs);
    let plane_icons = load_plane_icons(vfs);
    let building_icons = load_building_icons(vfs);
    let consumable_icons = load_consumable_icons(vfs);
    let death_cause_icons = load_death_cause_icons(vfs, wows_minimap_renderer::assets::ICON_SIZE);
    let powerup_icons = load_powerup_icons(vfs, wows_minimap_renderer::assets::ICON_SIZE);
    let flag_icons = load_flag_icons(vfs);
    let ribbon_icons = load_ribbon_icons(vfs);

    // Load game constants from game data (falls back to hardcoded defaults per-field)
    let mut game_constants = GameConstants::from_vfs(vfs);

    // Auto-load constants.json from extracted dir if present (and no explicit --constants)
    if args.constants.is_none()
        && let Some(ref resolved) = resolved_extracted
    {
        let auto_constants = resolved.join("constants.json");
        if auto_constants.exists()
            && let Ok(data) = std::fs::read_to_string(&auto_constants)
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(&data)
        {
            game_constants.merge_replay_constants(&json, replay_version.build);
            info!("Merged constants from dump: {}", auto_constants.display());
        }
    }

    // Explicit --constants flag takes priority (overrides auto-loaded)
    if let Some(ref constants_path) = args.constants {
        let data = std::fs::read_to_string(constants_path)
            .unwrap_or_else(|e| panic!("Failed to read constants file {}: {e}", constants_path.display()));
        let json: serde_json::Value =
            serde_json::from_str(&data).unwrap_or_else(|e| panic!("Failed to parse constants JSON: {e}"));
        game_constants.merge_replay_constants(&json, replay_version.build);
        info!("Merged replay constants from {}", constants_path.display());
    }

    if let Some(mode_name) = game_constants.game_mode_name(replay_file.meta.gameMode as i32) {
        info!(mode = %mode_name, id = replay_file.meta.gameMode, "Game mode");
    }

    // Load map image and metadata from game files
    let map_name = &replay_file.meta.mapName;
    let map_image = load_map_image(map_name, vfs);
    let map_info = load_map_info(map_name, vfs);

    let game_duration = replay_file.meta.duration as f32;

    // Load config: --config path > exe-adjacent minimap_renderer.toml > defaults
    let mut config = if let Some(config_path) = &args.config {
        RendererConfig::load(config_path)?
    } else {
        // Try exe-adjacent config file
        let exe_config = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("minimap_renderer.toml")));
        match exe_config {
            Some(path) if path.exists() => {
                info!(path = ?path, "Loading config");
                RendererConfig::load(&path)?
            }
            _ => RendererConfig::default(),
        }
    };
    config.apply_cli_overrides(&wows_minimap_renderer::config::CliOverrides {
        no_player_names: args.no_player_names,
        no_ship_names: args.no_ship_names,
        no_capture_points: args.no_capture_points,
        no_buildings: args.no_buildings,
        no_turret_direction: args.no_turret_direction,
        no_armament: args.no_armament,
        no_kill_feed: args.no_kill_feed,
        no_chat: args.no_chat,
        no_stats_panel: args.no_stats_panel,
        show_trails: args.show_trails,
        no_dead_trails: args.no_dead_trails,
        show_speed_trails: args.show_speed_trails,
        show_ship_config: args.show_ship_config,
        discord_layout: args.discord_layout,
        aspect_ratio_16_9: args.aspect_ratio_16_9,
        video_duration: args.duration,
    });
    let mut options = config.into_render_options();
    options.ship_config_visibility = wows_minimap_renderer::ShipConfigVisibility::SelfOnly;

    let mut target = ImageTarget::with_stats_panel_16_9(
        map_image,
        game_fonts.clone(),
        ship_icons,
        plane_icons,
        building_icons,
        consumable_icons,
        death_cause_icons,
        powerup_icons,
        ribbon_icons,
        options.show_stats_panel,
        options.aspect_ratio_16_9,
    );
    target.large_elements = options.large_elements;
    target.compact_stats = options.compact_stats;
    let video_duration = options.video_duration;

    // Load self player's ship silhouette for the stats panel
    let self_silhouette = replay_file.meta.vehicles.iter().find(|v| v.relation == 0).and_then(|v| {
        let param = GameParamProvider::game_param_by_id(&game_params, v.shipId)?;
        let path = format!("gui/ships_silhouettes/{}.png", param.index());
        let img = load_packed_image(&path, vfs)?;
        Some(img.into_rgba8())
    });

    let mut renderer = MinimapRenderer::new(map_info.clone(), &game_params, replay_version, options);
    renderer.set_fonts(game_fonts);
    renderer.set_flag_icons(flag_icons);
    if let Some(sil) = self_silhouette {
        renderer.set_self_silhouette(sil);
    }

    let (cw, ch) = target.canvas_size();
    let mut encoder = VideoEncoder::new(output, dump_mode, args.dump_frames, game_duration, cw, ch);
    if video_duration > 0.0 {
        encoder.set_output_duration(video_duration);
    }
    if args.cpu {
        encoder.set_prefer_cpu(true);
    }
    // Initialize the encoder eagerly so startup logs appear before the
    // progress bar. Skip for dump modes which don't encode video.
    if args.dump_frame.is_none() {
        encoder.init()?;
    }

    let use_progress_bar = !args.no_progress;
    let progress_bar = if use_progress_bar {
        let pb = ProgressBar::new(0);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{msg} [{bar:40}] {pos}/{len} ({eta})")
                .expect("valid progress template")
                .progress_chars("=> "),
        );
        pb.set_message("Encoding");
        let pb_clone = pb.clone();
        let current_stage = Cell::new(RenderStage::Encoding);
        encoder.set_progress_callback(move |p| {
            if p.stage != current_stage.get() {
                current_stage.set(p.stage);
                pb_clone.set_position(0);
                pb_clone.set_message(match p.stage {
                    RenderStage::Encoding => "Encoding",
                    RenderStage::Muxing => "Muxing",
                });
            }
            pb_clone.set_length(p.total);
            pb_clone.set_position(p.current);
        });
        Some(pb)
    } else {
        let last_reported = Cell::new(RenderStage::Encoding);
        encoder.set_progress_callback(move |p| {
            if p.stage != last_reported.get() {
                last_reported.set(p.stage);
                info!(stage = ?p.stage, total = p.total, "Starting stage");
            }
            if p.current % 100 == 0 || p.current == p.total {
                info!(stage = ?p.stage, frame = p.current, total = p.total, "Progress");
            }
        });
        None
    };

    // Pre-scan packets to find the last clock for accurate progress reporting.
    {
        let mut scan_parser = wows_replays::packet2::Parser::new(&specs);
        let mut scan_remaining = &replay_file.packet_data[..];
        let mut last_clock = wows_replays::types::GameClock(0.0);
        while !scan_remaining.is_empty() {
            match scan_parser.parse_packet(&mut scan_remaining) {
                Ok(packet) => {
                    last_clock = wows_replays::types::GameClock(packet.clock.0.max(last_clock.0));
                }
                Err(_) => break,
            }
        }
        if last_clock.seconds() > 0.0 {
            encoder.set_battle_duration(last_clock);
        }
    }

    let (ts_green, ts_red) = {
        let mut h_parser_green = wows_replays::packet2::Parser::new(&specs);
        let mut h_remaining_green = &replay_file.packet_data[..];
        let mut h_controller_green = BattleController::new(&replay_file.meta, &controller_game_params, Some(&game_constants));
        
        while !h_remaining_green.is_empty() {
            let packet = h_parser_green.parse_packet(&mut h_remaining_green).map_err(|e| report!("Handshake Green error: {e:?}"))?;
            h_controller_green.process(&packet);
            if h_controller_green.battle_start_clock().is_some() || packet.clock.0 > 300.0 {
                break;
            }
        }
        
        let ts_g = h_controller_green.server_timestamp().unwrap_or(0.0);
        let ts_r_adj = if let Some(ref red_file) = red_replay_file {
            let mut h_parser_red = wows_replays::packet2::Parser::new(&specs);
            let mut h_remaining_red = &red_file.packet_data[..];
            let mut h_controller_red = BattleController::new(&red_file.meta, &controller_game_params, Some(&game_constants));
            
            while !h_remaining_red.is_empty() {
                let packet = h_parser_red.parse_packet(&mut h_remaining_red).map_err(|e| report!("Handshake Red error: {e:?}"))?;
                h_controller_red.process(&packet);
                if h_controller_red.battle_start_clock().is_some() || packet.clock.0 > 300.0 {
                    break;
                }
            }
            
            let ts_r = h_controller_red.server_timestamp().unwrap_or(0.0);
            let mut ts_r_val = ts_r;
            
            if (ts_g - ts_r).abs() > 60.0 {
                if let (Some(start_green), Some(start_red)) = (h_controller_green.battle_start_clock(), h_controller_red.battle_start_clock()) {
                    let offset = start_green.0 - start_red.0;
                    info!("Synced via Battle Start: Green@{}, Red@{}. Applied Offset={}", start_green.0, start_red.0, offset);
                    ts_r_val = ts_g + (start_green.0 as f64 - start_red.0 as f64);
                }
            }
            ts_r_val
        } else {
            0.0
        };
        (ts_g, ts_r_adj)
    };
    
    info!("Final Sync: Green TS={}, Red TS={}, Offset={}", ts_green, ts_red, ts_green - ts_red);

    // --- Main Rendering Phase ---
    let mut controller = BattleController::new(&replay_file.meta, &controller_game_params, Some(&game_constants));
    let mut red_controller = None;
    if let Some(ref red_file) = red_replay_file {
        red_controller = Some(BattleController::new(&red_file.meta, &controller_game_params, Some(&game_constants)));
    }

    let mut parser_green = wows_replays::packet2::Parser::new(&specs);
    let mut remaining_green = &replay_file.packet_data[..];
    let mut prev_clock_green = wows_replays::types::GameClock(0.0);

    let mut parser_red = wows_replays::packet2::Parser::new(&specs);
    let mut remaining_red = red_replay_file.as_ref().map(|f| &f.packet_data[..]).unwrap_or(&[]);

    let mut state_merger = StateMerger::new();

    let mut next_packet_green = None;
    let mut next_packet_red = None;

    // Forensic Event Collection
    let mut forensic_events = Vec::new();
    let mut last_captured_kills = 0;
    let mut active_radars = HashMap::new(); // entity_id -> start_time
    let mut last_zone_teams: HashMap<usize, i64> = HashMap::new(); // zone_index -> team_id

    // The main loop needs to drive both controllers
    loop {
        // Get next packets if needed
        if next_packet_green.is_none() && !remaining_green.is_empty() {
            next_packet_green = Some(parser_green.parse_packet(&mut remaining_green).map_err(|e| report!("Packet parse error: {e:?}"))?);
        }
        if next_packet_red.is_none() && !remaining_red.is_empty() && red_controller.is_some() {
            next_packet_red = Some(parser_red.parse_packet(&mut remaining_red).map_err(|e| report!("Packet parse error: {e:?}"))?);
        }

        if next_packet_green.is_none() && (next_packet_red.is_none() || red_controller.is_none()) {
            break;
        }

        // Determine which packet is next in "Real Time"
        let time_green = next_packet_green.as_ref().map(|p| p.clock.0 as f64 + ts_green).unwrap_or(f64::INFINITY);
        let time_red = next_packet_red.as_ref().map(|p| p.clock.0 as f64 + ts_red).unwrap_or(f64::INFINITY);

        if time_green <= time_red {
            let packet = next_packet_green.take().unwrap();
            if packet.clock != prev_clock_green {
                // Render tick
                if let Some(ref red_ctrl) = red_controller {
                    state_merger.merge(&controller, red_ctrl);
                    let dual = DualController::new(&controller, red_ctrl, &state_merger);
                    renderer.populate_players(&dual);
                    renderer.update_squadron_info(&dual);
                    renderer.update_ship_abilities(&dual);
                    encoder.advance_clock(prev_clock_green, &dual, &mut renderer, &mut target);
                } else {
                    renderer.populate_players(&controller);
                    renderer.update_squadron_info(&controller);
                    renderer.update_ship_abilities(&controller);
                    encoder.advance_clock(prev_clock_green, &controller, &mut renderer, &mut target);
                }
                prev_clock_green = packet.clock;
            }
            controller.process(&packet);
        } else {
            let packet = next_packet_red.take().unwrap();
            if let Some(ref mut red_ctrl) = red_controller {
                red_ctrl.process(&packet);
            }
        }

        // --- Forensic Event Monitoring ---
        if args.dump_events.is_some() {
            let clock = controller.current_clock();
            let elapsed = controller.battle_start_clock().map(|start| clock.0 - start.0).unwrap_or(0.0);
            
            // 1. Detect Kills
            let current_kills = controller.kills().len();
            if current_kills > last_captured_kills {
                for i in last_captured_kills..current_kills {
                    let kill = &controller.kills()[i];
                    let victim_player = controller.player_entities().get(&kill.victim);
                    let killer_player = controller.player_entities().get(&kill.killer);
                    
                    let victim_name = victim_player.map(|p| p.initial_state().username()).unwrap_or("Unknown");
                    let killer_name = killer_player.map(|p| p.initial_state().username()).unwrap_or("Unknown");
                    let victim_is_friendly = victim_player.map(|p| p.relation().is_ally() || p.relation().is_self()).unwrap_or(false);

                    forensic_events.push(serde_json::json!({
                        "time": elapsed,
                        "timestamp": elapsed,
                        "type": "KILL",
                        "desc": format!("{} sunk by {}", victim_name, killer_name),
                        "team": if victim_is_friendly { 0 } else { 1 }
                    }));
                }
                last_captured_kills = current_kills;
            }

            // 2. Detect Radar/Hydro usage
            for (eid, consumables) in controller.active_consumables() {
                for c in consumables {
                    let is_radar_or_hydro = match c.consumable.known() {
                        Some(Consumable::Radar) => Some("Radar"),
                        Some(Consumable::HydroacousticSearch) | Some(Consumable::Hydrophone) => Some("Hydro"),
                        _ => None,
                    };

                    if let Some(event_name) = is_radar_or_hydro {
                        if !active_radars.contains_key(eid) {
                            active_radars.insert(*eid, elapsed);
                            let ship_name = controller.player_entities().get(eid).map(|p| p.initial_state().username().to_string()).unwrap_or_else(|| "Unknown".to_string());
                            forensic_events.push(serde_json::json!({
                                "time": elapsed,
                                "timestamp": elapsed,
                                "type": "CONSUMABLE",
                                "desc": format!("{} activated {}", ship_name, event_name),
                                "team": if controller.player_entities().get(eid).map(|p| p.relation().is_ally() || p.relation().is_self()).unwrap_or(false) { 0 } else { 1 }
                            }));
                        }
                    }
                }
            }

            // 3. Detect Zone Captures
            for (idx, cp) in controller.capture_points().iter().enumerate() {
                if !cp.is_enabled { continue; }
                let current_team = cp.team_id;
                let prev_team = last_zone_teams.get(&idx).copied().unwrap_or(-1);

                if current_team != prev_team && current_team != -1 {
                    // Zone was captured by a team
                    let zone_label = (b'A' + idx as u8) as char;
                    let team_name = if current_team == 0 { "Team A" } else { "Team B" };
                    forensic_events.push(serde_json::json!({
                        "time": elapsed,
                        "timestamp": elapsed,
                        "type": "ZONE",
                        "desc": format!("Zone {} captured by {}", zone_label, team_name),
                        "team": current_team
                    }));
                }
                last_zone_teams.insert(idx, current_team);
            }
        }
    }


    // Save Forensic Events
    if let Some(event_path) = args.dump_events {
        let json = serde_json::to_string_pretty(&forensic_events)?;
        std::fs::write(event_path, json)?;
        info!("Forensic events dumped successfully.");
    }

    // Render final tick
    if let Some(ref red_ctrl) = red_controller {
        state_merger.merge(&controller, red_ctrl);
        let dual = DualController::new(&controller, red_ctrl, &state_merger);
        renderer.populate_players(&dual);
        renderer.update_squadron_info(&dual);
        renderer.update_ship_abilities(&dual);
        encoder.advance_clock(prev_clock_green, &dual, &mut renderer, &mut target);
        encoder.finish(&dual, &mut renderer, &mut target)?;
    } else {
        renderer.populate_players(&controller);
        renderer.update_squadron_info(&controller);
        renderer.update_ship_abilities(&controller);
        encoder.advance_clock(prev_clock_green, &controller, &mut renderer, &mut target);
        encoder.finish(&controller, &mut renderer, &mut target)?;
    }

    controller.finish();
    if let Some(ref mut red_ctrl) = red_controller {
        red_ctrl.finish();
    }

    if let Some(pb) = progress_bar {
        pb.finish_and_clear();
    }

    info!("Done");
    Ok(())
}

type LoadedGameData =
    (VfsPath, Vec<wowsunpack::rpc::entitydefs::EntitySpec>, GameMetadataProvider, GameMetadataProvider);

/// Load game data from a full WoWS game installation.
fn load_from_game_dir(game_dir: &std::path::Path, replay_version: &Version) -> Result<LoadedGameData, Report> {
    info!(build = %replay_version.build, "Loading game data");
    let resources = game_data::load_game_resources(game_dir, replay_version).map_err(|e| report!("{e}"))?;
    let vfs = resources.vfs;
    let specs = resources.specs;

    info!("Loading game params");
    let mut game_params =
        GameMetadataProvider::from_vfs(&vfs).map_err(|e| report!("Failed to load GameParams: {e:?}"))?;
    let mut controller_game_params =
        GameMetadataProvider::from_vfs(&vfs).map_err(|e| report!("Failed to load GameParams for controller: {e:?}"))?;

    let mo_path = game_data::translations_path(game_dir, replay_version.build);
    load_translations(&mo_path, &mut game_params, &mut controller_game_params);

    Ok((vfs, specs, game_params, controller_game_params))
}

struct ExtractedMetadata {
    version: String,
    build: u32,
}

fn read_metadata(path: &std::path::Path) -> Option<ExtractedMetadata> {
    let contents = std::fs::read_to_string(path.join("metadata.toml")).ok()?;
    let table: toml::Table = contents.parse().ok()?;
    Some(ExtractedMetadata {
        version: table.get("version")?.as_str()?.to_string(),
        build: table.get("build")?.as_integer()? as u32,
    })
}

/// Resolve the extracted data directory. If the user passed a parent directory
/// containing version subdirectories (e.g. `15.1.0_11965230/`), auto-detect the
/// right one. If they passed the version dir itself, use it directly.
fn resolve_extracted_dir(path: &std::path::Path, replay_version: &Version) -> Result<PathBuf, Report> {
    if !path.exists() {
        bail!("Extracted data directory does not exist: {}", path.display());
    }

    // If the path itself contains metadata.toml, it's already the version dir
    if let Some(meta) = read_metadata(path) {
        if meta.build != replay_version.build {
            bail!(
                "Extracted data is build {} ({}) but replay is build {}. \
                 Entity definitions will not match. Use extracted data for the correct build.",
                meta.build,
                meta.version,
                replay_version.build
            );
        }
        return Ok(path.to_path_buf());
    }

    // Otherwise, scan for version subdirectories
    let mut candidates: Vec<(PathBuf, ExtractedMetadata)> = Vec::new();
    let entries = std::fs::read_dir(path).attach_with(|| format!("Failed to read directory: {}", path.display()))?;

    for entry in entries.flatten() {
        let sub = entry.path();
        if let Some(meta) = read_metadata(&sub) {
            candidates.push((sub, meta));
        }
    }

    if candidates.is_empty() {
        bail!(
            "No extracted game data found in {}. Expected either a version directory \
             (containing metadata.toml, vfs/, game_params.rkyv) or a parent directory \
             containing version subdirectories (e.g. 15.1.0_11965230/).",
            path.display()
        );
    }

    // Try to match by build number first
    if let Some(matched) = candidates.iter().find(|(_, m)| m.build == replay_version.build) {
        info!("Matched extracted data for build {}: {}", replay_version.build, matched.0.display());
        return Ok(matched.0.clone());
    }

    // Try version-based fallback via BuildsIndex (supports cross-region replays)
    let builds_path = path.join("builds.toml");
    if builds_path.exists() {
        let version_str = format!("{}.{}.{}", replay_version.major, replay_version.minor, replay_version.patch);
        let index = wows_data_mgr::builds::BuildsIndex::load(&builds_path);
        if let Some((entry, _exact)) = index.resolve_build(replay_version.build, Some(&version_str)) {
            let resolved = path.join(&entry.dir);
            warn!("No exact data for build {}; using {} (build {})", replay_version.build, entry.version, entry.build);
            return Ok(resolved);
        }
    }

    // Also try version prefix match on candidates (legacy dumps without builds.toml)
    let version_str = format!("{}.{}.{}", replay_version.major, replay_version.minor, replay_version.patch);
    if let Some(matched) = candidates.iter().find(|(_, m)| m.version == version_str) {
        warn!(
            "No exact data for build {}; using {} (build {})",
            replay_version.build, matched.1.version, matched.1.build
        );
        return Ok(matched.0.clone());
    }

    let available: Vec<String> = candidates.iter().map(|(_, m)| format!("{} (build {})", m.version, m.build)).collect();
    bail!(
        "No extracted data matches replay build {}. Available versions in {}: {}",
        replay_version.build,
        path.display(),
        available.join(", ")
    );
}

/// Load game data from a pre-extracted renderer data directory.
fn load_from_extracted(
    extracted_dir: &std::path::Path,
    _replay_version: &Version,
    recreate_game_params: bool,
) -> Result<LoadedGameData, Report> {
    use std::borrow::Cow;
    use std::io::Read;
    use wowsunpack::data::DataFileWithCallback;
    use wowsunpack::rpc::entitydefs::parse_scripts;
    use wowsunpack::vfs::impls::physical::PhysicalFS;

    info!("Loading from extracted directory: {}", extracted_dir.display());

    let vfs_root = extracted_dir.join("vfs");
    if !vfs_root.exists() {
        bail!("VFS directory not found: {}", vfs_root.display());
    }
    let vfs = VfsPath::new(PhysicalFS::new(&vfs_root));

    // Load entity specs from VFS
    info!("Loading entity specs");
    let specs = {
        let vfs_ref = &vfs;
        let loader = DataFileWithCallback::new(move |path: &str| {
            let mut data = Vec::new();
            vfs_ref.join(path)?.open_file()?.read_to_end(&mut data)?;
            Ok(Cow::Owned(data))
        });
        parse_scripts(&loader).map_err(|e| report!("Failed to parse entity specs: {e:?}"))?
    };

    // Load GameParams: try rkyv cache first, fall back to VFS if --recreate-game-params
    let rkyv_path = extracted_dir.join("game_params.rkyv");
    let params: Vec<Param> = if rkyv_path.exists() {
        info!("Loading game params from rkyv cache");
        let rkyv_data = std::fs::read(&rkyv_path).attach_with(|| format!("Failed to read {}", rkyv_path.display()))?;
        match rkyv::from_bytes::<Vec<Param>, rkyv::rancor::Error>(&rkyv_data) {
            Ok(params) => params,
            Err(e) if recreate_game_params => {
                warn!("Failed to deserialize game_params.rkyv ({e}), recreating from GameParams.data");

                recreate_rkyv_from_vfs(&vfs, &rkyv_path)?
            }
            Err(e) => {
                bail!(
                    "Failed to deserialize GameParams: {e}\n\
                       Hint: the rkyv format may have changed. Pass --recreate-game-params to rebuild from GameParams.data"
                );
            }
        }
    } else if recreate_game_params {
        info!("game_params.rkyv not found, creating from GameParams.data");
        recreate_rkyv_from_vfs(&vfs, &rkyv_path)?
    } else {
        bail!(
            "game_params.rkyv not found at {}\n\
               Hint: pass --recreate-game-params to create it from GameParams.data",
            rkyv_path.display()
        );
    };

    let mut game_params = GameMetadataProvider::from_params_no_specs(params.clone())
        .map_err(|e| report!("Failed to build GameMetadataProvider: {e:?}"))?;
    let mut controller_game_params = GameMetadataProvider::from_params_no_specs(params)
        .map_err(|e| report!("Failed to build controller GameMetadataProvider: {e:?}"))?;

    // Load translations
    let mo_path = extracted_dir.join("translations/en/LC_MESSAGES/global.mo");
    load_translations(&mo_path, &mut game_params, &mut controller_game_params);

    Ok((vfs, specs, game_params, controller_game_params))
}

/// Load GameParams from VFS, serialize to rkyv, and write the cache file.
fn recreate_rkyv_from_vfs(vfs: &VfsPath, rkyv_path: &std::path::Path) -> Result<Vec<Param>, Report> {
    use std::sync::Arc;

    let game_metadata =
        GameMetadataProvider::from_vfs(vfs).map_err(|e| report!("Failed to load GameParams from VFS: {e:?}"))?;
    let params: Vec<Param> = game_metadata.params().iter().map(|p| Arc::unwrap_or_clone(Arc::clone(p))).collect();
    let bytes =
        rkyv::to_bytes::<rkyv::rancor::Error>(&params).map_err(|e| report!("Failed to serialize GameParams: {e}"))?;
    std::fs::write(rkyv_path, &bytes).attach_with(|| format!("Failed to write {}", rkyv_path.display()))?;
    info!("Wrote {} ({} bytes)", rkyv_path.display(), bytes.len());
    Ok(params)
}

fn load_translations(
    mo_path: &std::path::Path,
    game_params: &mut GameMetadataProvider,
    controller_game_params: &mut GameMetadataProvider,
) {
    if mo_path.exists() {
        if let Ok(file) = File::open(mo_path)
            && let Ok(catalog) = gettext::Catalog::parse(file)
        {
            game_params.set_translations(catalog);
            if let Ok(file2) = File::open(mo_path)
                && let Ok(catalog2) = gettext::Catalog::parse(file2)
            {
                controller_game_params.set_translations(catalog2);
            }
        } else {
            warn!(path = ?mo_path, "Failed to parse translations");
        }
    } else {
        warn!(path = ?mo_path, "Translations not found, ship names will be unavailable");
    }
}
