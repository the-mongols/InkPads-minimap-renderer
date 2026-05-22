#![allow(clippy::too_many_arguments)]

pub mod advantage;
#[cfg(feature = "rendering")]
pub mod assets;
pub mod config;
pub mod draw_command;
#[cfg(feature = "rendering")]
pub mod drawing;
#[cfg(feature = "rendering")]
pub mod encoder;
pub mod error;
pub mod map_data;
#[cfg(feature = "rendering")]
pub mod renderer;
#[cfg(feature = "rendering")]
pub mod sync;
pub mod video;

/// Minimap image size in pixels (square). Multiple of 16 for H.264 macroblock alignment.
pub const MINIMAP_SIZE: u32 = 1200;
/// Top margin for HUD elements (score bar, timer, kill feed).
pub const HUD_HEIGHT: u32 = 0;
/// Total canvas height: minimap + HUD.
pub const CANVAS_HEIGHT: u32 = MINIMAP_SIZE + HUD_HEIGHT;
/// Width of the stats side panel when enabled.
pub const STATS_PANEL_WIDTH: u32 = 192;
/// Width of the stats side panel in 16:9 aspect ratio mode.
/// 1200 + 192 = 1392 wide × 1200 tall.
pub const STATS_PANEL_WIDTH_16_9: u32 = 192;

#[cfg(feature = "rendering")]
pub use assets::GameFonts;
pub use config::RenderOptions;
pub use draw_command::ActivityFeedEntry;
pub use draw_command::DrawCommand;
pub use draw_command::FontHint;
pub use draw_command::RenderTarget;
pub use draw_command::RibbonCount;
pub use draw_command::ShipConfigFilter;
pub use draw_command::ShipConfigVisibility;
pub use draw_command::ShipVisibility;
#[cfg(feature = "rendering")]
pub use drawing::ImageTarget;
#[cfg(feature = "rendering")]
pub use drawing::ShipIcon;
#[cfg(feature = "rendering")]
pub use encoder::EncoderStatus;
#[cfg(feature = "rendering")]
pub use encoder::check_encoder;
pub use map_data::MapInfo;
pub use map_data::MinimapPos;
#[cfg(feature = "rendering")]
pub use renderer::MinimapRenderer;
#[cfg(feature = "rendering")]
pub use video::DumpMode;
#[cfg(feature = "rendering")]
pub use video::RenderProgress;
#[cfg(feature = "rendering")]
pub use video::RenderStage;
#[cfg(feature = "rendering")]
pub use video::VideoEncoder;
