#![allow(clippy::too_many_arguments)]

pub mod advantage;
#[cfg(feature = "rendering")]
pub mod assets;
pub mod codec;
pub mod config;
pub mod draw_command;
#[cfg(feature = "rendering")]
pub mod drawing;
#[cfg(feature = "rendering")]
pub mod encoder;
pub mod error;
pub mod map_data;
pub mod panel_math;
#[cfg(feature = "rendering")]
pub mod renderer;
#[cfg(feature = "rendering")]
pub mod video;

/// Minimap image size in pixels (square). Multiple of 16 for H.264 macroblock alignment.
pub const MINIMAP_SIZE: u32 = 1200;
/// Top margin for HUD elements (score bar, timer, kill feed).
pub const HUD_HEIGHT: u32 = 0;
/// Total canvas height: minimap + HUD.
pub const CANVAS_HEIGHT: u32 = MINIMAP_SIZE + HUD_HEIGHT;
/// Width of the stats side panel when enabled (16:10 default layout).
pub const STATS_PANEL_WIDTH: u32 = 720;
/// Width of the stats side panel in 16:9 aspect ratio mode.
pub const STATS_PANEL_WIDTH_16_9: u32 = 928;
/// Width of the vertical score strip on the left side of the minimap.
pub const VERTICAL_SCORE_STRIP_WIDTH: u32 = 112;
/// Width of the left and right outer empty margins in 16:9 Discord layout.
pub const CANVAS_MARGIN_WIDTH: u32 = 48;
/// Total canvas width for the 16:9 Discord layout: 48 + 112 + 1200 + 720 + 48 = 2128.
pub const CANVAS_WIDTH_16_9: u32 = 2128;
/// Halo thickness (in icon-pixel space) for the detected-teammate outline
/// drawn around ship icons. Both renderers pad the outline texture by this
/// amount on each side so the halo wraps fully around the icon's silhouette
/// instead of being clipped to the icon's bounding box.
pub const SHIP_ICON_OUTLINE_THICKNESS: u32 = 2;

#[cfg(feature = "rendering")]
pub use assets::GameFonts;
pub use codec::EncoderKind;
pub use codec::VideoCodec;
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
pub use encoder::EncoderConfig;
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
