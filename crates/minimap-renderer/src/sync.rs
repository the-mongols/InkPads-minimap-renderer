use std::collections::HashMap;
use wows_replays::analyzer::battle_controller::listener::BattleControllerState;
use wows_replays::types::{EntityId, GameClock, GameParamId, PlaneId, ElapsedClock};
use wows_replays::analyzer::battle_controller::{Player, Entity, GameMessage};
use wows_replays::analyzer::battle_controller::state::{
    ShipPosition, MinimapPosition, CapturePointState, BuffZoneState, LocalWeatherZone,
    CapturedBuff, TeamScore, ActiveConsumable, ActiveShot, ActiveTorpedo, ResolvedShotHit,
    ActivePlane, ActiveWard, KillRecord, DeadShip, ScoringRules, RibbonRecord
};
use wows_replays::analyzer::decoder::{BattleStage, FinishType, DamageStatEntry, DamageStatCategory, DamageStatWeapon, Recognized};
use wowsunpack::game_types::{BattleType, Ribbon};
use wows_replays::Rc;

pub struct StateMerger {
    // Cached merged states
    merged_ship_positions: HashMap<EntityId, ShipPosition>,
    merged_minimap_positions: HashMap<EntityId, MinimapPosition>,
    merged_player_entities: HashMap<EntityId, Rc<Player>>,
    merged_entities_by_id: HashMap<EntityId, Entity>,
    merged_active_consumables: HashMap<EntityId, Vec<ActiveConsumable>>,
}

impl StateMerger {
    pub fn new() -> Self {
        Self {
            merged_ship_positions: HashMap::new(),
            merged_minimap_positions: HashMap::new(),
            merged_player_entities: HashMap::new(),
            merged_entities_by_id: HashMap::new(),
            merged_active_consumables: HashMap::new(),
        }
    }

    /// Refresh the merged states from the current clocks.
    pub fn merge(&mut self, green: &dyn BattleControllerState, red: &dyn BattleControllerState) {
        self.merged_ship_positions.clear();
        self.merged_minimap_positions.clear();
        self.merged_player_entities.clear();
        self.merged_entities_by_id.clear();
        self.merged_active_consumables.clear();

        // 1. Players and Entities
        self.merged_player_entities.extend(red.player_entities().iter().map(|(k, v)| (*k, v.clone())));
        self.merged_player_entities.extend(green.player_entities().iter().map(|(k, v)| (*k, v.clone())));
        
        self.merged_entities_by_id.extend(red.entities_by_id().iter().map(|(k, v)| (*k, v.clone())));
        self.merged_entities_by_id.extend(green.entities_by_id().iter().map(|(k, v)| (*k, v.clone())));

        // 2. Consumables
        self.merged_active_consumables.extend(red.active_consumables().iter().map(|(k, v)| (*k, v.clone())));
        self.merged_active_consumables.extend(green.active_consumables().iter().map(|(k, v)| (*k, v.clone())));

        // 3. Positions
        let green_ships = green.ship_positions();
        let red_ships = red.ship_positions();
        
        let mut all_ids: std::collections::HashSet<EntityId> = green_ships.keys().cloned().collect();
        all_ids.extend(red_ships.keys().cloned());

        for id in all_ids {
            let green_pos = green_ships.get(&id);
            let red_pos = red_ships.get(&id);

            match (green_pos, red_pos) {
                (Some(g), Some(r)) => {
                    let is_green_ally = green.player_entities().get(&id).map(|p| p.relation().is_ally()).unwrap_or(false);
                    if is_green_ally {
                        self.merged_ship_positions.insert(id, g.clone());
                    } else {
                        self.merged_ship_positions.insert(id, r.clone());
                    }
                }
                (Some(g), None) => { self.merged_ship_positions.insert(id, g.clone()); }
                (None, Some(r)) => { self.merged_ship_positions.insert(id, r.clone()); }
                (None, None) => {}
            }
        }
        
        let green_mm = green.minimap_positions();
        let red_mm = red.minimap_positions();
        
        let mut all_mm_ids: std::collections::HashSet<EntityId> = green_mm.keys().cloned().collect();
        all_mm_ids.extend(red_mm.keys().cloned());

        for id in all_mm_ids {
            let green_pos = green_mm.get(&id);
            let red_pos = red_mm.get(&id);

            match (green_pos, red_pos) {
                (Some(g), Some(r)) => {
                    let is_green_ally = green.player_entities().get(&id).map(|p| p.relation().is_ally()).unwrap_or(false);
                    if is_green_ally {
                        self.merged_minimap_positions.insert(id, g.clone());
                    } else {
                        self.merged_minimap_positions.insert(id, r.clone());
                    }
                }
                (Some(g), None) => { self.merged_minimap_positions.insert(id, g.clone()); }
                (None, Some(r)) => { self.merged_minimap_positions.insert(id, r.clone()); }
                (None, None) => {}
            }
        }
    }
}

pub struct DualController<'a> {
    green: &'a dyn BattleControllerState,
    _red: &'a dyn BattleControllerState,
    merger: &'a StateMerger,
}

impl<'a> DualController<'a> {
    pub fn new(green: &'a dyn BattleControllerState, red: &'a dyn BattleControllerState, merger: &'a StateMerger) -> Self {
        Self { green, _red: red, merger }
    }
}

impl<'a> BattleControllerState for DualController<'a> {
    fn clock(&self) -> GameClock {
        self.green.clock()
    }

    fn ship_positions(&self) -> &HashMap<EntityId, ShipPosition> {
        &self.merger.merged_ship_positions
    }

    fn minimap_positions(&self) -> &HashMap<EntityId, MinimapPosition> {
        &self.merger.merged_minimap_positions
    }

    fn player_entities(&self) -> &HashMap<EntityId, Rc<Player>> {
        &self.merger.merged_player_entities
    }

    fn metadata_players(&self) -> &[wows_replays::analyzer::battle_controller::SharedPlayer] {
        self.green.metadata_players()
    }

    fn entities_by_id(&self) -> &HashMap<EntityId, Entity> {
        &self.merger.merged_entities_by_id
    }

    fn capture_points(&self) -> &[CapturePointState] {
        self.green.capture_points()
    }

    fn buff_zones(&self) -> &HashMap<EntityId, BuffZoneState> {
        self.green.buff_zones()
    }

    fn local_weather_zones(&self) -> &[LocalWeatherZone] {
        self.green.local_weather_zones()
    }

    fn captured_buffs(&self) -> &[CapturedBuff] {
        self.green.captured_buffs()
    }

    fn team_scores(&self) -> &[TeamScore] {
        self.green.team_scores()
    }

    fn game_chat(&self) -> &[GameMessage] {
        self.green.game_chat()
    }

    fn active_consumables(&self) -> &HashMap<EntityId, Vec<ActiveConsumable>> {
        &self.merger.merged_active_consumables
    }

    fn active_shots(&self) -> &[ActiveShot] {
        // TODO: Merge shots?
        self.green.active_shots()
    }

    fn active_torpedoes(&self) -> &[ActiveTorpedo] {
        self.green.active_torpedoes()
    }

    fn shot_hits(&self) -> &[ResolvedShotHit] {
        self.green.shot_hits()
    }

    fn active_planes(&self) -> &HashMap<PlaneId, ActivePlane> {
        self.green.active_planes()
    }

    fn active_wards(&self) -> &HashMap<PlaneId, ActiveWard> {
        self.green.active_wards()
    }

    fn kills(&self) -> &[KillRecord] {
        self.green.kills()
    }

    fn dead_ships(&self) -> &HashMap<EntityId, DeadShip> {
        self.green.dead_ships()
    }

    fn battle_end_clock(&self) -> Option<GameClock> {
        self.green.battle_end_clock()
    }

    fn winning_team(&self) -> Option<i8> {
        self.green.winning_team()
    }

    fn finish_type(&self) -> Option<&Recognized<FinishType>> {
        self.green.finish_type()
    }

    fn turret_yaws(&self) -> &HashMap<EntityId, Vec<f32>> {
        self.green.turret_yaws()
    }

    fn target_yaws(&self) -> &HashMap<EntityId, f32> {
        self.green.target_yaws()
    }

    fn selected_ammo(&self) -> &HashMap<EntityId, GameParamId> {
        self.green.selected_ammo()
    }

    fn battle_type(&self) -> Recognized<BattleType> {
        self.green.battle_type()
    }

    fn scoring_rules(&self) -> Option<&ScoringRules> {
        self.green.scoring_rules()
    }

    fn time_left(&self) -> Option<i64> {
        self.green.time_left()
    }

    fn battle_stage(&self) -> Option<BattleStage> {
        self.green.battle_stage()
    }

    fn battle_start_clock(&self) -> Option<GameClock> {
        self.green.battle_start_clock()
    }

    fn game_clock_to_elapsed(&self, clock: GameClock) -> ElapsedClock {
        self.green.game_clock_to_elapsed(clock)
    }

    fn elapsed_to_game_clock(&self, elapsed: ElapsedClock) -> GameClock {
        self.green.elapsed_to_game_clock(elapsed)
    }

    fn self_ribbons(&self) -> &HashMap<Ribbon, usize> {
        self.green.self_ribbons()
    }

    fn self_damage_stats(&self) -> &HashMap<(Recognized<DamageStatWeapon>, Recognized<DamageStatCategory>), DamageStatEntry> {
        self.green.self_damage_stats()
    }

    fn ribbon_history(&self) -> &[RibbonRecord] {
        self.green.ribbon_history()
    }

    fn server_timestamp(&self) -> Option<f64> {
        self.green.server_timestamp()
    }
}
