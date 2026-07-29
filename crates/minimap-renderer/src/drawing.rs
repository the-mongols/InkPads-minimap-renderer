use std::collections::HashMap;

use ab_glyph::Font;
use ab_glyph::FontArc;
use ab_glyph::PxScale;
use ab_glyph::ScaleFont;
use image::Rgb;
use image::RgbImage;
use image::RgbaImage;
use tiny_skia::BlendMode;
use tiny_skia::FillRule;
use tiny_skia::FilterQuality;
use tiny_skia::LineCap;
use tiny_skia::LineJoin;
use tiny_skia::Paint;
use tiny_skia::PathBuilder;
use tiny_skia::Pixmap;
use tiny_skia::PixmapPaint;
use tiny_skia::Stroke;
use tiny_skia::StrokeDash;
use tiny_skia::Transform;

use std::sync::Arc;

use crate::STATS_PANEL_WIDTH;
use crate::STATS_PANEL_WIDTH_16_9;
use crate::assets::GameFonts;

use crate::SHIP_ICON_OUTLINE_THICKNESS;

use crate::draw_command::ActivityFeedEntry;
use crate::draw_command::ActivityFeedKind;
use crate::draw_command::ChatEntry;
use crate::draw_command::DrawCommand;
use crate::draw_command::FontHint;
use crate::draw_command::KillFeedEntry;
use crate::draw_command::RenderTarget;
use crate::draw_command::ShipVisibility;
use wows_replays::types::ElapsedClock;
use wt_translations::DefaultTextResolver;
use wt_translations::TextResolver;
use wt_translations::TranslatableText;

// ── Pixmap conversion helpers ──────────────────────────────────────────────

/// Convert an RGB image (no alpha) to a tiny-skia Pixmap (opaque RGBA, premultiplied).
fn rgb_to_pixmap(img: &RgbImage) -> Pixmap {
    let w = img.width();
    let h = img.height();
    let mut pm = Pixmap::new(w, h).expect("failed to create pixmap");
    let data = pm.data_mut();
    for y in 0..h {
        for x in 0..w {
            let px = img.get_pixel(x, y).0;
            let idx = (y * w + x) as usize * 4;
            data[idx] = px[0];
            data[idx + 1] = px[1];
            data[idx + 2] = px[2];
            data[idx + 3] = 255;
        }
    }
    pm
}

/// Convert a tiny-skia Pixmap (premultiplied RGBA) back to an RGB image.
fn pixmap_to_rgb(pm: &Pixmap) -> RgbImage {
    let w = pm.width();
    let h = pm.height();
    let data = pm.data();
    let mut img = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize * 4;
            let a = data[idx + 3] as f32 / 255.0;
            // Unpremultiply alpha
            let (r, g, b) = if a > 0.001 {
                (
                    (data[idx] as f32 / a).min(255.0) as u8,
                    (data[idx + 1] as f32 / a).min(255.0) as u8,
                    (data[idx + 2] as f32 / a).min(255.0) as u8,
                )
            } else {
                (0, 0, 0)
            };
            img.put_pixel(x, y, Rgb([r, g, b]));
        }
    }
    img
}

/// Convert an RGBA image to a tiny-skia Pixmap (premultiplied alpha).
fn rgba_to_pixmap(img: &RgbaImage) -> Pixmap {
    let w = img.width();
    let h = img.height();
    let mut pm = Pixmap::new(w, h).expect("failed to create pixmap");
    let data = pm.data_mut();
    for y in 0..h {
        for x in 0..w {
            let px = img.get_pixel(x, y).0;
            let idx = (y * w + x) as usize * 4;
            let a = px[3] as f32 / 255.0;
            // Premultiply
            data[idx] = (px[0] as f32 * a) as u8;
            data[idx + 1] = (px[1] as f32 * a) as u8;
            data[idx + 2] = (px[2] as f32 * a) as u8;
            data[idx + 3] = px[3];
        }
    }
    pm
}

// ── Paint helpers ──────────────────────────────────────────────────────────

/// Create a solid-color paint with the given RGBA values.
fn solid_paint(r: u8, g: u8, b: u8, a: u8) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(r, g, b, a);
    paint.anti_alias = true;
    paint
}

/// Create a solid-color paint from an [u8; 3] array with alpha.
fn color_paint(color: [u8; 3], alpha: f32) -> Paint<'static> {
    let a = (alpha.clamp(0.0, 1.0) * 255.0) as u8;
    solid_paint(color[0], color[1], color[2], a)
}

// ── Text rendering directly onto Pixmap ────────────────────────────────────

/// Draw anti-aliased text onto a Pixmap at (x, y) with the given color.
///
/// Uses ab_glyph's per-pixel coverage callback for proper anti-aliasing.
/// Coordinates are in pixel space (x = left edge, y = top edge of text).
fn draw_text(pm: &mut Pixmap, color: [u8; 3], x: i32, y: i32, scale: PxScale, font: &impl Font, text: &str) {
    let scaled = font.as_scaled(scale);
    let mut cursor_x = x as f32;
    let baseline_y = y as f32 + scaled.ascent();
    let w = pm.width() as i32;
    let h = pm.height() as i32;
    let data = pm.data_mut();

    let mut last_glyph_id = None;
    for c in text.chars() {
        let glyph_id = scaled.glyph_id(c);
        if let Some(last) = last_glyph_id {
            cursor_x += scaled.kern(last, glyph_id);
        }
        let glyph = glyph_id.with_scale_and_position(scale, ab_glyph::point(cursor_x, baseline_y));
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, coverage| {
                let px = gx as i32 + bounds.min.x as i32;
                let py = gy as i32 + bounds.min.y as i32;
                if px < 0 || px >= w || py < 0 || py >= h {
                    return;
                }
                let cov = coverage.clamp(0.0, 1.0);
                if cov < 0.01 {
                    return;
                }
                let idx = (py as usize * w as usize + px as usize) * 4;
                // Read existing premultiplied pixel
                let bg_r = data[idx] as f32;
                let bg_g = data[idx + 1] as f32;
                let bg_b = data[idx + 2] as f32;
                let bg_a = data[idx + 3] as f32;
                // Source color (premultiplied by coverage)
                let src_r = color[0] as f32 * cov;
                let src_g = color[1] as f32 * cov;
                let src_b = color[2] as f32 * cov;
                let src_a = 255.0 * cov;
                // Source-over compositing
                let inv_a = 1.0 - cov;
                data[idx] = (src_r + bg_r * inv_a).min(255.0) as u8;
                data[idx + 1] = (src_g + bg_g * inv_a).min(255.0) as u8;
                data[idx + 2] = (src_b + bg_b * inv_a).min(255.0) as u8;
                data[idx + 3] = (src_a + bg_a * inv_a).min(255.0) as u8;
            });
        }
        cursor_x += scaled.h_advance(glyph_id);
        last_glyph_id = Some(glyph_id);
    }
}

/// Measure the width and height of text at the given scale.
fn text_size(scale: PxScale, font: &impl Font, text: &str) -> (u32, u32) {
    let scaled = font.as_scaled(scale);
    let mut w = 0.0f32;
    let mut last_glyph_id = None;
    for c in text.chars() {
        let glyph_id = scaled.glyph_id(c);
        if let Some(last) = last_glyph_id {
            w += scaled.kern(last, glyph_id);
        }
        w += scaled.h_advance(glyph_id);
        last_glyph_id = Some(glyph_id);
    }
    let h = scaled.ascent() - scaled.descent();
    (w.ceil() as u32, h.ceil() as u32)
}

/// Returns the standardized tier color for a given WPA score relative to team average.
fn wpa_color(wpa: f64, team_avg: f64) -> [u8; 3] {
    let safe_avg = team_avg.max(0.5);
    let mult = wpa / safe_avg;

    if mult >= 2.0 && wpa >= 1.5 {
        [180, 100, 255] // Purple (MVP / Dominant relative performance)
    } else if mult >= 1.2 || wpa >= 1.8 {
        [100, 220, 120] // Green (High Contribution)
    } else if mult >= 0.6 || wpa >= 0.8 {
        [240, 200, 80]  // Yellow (Average Contribution)
    } else {
        [230, 90, 90]   // Red (Below Average / Low Contribution)
    }
}

/// Draw text with a shadow (black offset by +1,+1).
fn draw_text_shadow(pm: &mut Pixmap, color: [u8; 3], x: i32, y: i32, scale: PxScale, font: &impl Font, text: &str) {
    draw_text(pm, [0, 0, 0], x + 1, y + 1, scale, font, text);
    draw_text(pm, color, x, y, scale, font, text);
}

// ── Drawing primitives ─────────────────────────────────────────────────────

/// Draw an anti-aliased line.
fn draw_line(pm: &mut Pixmap, x1: f32, y1: f32, x2: f32, y2: f32, color: [u8; 3], alpha: f32, width: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(x1, y1);
    pb.line_to(x2, y2);
    let Some(path) = pb.finish() else { return };
    let paint = color_paint(color, alpha);
    let stroke = Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, miter_limit: 4.0, dash: None };
    pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

/// Draw an anti-aliased filled circle.
fn draw_filled_circle(pm: &mut Pixmap, cx: f32, cy: f32, radius: f32, color: [u8; 3], alpha: f32) {
    let Some(path) = PathBuilder::from_circle(cx, cy, radius) else {
        return;
    };
    let paint = color_paint(color, alpha);
    pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
}

/// Draw an anti-aliased circle outline.
fn draw_circle_outline(pm: &mut Pixmap, cx: f32, cy: f32, radius: f32, color: [u8; 3], alpha: f32, width: f32) {
    let Some(path) = PathBuilder::from_circle(cx, cy, radius) else {
        return;
    };
    let paint = color_paint(color, alpha);
    let stroke = Stroke { width, line_cap: LineCap::Butt, line_join: LineJoin::Miter, miter_limit: 4.0, dash: None };
    pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

/// Draw an anti-aliased dashed circle outline.
fn draw_dashed_circle(pm: &mut Pixmap, cx: f32, cy: f32, radius: f32, color: [u8; 3], alpha: f32, width: f32) {
    let Some(path) = PathBuilder::from_circle(cx, cy, radius) else {
        return;
    };
    let paint = color_paint(color, alpha);
    // Dash pattern: 8px on, 8px off
    let dash = StrokeDash::new(vec![8.0, 8.0], 0.0);
    let stroke = Stroke { width, line_cap: LineCap::Butt, line_join: LineJoin::Miter, miter_limit: 4.0, dash };
    pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

/// Draw a filled rectangle.
fn draw_filled_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: [u8; 3], alpha: f32) {
    let Some(rect) = tiny_skia::Rect::from_xywh(x, y, w, h) else {
        return;
    };
    let paint = color_paint(color, alpha);
    pm.fill_rect(rect, &paint, Transform::identity(), None);
}

fn draw_rounded_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, radius: f32, color: [u8; 3], alpha: f32) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let r = radius.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    let Some(path) = pb.finish() else {
        return;
    };
    let paint = color_paint(color, alpha);
    pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
}

// ── Composite drawing functions ────────────────────────────────────────────

/// Draw a capture point zone: filled circle + progress pie + outline + label.
fn draw_capture_point(
    pm: &mut Pixmap,
    x: f32,
    y: f32,
    radius: f32,
    color: [u8; 3],
    alpha: f32,
    label: &str,
    progress: f32,
    invader_color: Option<[u8; 3]>,
    flag_icon: Option<&RgbaImage>,
    fonts: &GameFonts,
) {
    // Base filled circle with owner's color
    draw_filled_circle(pm, x, y, radius, color, alpha);

    // If capture in progress, draw a pie-slice fill in the invader's color
    if progress > 0.001
        && let Some(inv_color) = invader_color
    {
        let fill_alpha = 0.65;
        // Pie-slice from top (-PI/2), sweeping clockwise by progress * 2*PI
        let start_angle = -std::f32::consts::FRAC_PI_2;
        let sweep = progress * std::f32::consts::TAU;

        let mut pb = PathBuilder::new();
        pb.move_to(x, y);
        // Starting point on circle
        let sx = x + radius * (start_angle).cos();
        let sy = y + radius * (start_angle).sin();
        pb.line_to(sx, sy);

        // Approximate the arc with line segments (smooth enough at this scale)
        let steps = ((sweep / std::f32::consts::TAU) * 64.0).max(4.0) as i32;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let angle = start_angle + sweep * t;
            let px = x + radius * angle.cos();
            let py = y + radius * angle.sin();
            pb.line_to(px, py);
        }
        pb.close();

        if let Some(path) = pb.finish() {
            let paint = color_paint(inv_color, fill_alpha);
            pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }

    // Circle outline
    let outline_color = if let Some(ic) = invader_color
        && progress > 0.001
    {
        ic
    } else {
        color
    };
    draw_circle_outline(pm, x, y, radius, outline_color, 0.85, 2.0);

    // Centered label: icon for base-type, text for domination
    if let Some(icon) = flag_icon {
        draw_icon(pm, icon, x, y);
    } else {
        let scale = fonts.scale(16.0);
        let (tw, th) = text_size(scale, &fonts.primary, label);
        let tx = x as i32 - tw as i32 / 2;
        let ty = y as i32 - th as i32 / 2;
        draw_text_shadow(pm, [255, 255, 255], tx, ty, scale, &fonts.primary, label);
    }
}

/// Draw player name and/or ship name labels centered above a ship icon.
fn draw_ship_labels(
    pm: &mut Pixmap,
    x: f32,
    y: f32,
    player_name: Option<&str>,
    ship_name: Option<&str>,
    name_color: Option<[u8; 3]>,
    fonts: &GameFonts,
    scale_factor: f32,
) {
    let line_height = (12.0 * scale_factor).round() as i32;
    let line_count = player_name.is_some() as i32 + ship_name.is_some() as i32;
    if line_count == 0 {
        return;
    }

    // Apply armament color to ship_name if shown, otherwise player_name
    let color_on_ship = ship_name.is_some();

    let x = x.round() as i32;
    let y = y.round() as i32;

    // Position lines below the icon and health bar
    let icon_offset = (28.0 * scale_factor).round() as i32;
    let base_y = y + icon_offset;
    let mut cur_y = base_y;

    if let Some(name) = player_name {
        let (font, scale) = fonts.font_and_scale(name, 10.0 * scale_factor);
        let color = if !color_on_ship { name_color.unwrap_or([255, 255, 255]) } else { [255, 255, 255] };
        let (w, _) = text_size(scale, font, name);
        let tx = x - w as i32 / 2;
        draw_text_shadow(pm, color, tx, cur_y, scale, font, name);
        cur_y += line_height;
    }
    if let Some(name) = ship_name {
        let (font, scale) = fonts.font_and_scale(name, 12.0 * scale_factor);
        let color = name_color.unwrap_or([255, 255, 255]);
        let (w, _) = text_size(scale, font, name);
        let tx = x - w as i32 / 2;
        draw_text_shadow(pm, color, tx, cur_y, scale, font, name);
    }
}

/// Draw a health bar below a ship icon.
fn draw_health_bar(
    pm: &mut Pixmap,
    x: f32,
    y: f32,
    fraction: f32,
    fill_color: [u8; 3],
    bg_color: [u8; 3],
    bg_alpha: f32,
    scale_factor: f32,
) {
    let bar_w = 20.0f32 * scale_factor;
    let bar_h = 3.0f32 * scale_factor;
    let bar_x = x - bar_w / 2.0;
    let bar_y = y + 16.0 * scale_factor;

    let fill_w = (fraction.clamp(0.0, 1.0) * bar_w).round();

    // Background portion
    if fill_w < bar_w {
        draw_filled_rect(pm, bar_x + fill_w, bar_y, bar_w - fill_w, bar_h, bg_color, bg_alpha);
    }
    // Filled portion
    if fill_w > 0.0 {
        draw_filled_rect(pm, bar_x, bar_y, fill_w, bar_h, fill_color, 1.0);
    }
}

/// Draw a ship icon rotated by yaw, with optional team-color tinting.
///
/// Uses tiny-skia's bilinear-filtered transform compositing for smooth rotation
/// and sub-pixel placement.
fn draw_ship_icon(
    pm: &mut Pixmap,
    icon: &RgbaImage,
    x: f32,
    y: f32,
    yaw: f32,
    color: Option<[u8; 3]>,
    opacity: f32,
    scale_factor: f32,
) {
    let iw = icon.width();
    let ih = icon.height();
    let cx = (iw as f32 * scale_factor) / 2.0;
    let cy = (ih as f32 * scale_factor) / 2.0;

    // Create a tinted copy of the icon as a Pixmap
    let mut icon_pm = Pixmap::new(iw, ih).expect("failed to create icon pixmap");
    let data = icon_pm.data_mut();
    for iy in 0..ih {
        for ix in 0..iw {
            let px = icon.get_pixel(ix, iy).0;
            let idx = (iy * iw + ix) as usize * 4;
            let a = px[3] as f32 / 255.0;
            if a < 0.01 {
                continue;
            }
            let (r, g, b) = if let Some(c) = color {
                // Tint: use luminance as intensity
                let luminance = (px[0] as f32 * 0.299 + px[1] as f32 * 0.587 + px[2] as f32 * 0.114) / 255.0;
                ((c[0] as f32 * luminance) as u8, (c[1] as f32 * luminance) as u8, (c[2] as f32 * luminance) as u8)
            } else {
                (px[0], px[1], px[2])
            };
            // Premultiply
            data[idx] = (r as f32 * a) as u8;
            data[idx + 1] = (g as f32 * a) as u8;
            data[idx + 2] = (b as f32 * a) as u8;
            data[idx + 3] = px[3];
        }
    }

    // The SVG icons point upward (north = -Y). In game coordinates,
    // yaw=0 means east and increases counter-clockwise.
    // Screen rotation: R = PI/2 - yaw, converted to degrees for tiny-skia.
    let angle_deg = (std::f32::consts::FRAC_PI_2 - yaw).to_degrees();

    // Build transform: translate icon center to destination, then rotate, then scale
    let tx = x - cx;
    let ty = y - cy;
    let transform =
        Transform::from_scale(scale_factor, scale_factor).post_translate(tx, ty).post_rotate_at(angle_deg, x, y);

    let paint = PixmapPaint { opacity, blend_mode: BlendMode::SourceOver, quality: FilterQuality::Bilinear };

    pm.draw_pixmap(0, 0, icon_pm.as_ref(), &paint, transform, None);
}

/// Build a padded outline image for a ship icon: the icon's silhouette dilated
/// by `thickness` pixels, colored gold, on a canvas large enough to hold the
/// extension beyond the original icon bounds. The original icon area itself is
/// left transparent so it composites cleanly under the real icon.
fn make_ship_icon_outline(icon: &RgbaImage, thickness: u32, color: [u8; 3], alpha: u8) -> RgbaImage {
    let t = thickness;
    let iw = icon.width();
    let ih = icon.height();
    let ow = iw + 2 * t;
    let oh = ih + 2 * t;
    let mut out = RgbaImage::from_pixel(ow, oh, image::Rgba([0, 0, 0, 0]));

    let alpha_at = |x: i32, y: i32| -> u8 {
        let ix = x - t as i32;
        let iy = y - t as i32;
        if ix < 0 || iy < 0 || ix >= iw as i32 || iy >= ih as i32 {
            0
        } else {
            icon.get_pixel(ix as u32, iy as u32).0[3]
        }
    };

    let t_i = t as i32;
    let t_sq = t_i * t_i;
    for y in 0..oh as i32 {
        for x in 0..ow as i32 {
            if alpha_at(x, y) > 128 {
                continue;
            }
            let mut hit = false;
            'scan: for dy in -t_i..=t_i {
                for dx in -t_i..=t_i {
                    if dx * dx + dy * dy > t_sq {
                        continue;
                    }
                    if alpha_at(x + dx, y + dy) > 128 {
                        hit = true;
                        break 'scan;
                    }
                }
            }
            if hit {
                out.put_pixel(x as u32, y as u32, image::Rgba([color[0], color[1], color[2], alpha]));
            }
        }
    }
    out
}

/// Draw a plane/consumable icon (pre-colored RGBA, no rotation).
fn draw_icon(pm: &mut Pixmap, icon: &RgbaImage, x: f32, y: f32) {
    let iw = icon.width();
    let ih = icon.height();
    let icon_pm = rgba_to_pixmap(icon);
    let tx = x - iw as f32 / 2.0;
    let ty = y - ih as f32 / 2.0;
    let paint = PixmapPaint { opacity: 1.0, blend_mode: BlendMode::SourceOver, quality: FilterQuality::Bilinear };
    pm.draw_pixmap(0, 0, icon_pm.as_ref(), &paint, Transform::from_translate(tx, ty), None);
}

/// Draw the team score bar at the top of the frame.
///
/// Two independent progress bars growing toward the center. Each bar represents
/// progress toward 1000 points. Team 0 (friendly) grows left→center,
/// team 1 (enemy) grows right→center.
fn draw_score_bar(
    pm: &mut Pixmap,
    team0_score: i32,
    team1_score: i32,
    team0_color: [u8; 3],
    team1_color: [u8; 3],
    max_score: i32,
    team0_timer: Option<&str>,
    team1_timer: Option<&str>,
    _advantage_label: &str,
    _advantage_team: i32,
    fonts: &GameFonts,
    map_width: u32,
    scale_factor: f32,
) {
    let width = map_width as f32;
    let sf = scale_factor;
    let progress_h = (60.0 * sf).round();
    let bg_height = progress_h;
    let half = width / 2.0;
    let center_gap = 2.0f32 * sf;
    let bar_max_w = half - center_gap;

    // Dark neutral background overlay across full top bar width
    draw_filled_rect(pm, 0.0, 0.0, width, bg_height, [20, 24, 32], 0.35);

    // Team 0 progress (grows dynamically from left edge toward center based on points, with 15% transparency)
    let t0_frac = (team0_score as f32 / max_score as f32).clamp(0.0, 1.0);
    let t0_width = t0_frac * bar_max_w;
    if t0_width > 0.0 {
        draw_filled_rect(pm, 0.0, 0.0, t0_width, progress_h, team0_color, 0.85);
    }

    // Team 1 progress (grows dynamically from right edge toward center based on points, with 15% transparency)
    let t1_frac = (team1_score as f32 / max_score as f32).clamp(0.0, 1.0);
    let t1_width = t1_frac * bar_max_w;
    if t1_width > 0.0 {
        draw_filled_rect(pm, width - t1_width, 0.0, t1_width, progress_h, team1_color, 0.85);
    }

    let font = &fonts.primary;
    let timer_color: [u8; 3] = [200, 200, 200];
    let pill_pad_x = 6.0f32 * sf;
    let pill_pad_y = 3.0f32 * sf;
    let pill_radius = 4.0f32 * sf;
    let pill_color: [u8; 3] = [10, 12, 16];
    let pill_alpha = 0.70f32;

    let score_scale = fonts.scale(30.0 * sf);
    let timer_scale = fonts.scale(24.0 * sf);

    let t0 = format!("{}", team0_score);
    let t1 = format!("{}", team1_score);
    let (t0w, t0h) = text_size(score_scale, font, &t0);
    let (t1w, _) = text_size(score_scale, font, &t1);

    let pill_h = t0h as f32 + pill_pad_y * 2.0;
    let pill_y = (progress_h - pill_h) / 2.0;
    let text_y = (pill_y + pill_pad_y).round() as i32;

    // ── Measure team 0 pill width ──
    let mut t0_total_w = t0w as f32;
    if let Some(timer) = team0_timer {
        let (tw, _) = text_size(timer_scale, font, timer);
        t0_total_w += 4.0 * sf + tw as f32;
    }

    // ── Measure team 1 pill width ──
    let mut t1_total_w = t1w as f32;
    if let Some(timer) = team1_timer {
        let (tw, _) = text_size(timer_scale, font, timer);
        t1_total_w += 4.0 * sf + tw as f32;
    }

    // ── Draw team 0 pill background + text ──
    let t0_pill_x = 8.0 * sf - pill_pad_x;
    draw_rounded_rect(
        pm,
        t0_pill_x,
        pill_y,
        t0_total_w + pill_pad_x * 2.0,
        pill_h,
        pill_radius,
        pill_color,
        pill_alpha,
    );

    let mut t0_cursor = (8.0 * sf) as i32;
    draw_text(pm, [255, 255, 255], t0_cursor, text_y, score_scale, font, &t0);
    t0_cursor += t0w as i32;
    if let Some(timer) = team0_timer {
        t0_cursor += (4.0 * sf) as i32;
        draw_text(pm, timer_color, t0_cursor, text_y + (1.0 * sf) as i32, timer_scale, font, timer);
    }

    // ── Draw team 1 pill background + text ──
    let t1_pill_x = width - 8.0 * sf - t1_total_w - pill_pad_x * 2.0;
    draw_rounded_rect(
        pm,
        t1_pill_x,
        pill_y,
        t1_total_w + pill_pad_x * 2.0,
        pill_h,
        pill_radius,
        pill_color,
        pill_alpha,
    );

    // Team 1 draws right-to-left: [timer] [score]
    let mut t1_cursor = (width - 8.0 * sf - pill_pad_x) as i32; // right edge of score
    // Score (rightmost)
    let t1_x = t1_cursor - t1w as i32;
    draw_text(pm, [255, 255, 255], t1_x, text_y, score_scale, font, &t1);
    t1_cursor = t1_x;
    // Timer (left of score)
    if let Some(timer) = team1_timer {
        let (tw, _) = text_size(timer_scale, font, timer);
        t1_cursor -= (6.0 * sf) as i32;
        let tx = t1_cursor - tw as i32;
        draw_text(pm, timer_color, tx, text_y + (1.0 * sf) as i32, timer_scale, font, timer);
    }
}

/// Draw the game timer.
fn draw_timer(pm: &mut Pixmap, time_remaining: Option<i64>, elapsed: ElapsedClock, fonts: &GameFonts, map_width: u32) {
    let font = &fonts.primary;
    let center_x = map_width as i32 / 2;
    let main_scale = fonts.scale(48.0);
    let small_scale = fonts.scale(32.0);

    if let Some(remaining) = time_remaining {
        // Show time remaining as main timer (centered)
        let r_mins = remaining / 60;
        let r_secs = remaining % 60;
        let remaining_text = format!("{:02}:{:02}", r_mins, r_secs);
        let (rw, _) = text_size(main_scale, font, &remaining_text);
        let rx = center_x - rw as i32 / 2;
        draw_text_shadow(pm, [255, 255, 255], rx, 4, main_scale, font, &remaining_text);

        // Show elapsed as smaller text below
        let e_mins = (elapsed.seconds() as i32) / 60;
        let e_secs = (elapsed.seconds() as i32) % 60;
        let elapsed_text = format!("+{:02}:{:02}", e_mins, e_secs);
        let (ew, _) = text_size(small_scale, font, &elapsed_text);
        let ex = center_x - ew as i32 / 2;
        draw_text_shadow(pm, [180, 180, 180], ex, 58, small_scale, font, &elapsed_text);
    } else {
        // Fallback: just show elapsed time centered (no timeLeft data yet)
        let mins = (elapsed.seconds() as i32) / 60;
        let secs = (elapsed.seconds() as i32) % 60;
        let text = format!("{:02}:{:02}", mins, secs);
        let (w, _) = text_size(main_scale, font, &text);
        let x = center_x - w as i32 / 2;
        draw_text_shadow(pm, [255, 255, 255], x, 20, main_scale, font, &text);
    }
}

fn draw_pre_battle_countdown(
    pm: &mut Pixmap,
    seconds: i64,
    fonts: &GameFonts,
    resolver: &dyn TextResolver,
    map_width: u32,
) {
    let text = format!("{}", seconds);
    let subtitle = resolver.resolve(&TranslatableText::PreBattleLabel);
    let glow_color: [u8; 3] = [255, 200, 50]; // gold
    draw_battle_result_overlay(pm, &text, Some(&subtitle), glow_color, true, fonts, map_width);
}

/// Map a DeathCause to the icon key used in the death_cause_icons HashMap.
///
/// Keys correspond to the base name portion of `icon_frag_{key}.png` files
/// in `gui/battle_hud/icon_frag/`.
fn death_cause_icon_key(
    cause: &wows_replays::analyzer::decoder::Recognized<wows_replays::analyzer::decoder::DeathCause>,
) -> &'static str {
    use wows_replays::analyzer::decoder::DeathCause;
    match cause.known() {
        Some(DeathCause::Artillery | DeathCause::ApShell | DeathCause::HeShell | DeathCause::CsShell) => "main_caliber",
        Some(DeathCause::Secondaries) => "atba",
        Some(DeathCause::Torpedo | DeathCause::AerialTorpedo) => "torpedo",
        Some(DeathCause::Fire) => "burning",
        Some(DeathCause::Flooding) => "flood",
        Some(DeathCause::DiveBomber) => "bomb",
        Some(DeathCause::SkipBombs) => "skip",
        Some(DeathCause::AerialRocket) => "rocket",
        Some(DeathCause::Detonation) => "detonate",
        Some(DeathCause::Ramming) => "ram",
        Some(DeathCause::DepthCharge | DeathCause::AerialDepthCharge) => "depthbomb",
        Some(DeathCause::Missile) => "missile",
        _ => "main_caliber",
    }
}

/// Draw rich kill feed entries in the top-right corner.
///
/// Layout per line (left-aligned):
/// `KILLER_NAME [icon] ship_name  [cause]  VICTIM_NAME [icon] ship_name`
fn draw_kill_feed(
    pm: &mut Pixmap,
    entries: &[KillFeedEntry],
    fonts: &GameFonts,
    ship_icons: &HashMap<String, ShipIcon>,
    death_cause_icons: &HashMap<String, RgbaImage>,
    left_edge: u32,
) {
    let default_font = &fonts.primary;
    let default_name_scale = fonts.scale(12.0);
    let line_height = 20i32;
    let left_margin = 4i32;
    let icon_size = crate::assets::ICON_SIZE as i32;
    let cause_icon_size = icon_size;
    let gap = 2i32; // gap between elements
    // `left_edge` is the canvas x at which entries align left.
    let start_x = left_edge as i32;

    let (_, text_h) = text_size(default_name_scale, default_font, "Ag");
    let text_h = text_h as i32;

    for (i, entry) in entries.iter().take(5).enumerate() {
        let y = crate::HUD_HEIGHT as i32 + i as i32 * line_height;
        let icon_y = y + (text_h - icon_size) / 2;

        // Get death cause icon key
        let cause_key = death_cause_icon_key(&entry.cause);
        let has_cause_icon = death_cause_icons.contains_key(cause_key);
        let cause_w = if has_cause_icon { cause_icon_size } else { 0 } as u32;

        // Pick appropriate fonts for each text segment
        let (killer_name_font, killer_name_scale) = fonts.font_and_scale(&entry.killer_name, 12.0);
        let (victim_name_font, victim_name_scale) = fonts.font_and_scale(&entry.victim_name, 12.0);
        let killer_ship = entry.killer_ship_name.as_deref().unwrap_or("");
        let victim_ship = entry.victim_ship_name.as_deref().unwrap_or("");
        let (killer_ship_font, killer_ship_scale) = fonts.font_and_scale(killer_ship, 12.0);
        let (victim_ship_font, victim_ship_scale) = fonts.font_and_scale(victim_ship, 12.0);

        // Measure all text segments
        let (killer_name_w, _) = text_size(killer_name_scale, killer_name_font, &entry.killer_name);
        let (killer_ship_w, _) =
            if !killer_ship.is_empty() { text_size(killer_ship_scale, killer_ship_font, killer_ship) } else { (0, 0) };
        let (victim_name_w, _) = text_size(victim_name_scale, victim_name_font, &entry.victim_name);
        let (victim_ship_w, _) =
            if !victim_ship.is_empty() { text_size(victim_ship_scale, victim_ship_font, victim_ship) } else { (0, 0) };

        // Determine if we have icons
        let has_killer_icon =
            entry.killer_species.is_some() && ship_icons.contains_key(entry.killer_species.as_ref().unwrap());
        let has_victim_icon =
            entry.victim_species.is_some() && ship_icons.contains_key(entry.victim_species.as_ref().unwrap());

        // Total width calculation:
        // killer_name [gap icon gap] killer_ship gap cause gap victim_name [gap icon gap] victim_ship
        let mut total_w = killer_name_w as i32;
        if has_killer_icon {
            total_w += gap + icon_size + gap;
        } else if killer_ship_w > 0 {
            total_w += gap;
        }
        if killer_ship_w > 0 {
            total_w += killer_ship_w as i32;
        }
        total_w += gap * 2 + cause_w as i32 + gap * 2;
        total_w += victim_name_w as i32;
        if has_victim_icon {
            total_w += gap + icon_size + gap;
        } else if victim_ship_w > 0 {
            total_w += gap;
        }
        if victim_ship_w > 0 {
            total_w += victim_ship_w as i32;
        }

        // Draw a semi-transparent background for readability
        let bg_x = start_x as f32;
        let bg_y = y as f32 - 1.0;
        draw_filled_rect(pm, bg_x, bg_y, (total_w + left_margin * 2) as f32, (line_height) as f32, [0, 0, 0], 0.5);

        let mut x = start_x + left_margin;

        // Killer name (team-colored)
        draw_text_shadow(pm, entry.killer_color, x, y, killer_name_scale, killer_name_font, &entry.killer_name);
        x += killer_name_w as i32;

        // Killer ship icon (friendly=left, enemy=right)
        if has_killer_icon {
            x += gap;
            let icon = &ship_icons[entry.killer_species.as_ref().unwrap()];
            draw_kill_feed_icon(pm, icon, x, icon_y, icon_size, entry.killer_color, !entry.killer_is_friendly);
            x += icon_size + gap;
        } else if killer_ship_w > 0 {
            x += gap;
        }

        // Killer ship name
        if killer_ship_w > 0 {
            draw_text_shadow(pm, entry.killer_color, x, y, killer_ship_scale, killer_ship_font, killer_ship);
            x += killer_ship_w as i32;
        }

        // Death cause icon (or fallback gap)
        x += gap * 2;
        if let Some(cause_icon) = death_cause_icons.get(cause_key) {
            let cause_center_y = icon_y + cause_icon_size / 2;
            draw_icon(pm, cause_icon, (x + cause_icon_size / 2) as f32, cause_center_y as f32);
        }
        x += cause_w as i32 + gap * 2;

        // Victim name (team-colored)
        draw_text_shadow(pm, entry.victim_color, x, y, victim_name_scale, victim_name_font, &entry.victim_name);
        x += victim_name_w as i32;

        // Victim ship icon (friendly=left, enemy=right)
        if has_victim_icon {
            x += gap;
            let icon = &ship_icons[entry.victim_species.as_ref().unwrap()];
            draw_kill_feed_icon(pm, icon, x, icon_y, icon_size, entry.victim_color, !entry.victim_is_friendly);
            x += icon_size + gap;
        } else if victim_ship_w > 0 {
            x += gap;
        }

        // Victim ship name
        if victim_ship_w > 0 {
            draw_text_shadow(pm, entry.victim_color, x, y, victim_ship_scale, victim_ship_font, victim_ship);
        }
    }
}

/// Draw a small ship icon for the kill feed, tinted with team color.
/// If `flip` is true, the icon faces left (horizontally mirrored).
fn draw_kill_feed_icon(pm: &mut Pixmap, icon: &RgbaImage, x: i32, y: i32, size: i32, color: [u8; 3], flip: bool) {
    let iw = icon.width();
    let ih = icon.height();
    let scale = size as f32 / iw.max(ih) as f32;

    // Create a tinted icon pixmap
    let mut icon_pm = Pixmap::new(iw, ih).expect("failed to create icon pixmap");
    let data = icon_pm.data_mut();
    for iy in 0..ih {
        for ix in 0..iw {
            let px = icon.get_pixel(ix, iy).0;
            let idx = (iy * iw + ix) as usize * 4;
            let a = px[3] as f32 / 255.0;
            if a < 0.01 {
                continue;
            }
            let luminance = (px[0] as f32 * 0.299 + px[1] as f32 * 0.587 + px[2] as f32 * 0.114) / 255.0;
            let r = (color[0] as f32 * luminance) as u8;
            let g = (color[1] as f32 * luminance) as u8;
            let b = (color[2] as f32 * luminance) as u8;
            // Premultiply
            data[idx] = (r as f32 * a) as u8;
            data[idx + 1] = (g as f32 * a) as u8;
            data[idx + 2] = (b as f32 * a) as u8;
            data[idx + 3] = px[3];
        }
    }

    // The ship icons point up (north). For kill feed we want them pointing
    // right (victim) or left (killer). Rotate 90° CW for right, 90° CCW for left.
    let angle_deg = if flip { -90.0 } else { 90.0 };

    let cx = iw as f32 / 2.0;
    let cy = ih as f32 / 2.0;
    // Center the icon at (x + size/2, y + size/2) with scaling
    let dest_cx = x as f32 + size as f32 / 2.0;
    let dest_cy = y as f32 + size as f32 / 2.0;

    let transform = Transform::from_translate(dest_cx - cx * scale, dest_cy - cy * scale)
        .pre_scale(scale, scale)
        .post_rotate_at(angle_deg, dest_cx, dest_cy);

    let paint = PixmapPaint { opacity: 1.0, blend_mode: BlendMode::SourceOver, quality: FilterQuality::Bilinear };

    pm.draw_pixmap(0, 0, icon_pm.as_ref(), &paint, transform, None);
}

/// Word-wrap text to fit within `max_width` pixels, breaking on word boundaries.
///
/// Returns a vector of lines. Each line fits within `max_width` when rendered at `scale`.
fn word_wrap(text: &str, max_width: u32, scale: PxScale, font: &impl Font) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current_line = String::new();

    for word in text.split_whitespace() {
        if current_line.is_empty() {
            // First word on the line — force-break if it alone exceeds max_width
            let (w, _) = text_size(scale, font, word);
            if w > max_width {
                force_break_word(word, max_width, scale, font, &mut lines);
                current_line = lines.pop().unwrap_or_default();
            } else {
                current_line = word.to_string();
            }
        } else {
            let candidate = format!("{} {}", current_line, word);
            let (w, _) = text_size(scale, font, &candidate);
            if w > max_width {
                // Push the current line and start a new one
                lines.push(current_line);
                // The new word itself might also be too wide
                let (ww, _) = text_size(scale, font, word);
                if ww > max_width {
                    force_break_word(word, max_width, scale, font, &mut lines);
                    current_line = lines.pop().unwrap_or_default();
                } else {
                    current_line = word.to_string();
                }
            } else {
                current_line = candidate;
            }
        }
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Break a single word into lines that each fit within `max_width`.
/// Completed lines are pushed to `lines`; the last (possibly partial) segment
/// is also pushed so the caller can pop it to continue accumulating.
fn force_break_word(word: &str, max_width: u32, scale: PxScale, font: &impl Font, lines: &mut Vec<String>) {
    let mut segment = String::new();
    for ch in word.chars() {
        let candidate = format!("{}{}", segment, ch);
        let (w, _) = text_size(scale, font, &candidate);
        if w > max_width && !segment.is_empty() {
            lines.push(segment);
            segment = ch.to_string();
        } else {
            segment = candidate;
        }
    }
    if !segment.is_empty() {
        lines.push(segment);
    }
}

/// Truncate `text` with ellipsis so it fits within `max_width` pixels.
/// Returns the original string if it already fits.
fn truncate_to_fit(text: &str, max_width: u32, scale: PxScale, font: &impl Font) -> String {
    let (w, _) = text_size(scale, font, text);
    if w <= max_width {
        return text.to_string();
    }
    let ellipsis = "..";
    let (ew, _) = text_size(scale, font, ellipsis);
    if ew >= max_width {
        return ellipsis.to_string();
    }
    let target = max_width - ew;
    let mut truncated = String::new();
    for ch in text.chars() {
        let candidate = format!("{}{}", truncated, ch);
        let (cw, _) = text_size(scale, font, &candidate);
        if cw > target {
            break;
        }
        truncated = candidate;
    }
    format!("{}{}", truncated, ellipsis)
}

/// Draw the chat overlay on the left side of the minimap.
///
/// Each entry shows:
///   Line 1: `[CLAN] PlayerName` (truncated to fit)
///   Line 2: `<ship_icon> ShipName:`
///   Line 3+: message text, word-wrapped
/// The whole thing sits in a semi-translucent dark box.
fn draw_chat_overlay(
    pm: &mut Pixmap,
    entries: &[ChatEntry],
    fonts: &GameFonts,
    ship_icons: &HashMap<String, ShipIcon>,
    y_offset: u32,
    x_offset: u32,
) {
    let font = &fonts.primary;
    if entries.is_empty() {
        return;
    }

    let minimap_w = crate::MINIMAP_SIZE;
    let max_box_width = (minimap_w / 4) as i32; // 1/4 of minimap = 192px
    let margin = 6i32;
    let inner_width = (max_box_width - margin * 2) as u32;
    let header_scale = fonts.scale(11.0);
    let msg_scale = fonts.scale(11.0);
    let line_height = 14i32;
    let entry_gap = 6i32;
    let icon_size = 12i32;

    // Pre-compute layout for each entry
    struct EntryLayout {
        /// Line 1: truncated "[CLAN] PlayerName"
        name_line: String,
        /// Color for clan tag portion (if present)
        clan_color: Option<[u8; 3]>,
        /// Length of clan tag prefix (including brackets and space), 0 if none
        clan_prefix_len: usize,
        /// Line 2: ship species key for icon + ship name
        ship_line: Option<String>,
        ship_species: Option<String>,
        /// Message lines (word-wrapped)
        msg_lines: Vec<String>,
        total_height: i32,
    }

    let mut layouts: Vec<EntryLayout> = Vec::new();
    let mut total_height = margin; // top padding

    for entry in entries {
        // Line 1: "[CLAN] PlayerName" -- pick font based on player name
        let clan_prefix = if !entry.clan_tag.is_empty() { format!("[{}] ", entry.clan_tag) } else { String::new() };
        let full_name_line = format!("{}{}", clan_prefix, entry.player_name);
        let (name_font, name_font_scale) = fonts.font_and_scale(&entry.player_name, 11.0);
        let name_line = truncate_to_fit(&full_name_line, inner_width, name_font_scale, name_font);
        let clan_color =
            if !entry.clan_tag.is_empty() { Some(entry.clan_color.unwrap_or(entry.team_color)) } else { None };

        // Line 2: ship icon + ship name (if available)
        let has_ship_line = entry.ship_name.is_some();
        let ship_line = entry.ship_name.as_ref().map(|name| {
            let (sf, ss) = fonts.font_and_scale(name, 11.0);
            let icon_reserved = (icon_size + 2) as u32;
            let available = inner_width.saturating_sub(icon_reserved);
            truncate_to_fit(name, available, ss, sf)
        });

        // Message lines (word-wrapped), using the font selected by font_hint
        let msg_font = match entry.font_hint {
            FontHint::Primary => &fonts.primary,
            FontHint::Fallback(i) => fonts.fallbacks.get(i).unwrap_or(&fonts.primary),
        };
        let entry_msg_scale = fonts.scale_for_hint(11.0, entry.font_hint);
        let msg_lines = word_wrap(&entry.message, inner_width, entry_msg_scale, msg_font);

        let line_count = 1 + has_ship_line as i32 + msg_lines.len() as i32;
        let entry_height = line_count * line_height;

        total_height += entry_height + entry_gap;
        layouts.push(EntryLayout {
            name_line,
            clan_color,
            clan_prefix_len: clan_prefix.len(),
            ship_line,
            ship_species: entry.ship_species.clone(),
            msg_lines,
            total_height: entry_height,
        });
    }
    total_height -= entry_gap; // remove trailing gap
    total_height += margin; // bottom padding

    // Position: middle-left of the minimap area, shifted right by the
    // left-side roster gutter (if any) so chat stays inside the map.
    let box_x = x_offset as i32;
    let map_mid_y = y_offset as i32 + (minimap_w as i32) / 2;
    let box_y = map_mid_y - total_height / 2;

    // Find the overall max opacity for the background alpha
    let max_opacity = entries.iter().map(|e| e.opacity).fold(0.0f32, f32::max);

    // Draw semi-translucent background
    draw_filled_rect(
        pm,
        box_x as f32,
        box_y as f32,
        max_box_width as f32,
        total_height as f32,
        [0, 0, 0],
        0.35 * max_opacity,
    );

    // Draw each entry
    let mut cur_y = box_y + margin;
    for (entry, layout) in entries.iter().zip(layouts.iter()) {
        let opacity = entry.opacity;
        if opacity < 0.01 {
            cur_y += layout.total_height + entry_gap;
            continue;
        }

        let apply_opacity = |color: [u8; 3], op: f32| -> [u8; 3] {
            [(color[0] as f32 * op) as u8, (color[1] as f32 * op) as u8, (color[2] as f32 * op) as u8]
        };

        // Line 1: [CLAN] PlayerName
        let x = box_x + margin;
        if let Some(clan_rgb) = layout.clan_color {
            // Draw clan prefix in clan color, rest in team color
            let clan_text: String = layout.name_line.chars().take(layout.clan_prefix_len).collect();
            let name_text: String = layout.name_line.chars().skip(layout.clan_prefix_len).collect();
            let (cw, _) = text_size(header_scale, font, &clan_text);
            draw_text(pm, apply_opacity(clan_rgb, opacity), x, cur_y, header_scale, font, &clan_text);
            draw_text(
                pm,
                apply_opacity(entry.team_color, opacity),
                x + cw as i32,
                cur_y,
                header_scale,
                font,
                &name_text,
            );
        } else {
            draw_text(pm, apply_opacity(entry.team_color, opacity), x, cur_y, header_scale, font, &layout.name_line);
        }
        cur_y += line_height;

        // Line 2: ship icon + ship name
        if let Some(ref ship_name) = layout.ship_line {
            let mut sx = x;
            if let Some(ref species_key) = layout.ship_species {
                if let Some(icon) = ship_icons.get(species_key) {
                    let icon_y = cur_y + (line_height - icon_size) / 2;
                    draw_kill_feed_icon(
                        pm,
                        icon,
                        sx,
                        icon_y,
                        icon_size,
                        apply_opacity(entry.team_color, opacity),
                        false,
                    );
                }
                sx += icon_size + 2;
            }
            draw_text(pm, apply_opacity(entry.team_color, opacity), sx, cur_y, header_scale, font, ship_name);
            cur_y += line_height;
        }

        // Message lines (using font selected by font_hint)
        let msg_font: &FontArc = match entry.font_hint {
            FontHint::Primary => &fonts.primary,
            FontHint::Fallback(i) => fonts.fallbacks.get(i).unwrap_or(&fonts.primary),
        };
        let msg_color = apply_opacity(entry.message_color, opacity);
        for line in &layout.msg_lines {
            draw_text(pm, msg_color, box_x + margin, cur_y, msg_scale, msg_font, line);
            cur_y += line_height;
        }

        cur_y += entry_gap;
    }
}

/// Draw the 10x10 grid overlay with labels.
fn draw_grid_at(pm: &mut Pixmap, minimap_size: u32, x_off: u32, y_off: u32, fonts: &GameFonts) {
    let font = &fonts.primary;
    let cell = minimap_size as f32 / 10.0;
    let grid_color = [180, 180, 180];
    let alpha = 0.25f32;
    let label_scale = fonts.scale(11.0);
    let x_off_f = x_off as f32;

    // Draw 9 interior lines in each direction
    for i in 1..10 {
        let pos = (i as f32 * cell).round();
        // Vertical line
        draw_line(
            pm,
            x_off_f + pos,
            y_off as f32,
            x_off_f + pos,
            (y_off + minimap_size) as f32,
            grid_color,
            alpha,
            1.0,
        );
        // Horizontal line
        draw_line(
            pm,
            x_off_f,
            pos + y_off as f32,
            x_off_f + minimap_size as f32,
            pos + y_off as f32,
            grid_color,
            alpha,
            1.0,
        );
    }

    // Labels: numbers 1-10 across the bottom, letters A-J down the left
    for i in 0..10 {
        let label = format!("{}", i + 1);
        let x = (x_off_f + i as f32 * cell + cell / 2.0 - 3.0) as i32;
        let y = (y_off + minimap_size) as i32 - 15;
        draw_text_shadow(pm, [255, 255, 255], x, y, label_scale, font, &label);
    }
    let labels_row = ['A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J'];
    for (i, &ch) in labels_row.iter().enumerate() {
        let label = ch.to_string();
        let x = x_off as i32 + 3i32;
        let y = y_off as i32 + (i as f32 * cell + cell / 2.0 - 5.0) as i32;
        draw_text_shadow(pm, [255, 255, 255], x, y, label_scale, font, &label);
    }
}

/// Draw a large centered battle result text with a colored glow effect.
///
/// The glow is created by drawing the text multiple times at increasing offsets
/// with decreasing opacity in the glow color, creating a soft halo effect.
fn draw_battle_result_overlay(
    pm: &mut Pixmap,
    text: &str,
    subtitle: Option<&str>,
    glow_color: [u8; 3],
    subtitle_above: bool,
    fonts: &GameFonts,
    map_width: u32,
) {
    let font = &fonts.primary;
    let w = map_width;
    let h = pm.height();

    let font_height = w as f32 / 8.0;
    let scale = fonts.scale(font_height);
    let (tw, th) = text_size(scale, font, text);

    let sub_scale = fonts.scale(font_height / 4.0);
    let sub_height = subtitle.map(|s| text_size(sub_scale, font, s).1).unwrap_or(0);
    let gap = if subtitle.is_some() { 8 } else { 0 };
    let total_height = th + gap as u32 + sub_height;

    let x = (w as i32 - tw as i32) / 2;
    let center_y = (h as i32 - total_height as i32) / 2;

    // When subtitle is above, subtitle comes first then main text;
    // when below, main text comes first then subtitle.
    let (main_y, sub_y) = if subtitle_above {
        (center_y + sub_height as i32 + gap, center_y)
    } else {
        (center_y, center_y + th as i32 + gap)
    };

    // Dark shadow behind text for contrast, then colored glow, then white text
    let glow_layers: &[(i32, [u8; 3], f32)] = &[
        (8, [0, 0, 0], 0.15),
        (6, [0, 0, 0], 0.25),
        (4, glow_color, 0.30),
        (3, glow_color, 0.45),
        (2, glow_color, 0.60),
        (1, glow_color, 0.80),
    ];
    let offsets: &[(i32, i32)] = &[(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)];
    for &(dist, color, opacity) in glow_layers {
        let c =
            [(color[0] as f32 * opacity) as u8, (color[1] as f32 * opacity) as u8, (color[2] as f32 * opacity) as u8];
        for &(dx, dy) in offsets {
            draw_text(pm, c, x + dx * dist, main_y + dy * dist, scale, font, text);
        }
    }
    draw_text(pm, [255, 255, 255], x, main_y, scale, font, text);

    // Subtitle
    if let Some(sub) = subtitle {
        let (sw, _) = text_size(sub_scale, font, sub);
        let sx = (w as i32 - sw as i32) / 2;
        // Subtle dark outline for readability
        for &(dx, dy) in offsets {
            draw_text(pm, [0, 0, 0], sx + dx * 2, sub_y + dy * 2, sub_scale, font, sub);
        }
        draw_text(pm, [200, 200, 200], sx, sub_y, sub_scale, font, sub);
    }
}

// ── Stats panel helpers ─────────────────────────────────────────────────────

/// Tint a silhouette image (alpha-masked) with a solid color.
fn tint_silhouette(img: &RgbaImage, color: [u8; 3]) -> RgbaImage {
    let mut out = img.clone();
    for px in out.pixels_mut() {
        let a = px.0[3];
        if a > 0 {
            px.0[0] = color[0];
            px.0[1] = color[1];
            px.0[2] = color[2];
            // Keep original alpha
        }
    }
    out
}

/// HP bar color lerp: soft green at full HP, lerped through amber to a muted
/// red. The previous gradient ran between pure primaries (`[0,255,0]` →
/// `[255,255,0]` → `[255,0,0]`) which read as eye-searing lime / sodium yellow
/// against the roster background; these stops desaturate the endpoints while
/// keeping the same semantic gradient.
fn hp_bar_color_lerp(fraction: f32) -> [u8; 3] {
    const GREEN: [f32; 3] = [85.0, 175.0, 110.0];
    const AMBER: [f32; 3] = [220.0, 195.0, 90.0];
    const RED: [f32; 3] = [215.0, 95.0, 85.0];
    let lerp = |a: [f32; 3], b: [f32; 3], t: f32| -> [u8; 3] {
        let t = t.clamp(0.0, 1.0);
        [(a[0] + (b[0] - a[0]) * t) as u8, (a[1] + (b[1] - a[1]) * t) as u8, (a[2] + (b[2] - a[2]) * t) as u8]
    };
    if fraction > 0.5 { lerp(AMBER, GREEN, (fraction - 0.5) / 0.5) } else { lerp(RED, AMBER, fraction / 0.5) }
}

/// Draw an RGBA image at a non-centered position (top-left corner).
fn draw_icon_at(pm: &mut Pixmap, icon: &RgbaImage, x: i32, y: i32) {
    let icon_pm = rgba_to_pixmap(icon);
    pm.draw_pixmap(x, y, icon_pm.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
}

/// Format a number with thousands separators (e.g. 12345 → "12,345").
fn format_number(n: i64) -> String {
    if n < 0 {
        return format!("-{}", format_number(-n));
    }
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

/// Short display name for a ribbon variant (for the compact stats panel).
#[allow(dead_code)]
fn damage_label_color_rgb(label: &str) -> [u8; 3] {
    match label {
        "AP" => [255, 200, 80],
        "HE" => [255, 140, 50],
        "SAP" => [200, 180, 255],
        "MAIN" => [255, 200, 80],
        "SEC" => [255, 170, 60],
        "TORP" => [100, 200, 255],
        "FIRE" => [255, 120, 50],
        "FLOOD" => [80, 160, 255],
        "BOMB" => [220, 180, 100],
        "ROCKET" => [230, 150, 80],
        "DC" => [160, 200, 160],
        "RAM" => [200, 100, 100],
        "MISSILE" => [220, 130, 220],
        _ => [180, 180, 180],
    }
}

// ── ImageTarget (RenderTarget implementation) ──────────────────────────────

use crate::CANVAS_HEIGHT;
use crate::HUD_HEIGHT;
use crate::MINIMAP_SIZE;

use crate::config::RenderOptions;

/// Which side panel the CLI canvas should reserve space for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidePanelLayout {
    /// Just the minimap.
    None,
    /// Self-perspective stats panel on the right.
    StatsPanel,
}

impl SidePanelLayout {
    pub fn from_options(opts: &RenderOptions) -> Self {
        if opts.show_stats_panel {
            Self::StatsPanel
        } else {
            Self::None
        }
    }
}

/// Pre-rasterized ship icon (RGBA, white/alpha mask to be tinted at draw time).
pub type ShipIcon = RgbaImage;

/// Software renderer that draws to a tiny-skia `Pixmap` for anti-aliased output.
///
/// Owns the map image, font, ship icons, plane icons, and consumable icons.
/// Implements `RenderTarget` by dispatching `DrawCommand`s to tiny-skia primitives.
pub struct ImageTarget {
    canvas: Pixmap,
    /// Pre-built background: map image + grid overlay. Cloned at start of each frame.
    base_canvas: Pixmap,
    /// Width of the minimap area (excludes side panels). Always `MINIMAP_SIZE`.
    map_width: u32,
    /// Horizontal offset (in canvas pixels) from the canvas's left edge to the
    /// minimap's left edge. Non-zero when team rosters reserve a left gutter.
    /// Map-coordinate draw commands add this offset; HUD elements that span
    /// the full canvas keep using `canvas.width()` directly.
    map_x_offset: u32,
    /// Width of the HUD strip. With team rosters, this spans the rosters plus
    /// the minimap so the score bar can stretch across both gutters; with the
    /// stats panel (or no side panel) it's just the minimap width.
    hud_width: u32,
    fonts: GameFonts,
    ship_icons: HashMap<String, ShipIcon>,
    /// Pre-computed gold outline halos for ship icons. Same keys as `ship_icons`;
    /// each image is `ship_icons[key]` padded by [`SHIP_ICON_OUTLINE_THICKNESS`]
    /// pixels with a dilated silhouette painted gold.
    ship_icon_outlines: HashMap<String, RgbaImage>,
    plane_icons: HashMap<String, RgbaImage>,
    building_icons: HashMap<String, RgbaImage>,
    consumable_icons: HashMap<String, RgbaImage>,
    ribbon_icons: HashMap<String, RgbaImage>,
    subribbon_icons: HashMap<String, RgbaImage>,
    death_cause_icons: HashMap<String, RgbaImage>,
    powerup_icons: HashMap<String, RgbaImage>,
    /// Bounding rects [x, y, w, h] of previously placed config-circle labels in the current frame.
    placed_labels: Vec<[i32; 4]>,
    /// Resolves translatable game text (battle results, advantage labels, etc.)
    text_resolver: Arc<dyn TextResolver>,
    pub large_elements: bool,
    pub aspect_ratio_16_9: bool,
    pub inkpads_layout: bool,
}

impl ImageTarget {
    pub fn new(
        map_image: Option<RgbImage>,
        fonts: GameFonts,
        ship_icons: HashMap<String, ShipIcon>,
        plane_icons: HashMap<String, RgbaImage>,
        building_icons: HashMap<String, RgbaImage>,
        consumable_icons: HashMap<String, RgbaImage>,
        ribbon_icons: HashMap<String, RgbaImage>,
        subribbon_icons: HashMap<String, RgbaImage>,
        death_cause_icons: HashMap<String, RgbaImage>,
        powerup_icons: HashMap<String, RgbaImage>,
    ) -> Self {
        Self::with_stats_panel(
            map_image,
            fonts,
            ship_icons,
            plane_icons,
            building_icons,
            consumable_icons,
            ribbon_icons,
            subribbon_icons,
            death_cause_icons,
            powerup_icons,
            false,
            false,
            false,
        )
    }

    pub fn with_stats_panel(
        map_image: Option<RgbImage>,
        fonts: GameFonts,
        ship_icons: HashMap<String, ShipIcon>,
        plane_icons: HashMap<String, RgbaImage>,
        building_icons: HashMap<String, RgbaImage>,
        consumable_icons: HashMap<String, RgbaImage>,
        ribbon_icons: HashMap<String, RgbaImage>,
        subribbon_icons: HashMap<String, RgbaImage>,
        death_cause_icons: HashMap<String, RgbaImage>,
        powerup_icons: HashMap<String, RgbaImage>,
        stats_panel: bool,
        large_elements: bool,
        aspect_ratio_16_9: bool,
    ) -> Self {
        let layout = if stats_panel { SidePanelLayout::StatsPanel } else { SidePanelLayout::None };
        Self::with_side_panel(
            map_image,
            fonts,
            ship_icons,
            plane_icons,
            building_icons,
            consumable_icons,
            ribbon_icons,
            subribbon_icons,
            death_cause_icons,
            powerup_icons,
            layout,
            large_elements,
            aspect_ratio_16_9,
            None,
        )
    }

    pub fn with_side_panel(
        map_image: Option<RgbImage>,
        fonts: GameFonts,
        ship_icons: HashMap<String, ShipIcon>,
        plane_icons: HashMap<String, RgbaImage>,
        building_icons: HashMap<String, RgbaImage>,
        consumable_icons: HashMap<String, RgbaImage>,
        ribbon_icons: HashMap<String, RgbaImage>,
        subribbon_icons: HashMap<String, RgbaImage>,
        death_cause_icons: HashMap<String, RgbaImage>,
        powerup_icons: HashMap<String, RgbaImage>,
        layout: SidePanelLayout,
        large_elements: bool,
        aspect_ratio_16_9: bool,
        stats_panel_width_override: Option<u32>,
    ) -> Self {
        let map = map_image.unwrap_or_else(|| RgbImage::from_pixel(MINIMAP_SIZE, MINIMAP_SIZE, Rgb([30, 40, 60])));

        let base_panel_width = if aspect_ratio_16_9 { STATS_PANEL_WIDTH_16_9 } else { STATS_PANEL_WIDTH };
        let mut panel_width = if let Some(ow) = stats_panel_width_override {
            ow
        } else if large_elements {
            (base_panel_width as f32 * 1.8f32).round() as u32
        } else {
            base_panel_width
        };
        if panel_width % 2 != 0 {
            panel_width += 1;
        }
        let (canvas_width, map_x_offset, hud_width) = match layout {
            SidePanelLayout::None => (MINIMAP_SIZE, 0, MINIMAP_SIZE),
            SidePanelLayout::StatsPanel => (MINIMAP_SIZE + panel_width, 0, MINIMAP_SIZE),
        };

        // Pre-build the base canvas: dark background + map + grid
        let mut base_rgb = RgbImage::from_pixel(canvas_width, CANVAS_HEIGHT, Rgb([20, 25, 35]));
        for y in 0..map.height().min(MINIMAP_SIZE) {
            for x in 0..map.width().min(MINIMAP_SIZE) {
                base_rgb.put_pixel(x + map_x_offset, y + HUD_HEIGHT, *map.get_pixel(x, y));
            }
        }
        let mut base = rgb_to_pixmap(&base_rgb);
        draw_grid_at(&mut base, MINIMAP_SIZE, map_x_offset, HUD_HEIGHT, &fonts);

        let ship_icon_outlines = ship_icons
            .iter()
            .map(|(k, icon)| (k.clone(), make_ship_icon_outline(icon, SHIP_ICON_OUTLINE_THICKNESS, [255, 215, 0], 230)))
            .collect();

        Self {
            canvas: Pixmap::new(canvas_width, CANVAS_HEIGHT).unwrap(),
            base_canvas: base,
            map_width: MINIMAP_SIZE,
            map_x_offset,
            hud_width,
            fonts,
            ship_icons,
            ship_icon_outlines,
            plane_icons,
            building_icons,
            consumable_icons,
            ribbon_icons,
            subribbon_icons,
            death_cause_icons,
            powerup_icons,
            placed_labels: Vec::new(),
            text_resolver: Arc::new(DefaultTextResolver),
            large_elements,
            aspect_ratio_16_9,
            inkpads_layout: false,
        }
    }

    /// Set a custom text resolver for translating game text.
    pub fn set_text_resolver(&mut self, resolver: Arc<dyn TextResolver>) {
        self.text_resolver = resolver;
    }

    /// Access the current frame as an RGB image (converted from Pixmap).
    pub fn frame(&self) -> RgbImage {
        pixmap_to_rgb(&self.canvas)
    }

    /// Canvas dimensions.
    pub fn canvas_size(&self) -> (u32, u32) {
        (self.canvas.width(), self.canvas.height())
    }
}

impl RenderTarget for ImageTarget {
    fn begin_frame(&mut self) {
        self.canvas = self.base_canvas.clone();
        self.placed_labels.clear();
    }

    fn draw(&mut self, cmd: &DrawCommand) {
        let y_off = HUD_HEIGHT as f32;
        let x_off = self.map_x_offset as f32;
        match cmd {
            DrawCommand::ShotTracer { from, to, color } => {
                draw_line(
                    &mut self.canvas,
                    from.x + x_off,
                    from.y + y_off,
                    to.x + x_off,
                    to.y + y_off,
                    *color,
                    1.0,
                    1.5,
                );
            }
            DrawCommand::ShotTracerTip { at, color } => {
                // A bit wider than the 1.5px tracer line so the ammo color is noticeable.
                draw_filled_circle(
                    &mut self.canvas,
                    at.x + x_off,
                    at.y + y_off,
                    1.9,
                    *color,
                    crate::draw_command::SHOT_TIP_ALPHA,
                );
            }
            DrawCommand::SecondaryShotTracer { from, to, color } => {
                draw_line(
                    &mut self.canvas,
                    from.x + x_off,
                    from.y + y_off,
                    to.x + x_off,
                    to.y + y_off,
                    *color,
                    crate::draw_command::SECONDARY_SHOT_ALPHA,
                    1.5,
                );
            }
            DrawCommand::SecondaryShotTracerTip { at, color } => {
                draw_filled_circle(
                    &mut self.canvas,
                    at.x + x_off,
                    at.y + y_off,
                    1.9,
                    *color,
                    crate::draw_command::SECONDARY_SHOT_ALPHA,
                );
            }
            DrawCommand::Torpedo { pos, color } => {
                draw_filled_circle(&mut self.canvas, pos.x + x_off, pos.y + y_off, 2.5, *color, 1.0);
            }
            DrawCommand::Smoke { pos, radius, color, alpha } => {
                draw_filled_circle(&mut self.canvas, pos.x + x_off, pos.y + y_off, *radius as f32, *color, *alpha);
            }
            DrawCommand::BuffZone { pos, radius, color, alpha, marker_name } => {
                let cx = pos.x + x_off;
                let cy = pos.y + y_off;
                let r = *radius as f32;
                // Filled circle
                draw_filled_circle(&mut self.canvas, cx, cy, r, *color, *alpha);
                // Border ring
                draw_circle_outline(&mut self.canvas, cx, cy, r, *color, 0.6, 1.5);
                // Draw powerup icon centered on zone
                if let Some(name) = marker_name
                    && let Some(icon) = self.powerup_icons.get(name.as_str())
                {
                    draw_icon(&mut self.canvas, icon, cx, cy);
                }
            }
            DrawCommand::CapturePoint { pos, radius, color, alpha, label, progress, invader_color, flag_icon } => {
                draw_capture_point(
                    &mut self.canvas,
                    pos.x + x_off,
                    pos.y + y_off,
                    *radius as f32,
                    *color,
                    *alpha,
                    label,
                    *progress,
                    *invader_color,
                    flag_icon.as_ref(),
                    &self.fonts,
                );
            }
            DrawCommand::CameraDirection { pos, yaw, color, length, .. } => {
                let x = pos.x + x_off;
                let y = pos.y + y_off;
                let dx = *length as f32 * yaw.cos();
                let dy = -*length as f32 * yaw.sin();
                draw_line(&mut self.canvas, x, y, x + dx, y + dy, *color, 0.7, 1.0);
            }
            DrawCommand::Building { pos, color, icon_type, relation, .. } => {
                // Try to render an icon; fall back to a dot if no icon is available
                let icon_key = icon_type.map(|t| format!("{}_{}", t.icon_name(), relation.icon_suffix()));
                let icon = icon_key.as_ref().and_then(|k| self.building_icons.get(k));
                if let Some(icon) = icon {
                    draw_icon(&mut self.canvas, icon, pos.x + x_off, pos.y + y_off);
                } else {
                    draw_filled_circle(&mut self.canvas, pos.x + x_off, pos.y + y_off, 2.5, *color, 1.0);
                }
            }
            DrawCommand::WeatherZone { pos, radius } => {
                // Semi-transparent light gray circle for weather zones (squalls/storms)
                draw_filled_circle(
                    &mut self.canvas,
                    pos.x + x_off,
                    pos.y + y_off,
                    *radius as f32,
                    [255, 255, 255],
                    0.25,
                );
            }
            DrawCommand::Ship {
                pos,
                yaw,
                species,
                color,
                visibility,
                opacity,
                is_self,
                player_name,
                ship_name,
                is_detected_teammate,
                is_disconnected,
                name_color,
                ..
            } => {
                let x = pos.x + x_off;
                let y = pos.y + y_off;

                let fallback_key = match (*visibility, *is_self) {
                    (ShipVisibility::Visible, true) => "Auxiliary_self",
                    (ShipVisibility::Visible, false) => "Auxiliary",
                    (ShipVisibility::MinimapOnly | ShipVisibility::Undetected, _) => "Auxiliary_invisible",
                };
                let lookup_keys: Vec<String> = if let Some(sp) = species.as_ref() {
                    let variant_key = match (*visibility, *is_self) {
                        (ShipVisibility::Visible, true) => format!("{}_self", sp),
                        (ShipVisibility::Visible, false) => sp.clone(),
                        (ShipVisibility::MinimapOnly, _) => format!("{}_invisible", sp),
                        (ShipVisibility::Undetected, _) => format!("{}_invisible", sp),
                    };
                    vec![variant_key, sp.clone(), fallback_key.to_string()]
                } else {
                    vec![fallback_key.to_string()]
                };

                let Some((icon_key, icon)) =
                    lookup_keys.iter().find_map(|k| self.ship_icons.get(k).map(|i| (k.as_str(), i)))
                else {
                    return;
                };

                let scale_factor = if self.large_elements { 1.1f32 } else { 1.0f32 };

                if *is_detected_teammate && let Some(outline) = self.ship_icon_outlines.get(icon_key) {
                    draw_ship_icon(&mut self.canvas, outline, x, y, *yaw, None, 1.0, scale_factor);
                }
                if *is_disconnected && let Some(outline) = self.ship_icon_outlines.get(icon_key) {
                    draw_ship_icon(&mut self.canvas, outline, x, y, *yaw, Some([255, 60, 60]), 1.0, scale_factor);
                }

                draw_ship_icon(&mut self.canvas, icon, x, y, *yaw, color.map(|c| c), *opacity, scale_factor);
                draw_ship_labels(
                    &mut self.canvas,
                    x,
                    y,
                    player_name.as_deref(),
                    ship_name.as_deref(),
                    *name_color,
                    &self.fonts,
                    scale_factor,
                );
            }
            DrawCommand::HealthBar { pos, fraction, fill_color, background_color, background_alpha, .. } => {
                let scale_factor = if self.large_elements { 1.1f32 } else { 1.0f32 };
                draw_health_bar(
                    &mut self.canvas,
                    pos.x + x_off,
                    pos.y + y_off,
                    *fraction,
                    *fill_color,
                    *background_color,
                    *background_alpha,
                    scale_factor,
                );
            }
            DrawCommand::DeadShip { pos, yaw, species, color, is_self, .. } => {
                let x = pos.x + x_off;
                let y = pos.y + y_off;

                let fallback_key = if *is_self { "Auxiliary_dead_self" } else { "Auxiliary_dead" };
                let icon = if let Some(sp) = species.as_ref() {
                    let variant_key = if *is_self { format!("{}_dead_self", sp) } else { format!("{}_dead", sp) };
                    self.ship_icons
                        .get(&variant_key)
                        .or_else(|| self.ship_icons.get(sp))
                        .or_else(|| self.ship_icons.get(fallback_key))
                } else {
                    self.ship_icons.get(fallback_key)
                };

                let Some(icon) = icon else {
                    return;
                };

                let scale_factor = if self.large_elements { 1.1f32 } else { 1.0f32 };
                draw_ship_icon(&mut self.canvas, icon, x, y, *yaw, color.map(|c| c), 1.0, scale_factor);
            }
            DrawCommand::Plane { pos, icon_key, player_name, ship_name, .. } => {
                let x = pos.x + x_off;
                let y = pos.y + y_off;
                let Some(icon) = self.plane_icons.get(icon_key) else {
                    tracing::warn!(icon_key, "Missing plane icon, skipping");
                    return;
                };
                draw_icon(&mut self.canvas, icon, x, y);
                draw_ship_labels(
                    &mut self.canvas,
                    x,
                    y,
                    player_name.as_deref(),
                    ship_name.as_deref(),
                    None,
                    &self.fonts,
                    1.0,
                );
            }
            DrawCommand::ConsumableRadius { pos, radius_px, color, alpha, .. } => {
                let x = pos.x + x_off;
                let y = pos.y + y_off;
                // Semi-transparent filled circle
                draw_filled_circle(&mut self.canvas, x, y, *radius_px as f32, *color, *alpha);
                // Outline for visibility
                draw_circle_outline(&mut self.canvas, x, y, *radius_px as f32, *color, 0.5, 2.0);
            }
            DrawCommand::PatrolRadius { pos, radius_px, color, alpha, .. } => {
                let x = pos.x + x_off;
                let y = pos.y + y_off;
                // Filled circle only, no outline
                draw_filled_circle(&mut self.canvas, x, y, *radius_px as f32, *color, *alpha);
            }
            DrawCommand::ConsumableIcons { pos, icon_keys, has_hp_bar, .. } => {
                let x = (pos.x + x_off).round() as i32;
                let y = (pos.y + y_off).round() as i32;
                let base_y = if *has_hp_bar { y + 28 } else { y + 26 };
                let icon_size = 28i32;
                let gap = 1i32;
                let count = icon_keys.len() as i32;
                let total_w = count * icon_size + (count - 1) * gap;
                let start_x = x - total_w / 2 + icon_size / 2;
                for (i, icon_key) in icon_keys.iter().enumerate() {
                    if let Some(icon) = self.consumable_icons.get(icon_key) {
                        let ix = start_x + i as i32 * (icon_size + gap);
                        draw_icon(&mut self.canvas, icon, ix as f32, base_y as f32);
                    }
                }
            }
            DrawCommand::ScoreBar {
                team0,
                team1,
                team0_color,
                team1_color,
                max_score,
                team0_timer,
                team1_timer,
                advantage,
            } => {
                let advantage_label = advantage
                    .map(|(level, _)| self.text_resolver.resolve(&wt_translations::TranslatableText::Advantage(level)))
                    .unwrap_or_default();
                let advantage_team = advantage.map(|(_, team)| team as i32).unwrap_or(-1);
                let scale_factor = if self.large_elements { 1.8f32 } else { 1.0f32 };
                draw_score_bar(
                    &mut self.canvas,
                    *team0,
                    *team1,
                    *team0_color,
                    *team1_color,
                    *max_score,
                    team0_timer.as_deref(),
                    team1_timer.as_deref(),
                    &advantage_label,
                    advantage_team,
                    &self.fonts,
                    self.hud_width,
                    scale_factor,
                );
            }
            DrawCommand::TeamAdvantage { .. } => {
                // Advantage is now rendered inline in the score bar;
                // this command is retained for consumers that want the breakdown data.
            }
            DrawCommand::Timer { time_remaining, elapsed } => {
                draw_timer(&mut self.canvas, *time_remaining, *elapsed, &self.fonts, self.hud_width);
            }
            DrawCommand::PreBattleCountdown { seconds } => {
                draw_pre_battle_countdown(
                    &mut self.canvas,
                    *seconds,
                    &self.fonts,
                    &*self.text_resolver,
                    self.hud_width,
                );
            }
            DrawCommand::TeamBuffs { friendly_buffs, enemy_buffs } => {
                let icon_size = 16i32;
                let gap = 2i32;
                let buff_y = 64;
                let count_scale = self.fonts.scale(10.0);

                // Friendly buffs: left side of the minimap (shifted right by
                // the roster gutter when team rosters are on).
                let mut x = self.map_x_offset as i32 + 4i32;
                for (marker, count) in friendly_buffs {
                    if let Some(icon) = self.powerup_icons.get(marker.as_str()) {
                        let resized = image::imageops::resize(
                            icon,
                            icon_size as u32,
                            icon_size as u32,
                            image::imageops::FilterType::Nearest,
                        );
                        draw_icon(
                            &mut self.canvas,
                            &resized,
                            (x + icon_size / 2) as f32,
                            (buff_y + icon_size / 2) as f32,
                        );
                        if *count > 1 {
                            let label = format!("{}", count);
                            draw_text_shadow(
                                &mut self.canvas,
                                [255, 255, 255],
                                x + icon_size,
                                buff_y + 4,
                                count_scale,
                                &self.fonts.primary,
                                &label,
                            );
                            let (tw, _) = text_size(count_scale, &self.fonts.primary, &label);
                            x += icon_size + tw as i32 + gap;
                        } else {
                            x += icon_size + gap;
                        }
                    }
                }

                // Enemy buffs: right side, starting from the minimap's right
                // edge (map_x_offset + map_width) rather than the canvas edge.
                let width = (self.map_x_offset + self.map_width) as i32;
                let mut x = width - 4;
                for (marker, count) in enemy_buffs {
                    if let Some(icon) = self.powerup_icons.get(marker.as_str()) {
                        let resized = image::imageops::resize(
                            icon,
                            icon_size as u32,
                            icon_size as u32,
                            image::imageops::FilterType::Nearest,
                        );
                        if *count > 1 {
                            let label = format!("{}", count);
                            let (tw, _) = text_size(count_scale, &self.fonts.primary, &label);
                            x -= tw as i32;
                            draw_text_shadow(
                                &mut self.canvas,
                                [255, 255, 255],
                                x,
                                buff_y + 4,
                                count_scale,
                                &self.fonts.primary,
                                &label,
                            );
                            x -= icon_size;
                        } else {
                            x -= icon_size;
                        }
                        draw_icon(
                            &mut self.canvas,
                            &resized,
                            (x + icon_size / 2) as f32,
                            (buff_y + icon_size / 2) as f32,
                        );
                        x -= gap;
                    }
                }
            }
            DrawCommand::PositionTrail { points, .. } => {
                for (pos, color) in points {
                    draw_filled_circle(&mut self.canvas, pos.x + x_off, pos.y + y_off, 1.0, *color, 1.0);
                }
            }
            DrawCommand::ShipConfigCircle { pos, radius_px, color, alpha, dashed, label, .. } => {
                let x = pos.x + x_off;
                let y = pos.y + y_off;
                let r = *radius_px;
                if *dashed {
                    draw_dashed_circle(&mut self.canvas, x, y, r, *color, *alpha, 1.0);
                } else {
                    draw_circle_outline(&mut self.canvas, x, y, r, *color, *alpha, 1.0);
                }
                if let Some(text) = label {
                    let font = self.fonts.font_for_text(text);
                    let scale = self.fonts.scale(11.0);
                    let (tw, th) = text_size(scale, font, text);
                    let tw = tw as i32;
                    let th = th as i32;
                    let cx = x as i32;
                    let cy = y as i32;
                    let ri = *radius_px as i32;
                    let gap = 3;

                    // 8 candidate angles: top, top-right, right, bottom-right, bottom, bottom-left, left, top-left
                    let candidates: [(f32, f32); 8] = [
                        (0.0, -1.0),      // top
                        (0.707, -0.707),  // top-right
                        (1.0, 0.0),       // right
                        (0.707, 0.707),   // bottom-right
                        (0.0, 1.0),       // bottom
                        (-0.707, 0.707),  // bottom-left
                        (-1.0, 0.0),      // left
                        (-0.707, -0.707), // top-left
                    ];

                    let compute_rect = |cos: f32, sin: f32| -> [i32; 4] {
                        let ax = cx + ((ri + gap) as f32 * cos) as i32;
                        let ay = cy + ((ri + gap) as f32 * sin) as i32;
                        let lx = if cos < -0.3 {
                            ax - tw
                        } else if cos > 0.3 {
                            ax
                        } else {
                            ax - tw / 2
                        };
                        let ly = if sin < -0.3 {
                            ay - th
                        } else if sin > 0.3 {
                            ay
                        } else {
                            ay - th / 2
                        };
                        [lx, ly, tw, th]
                    };

                    let rects_overlap = |a: &[i32; 4], b: &[i32; 4]| -> bool {
                        a[0] < b[0] + b[2] && a[0] + a[2] > b[0] && a[1] < b[1] + b[3] && a[1] + a[3] > b[1]
                    };

                    let mut best = compute_rect(candidates[0].0, candidates[0].1);
                    for &(cos, sin) in &candidates {
                        let rect = compute_rect(cos, sin);
                        let overlaps = self.placed_labels.iter().any(|prev| rects_overlap(prev, &rect));
                        if !overlaps {
                            best = rect;
                            break;
                        }
                    }

                    self.placed_labels.push(best);
                    draw_text_shadow(&mut self.canvas, *color, best[0], best[1], scale, font, text);
                }
            }
            DrawCommand::KillFeed { entries } => {
                draw_kill_feed(
                    &mut self.canvas,
                    entries,
                    &self.fonts,
                    &self.ship_icons,
                    &self.death_cause_icons,
                    self.map_x_offset,
                );
            }
            DrawCommand::ChatOverlay { entries } => {
                draw_chat_overlay(&mut self.canvas, entries, &self.fonts, &self.ship_icons, 64, self.map_x_offset);
            }
            DrawCommand::BattleResultOverlay { result, finish_type, color, subtitle_above, custom_subtitle } => {
                let text = self.text_resolver.resolve(&TranslatableText::BattleResult(*result));
                let subtitle = custom_subtitle.clone().or_else(|| {
                    finish_type.as_ref().map(|ft| {
                        if let wowsunpack::recognized::Recognized::Known(wowsunpack::game_types::FinishType::Score)
                            | wowsunpack::recognized::Recognized::Known(wowsunpack::game_types::FinishType::ScoreExcess) = ft
                        {
                            match result {
                                wowsunpack::game_types::BattleResult::Victory => "YOUR TEAM WON ON POINTS".to_string(),
                                wowsunpack::game_types::BattleResult::Defeat => "ENEMY TEAM WON ON POINTS".to_string(),
                                wowsunpack::game_types::BattleResult::Draw => "POINTS LIMIT REACHED".to_string(),
                            }
                        } else {
                            self.text_resolver.resolve(&TranslatableText::FinishType(ft.clone())).to_uppercase()
                        }
                    })
                });
                draw_battle_result_overlay(
                    &mut self.canvas,
                    &text,
                    subtitle.as_deref(),
                    *color,
                    *subtitle_above,
                    &self.fonts,
                    self.hud_width,
                );
            }
            DrawCommand::StatsPanel { x, width } => {
                // Dark background for the stats panel area
                draw_filled_rect(
                    &mut self.canvas,
                    *x as f32,
                    0.0,
                    *width as f32,
                    CANVAS_HEIGHT as f32,
                    [30, 34, 42],
                    0.96,
                );
                // Subtle left border
                draw_line(&mut self.canvas, *x as f32, 0.0, *x as f32, CANVAS_HEIGHT as f32, [55, 60, 72], 0.8, 1.0);
            }
            DrawCommand::StatsSilhouette {
                x,
                y,
                width,
                height,
                ship_param_id: _,
                hp_fraction,
                hp_current,
                hp_max,
                hp_healable,
                hp_healable_per_charge,
                heal_availability,
                player_name,
                clan_tag,
                clan_color,
                ship_name,
                silhouette,
                emblem,
            } => {
                let scale_factor = if self.large_elements { 1.8f32 } else { 1.0f32 };
                let padding = (8.0 * scale_factor).round() as i32;
                
                // Left column width is 57% of the panel width (as requested)
                let left_w = (*width * 57) / 100;
                let right_w = *width - left_w;
                let inner_x = *x + padding;
                let inner_w = left_w - padding * 2;
                let right_center_x = *x + left_w + right_w / 2;

                // Let's compute text_group_h for player and ship name labels
                let show_player = player_name.is_some() || clan_tag.as_ref().is_some_and(|t| !t.is_empty());
                let show_ship = ship_name.is_some();
                let mut text_group_h = 0;
                if show_player {
                    text_group_h += (22.0 * scale_factor).round() as i32;
                }
                if show_ship {
                    text_group_h += (24.0 * scale_factor).round() as i32;
                }

                // Compute total height of the left stack to center it vertically:
                let top_section_h = *height;
                let emblem_size = (72.0 * scale_factor).round() as i32;
                let gap1 = (12.0 * scale_factor).round() as i32;
                let target_sil_h = (90.0 * scale_factor).round() as i32;
                let gap2 = (10.0 * scale_factor).round() as i32;
                let hp_text_h = (20.0 * scale_factor).round() as i32;
                let gap3 = (14.0 * scale_factor).round() as i32;
                let cons_size = (32.0 * scale_factor).round() as i32;

                let left_total_h = text_group_h + gap1 + target_sil_h + gap2 + hp_text_h + gap3 + cons_size;
                
                let start_y = *y + (top_section_h - left_total_h).max(0) / 4;

                // Draw emblem (dog tag) centered in right 43% starting at emblem_y
                let emblem_y = start_y + (4.0 * scale_factor).round() as i32;
                if let Some(emb_img) = emblem {
                    let emblem_x = right_center_x - (emblem_size as i32) / 2;
                    let resized = image::imageops::resize(
                        emb_img,
                        emblem_size as u32,
                        emblem_size as u32,
                        image::imageops::FilterType::Lanczos3,
                    );
                    draw_icon_at(&mut self.canvas, &resized, emblem_x, emblem_y);
                }

                // Draw clan tag + player name, and ship name centered vertically in left 57%
                let mut label_y = start_y;

                if show_player {
                    let clan_prefix = clan_tag.as_ref().filter(|t| !t.is_empty()).map(|t| format!("[{t}] "));
                    let name_part = player_name.as_deref().unwrap_or("");

                    let (name_font, name_font_scale) = self.fonts.font_and_scale(name_part, 16.0 * scale_factor);

                    let clan_w =
                        clan_prefix.as_ref().map(|cp| text_size(name_font_scale, name_font, cp).0).unwrap_or(0);
                    let name_w =
                        if !name_part.is_empty() { text_size(name_font_scale, name_font, name_part).0 } else { 0 };
                    let total_w = clan_w + name_w;
                    let mut cx = inner_x + (inner_w - total_w as i32) / 2;

                    if let Some(ref cp) = clan_prefix {
                        let cc = clan_color.unwrap_or([255, 255, 255]);
                        draw_text_shadow(&mut self.canvas, cc, cx, label_y, name_font_scale, name_font, cp);
                        cx += clan_w as i32;
                    }
                    if !name_part.is_empty() {
                        draw_text_shadow(
                            &mut self.canvas,
                            [255, 255, 255],
                            cx,
                            label_y,
                            name_font_scale,
                            name_font,
                            name_part,
                        );
                    }
                    label_y += (22.0 * scale_factor).round() as i32;
                }
                if let Some(name) = ship_name {
                    let (ship_font, ship_font_scale) = self.fonts.font_and_scale(name, 20.0 * scale_factor);
                    let (tw, _) = text_size(ship_font_scale, ship_font, name);
                    let tx = inner_x + (inner_w - tw as i32) / 2;
                    draw_text_shadow(&mut self.canvas, [180, 180, 180], tx, label_y, ship_font_scale, ship_font, name);
                }

                let left_second_y = start_y + text_group_h + gap1;
                let sil_y = left_second_y;
                let hp_text_y = sil_y + target_sil_h + gap2;

                if let Some(sil_img) = silhouette {
                    let aspect = sil_img.width() as f32 / sil_img.height() as f32;
                    let fit_w = inner_w as u32;
                    let fit_h = target_sil_h as u32;
                    let (draw_w, draw_h) = if aspect > (fit_w as f32 / fit_h as f32) {
                        (fit_w, (fit_w as f32 / aspect) as u32)
                    } else {
                        ((fit_h as f32 * aspect) as u32, fit_h)
                    };

                    let scale_mult = 1.6f32;
                    let draw_w = ((draw_w as f32 * scale_mult) as u32).min(fit_w).max(1);
                    let draw_h = ((draw_h as f32 * scale_mult) as u32).min(fit_h + 40).max(1);

                    let base_sil = tint_silhouette(sil_img, crate::draw_command::SILHOUETTE_BASE_RGB);
                    let resized_base =
                        image::imageops::resize(&base_sil, draw_w, draw_h, image::imageops::FilterType::Triangle);
                    let sil_x = inner_x + (inner_w - draw_w as i32) / 2;
                    let sil_cy = sil_y + (target_sil_h - draw_h as i32) / 2;
                    draw_icon_at(&mut self.canvas, &resized_base, sil_x, sil_cy);

                    let regions = crate::panel_math::silhouette_regions(
                        *hp_current,
                        *hp_healable,
                        *hp_healable_per_charge,
                        *hp_max,
                    );

                    let hp_color = hp_bar_color_lerp(*hp_fraction);
                    let hp_sil = tint_silhouette(sil_img, hp_color);
                    let resized_hp =
                        image::imageops::resize(&hp_sil, draw_w, draw_h, image::imageops::FilterType::Triangle);
                    let colored_px = (draw_w as f32 * regions.colored) as u32;
                    if colored_px > 0 {
                        let cropped = image::imageops::crop_imm(&resized_hp, 0, 0, colored_px, draw_h).to_image();
                        draw_icon_at(&mut self.canvas, &cropped, sil_x, sil_cy);
                    }

                    if let Some(color) = heal_availability.healable_rgb() {
                        let mut draw_region = |fraction_start: f32, fraction_w: f32, tint: [u8; 3]| {
                            let region_x = (draw_w as f32 * fraction_start) as u32;
                            let region_w = (draw_w as f32 * fraction_w) as u32;
                            if region_w > 0 {
                                let region_sil = tint_silhouette(sil_img, tint);
                                let resized_region = image::imageops::resize(
                                    &region_sil,
                                    draw_w,
                                    draw_h,
                                    image::imageops::FilterType::Triangle,
                                );
                                let cropped = image::imageops::crop_imm(&resized_region, region_x, 0, region_w, draw_h)
                                    .to_image();
                                draw_icon_at(&mut self.canvas, &cropped, sil_x + region_x as i32, sil_cy);
                            }
                        };
                        draw_region(regions.colored, regions.healable_bright, color);
                        draw_region(
                            regions.colored + regions.healable_bright,
                            regions.healable_dim,
                            crate::panel_math::darken(color, 0.5),
                        );
                    }
                }

                // HP text: "12,345 / 42,750"
                let hp_text = format!("{} / {}", format_number(*hp_current as i64), format_number(*hp_max as i64));
                let hp_scale = self.fonts.scale_cond_bold(16.0 * scale_factor);
                let (tw, _) = text_size(hp_scale, &self.fonts.cond_bold, &hp_text);
                let hp_text_x = inner_x + (inner_w - tw as i32) / 2;
                draw_text_shadow(
                    &mut self.canvas,
                    [220, 220, 220],
                    hp_text_x,
                    hp_text_y,
                    hp_scale,
                    &self.fonts.cond_bold,
                    &hp_text,
                );
            }
            DrawCommand::StatsDamage {
                x,
                y,
                width,
                height,
                breakdowns,
                damage_spotting,
                spotting_breakdowns: _,
                damage_potential,
                potential_breakdowns: _,
                wpa,
                consumables,
            } => {
                let scale_factor = if self.large_elements { 1.8f32 } else { 1.0f32 };
                let padding = (8.0 * scale_factor).round() as i32;
                
                // Align to right column (starting at 57% width)
                let left_w = (*width * 57) / 100;
                let right_w = *width - left_w;
                let right_center_x = *x + left_w + right_w / 2;
                
                let label_scale = self.fonts.scale_cond_bold(11.0 * scale_factor);
                let value_scale = self.fonts.scale_cond_bold(15.0 * scale_factor);
                let label_row_h = (14.0 * scale_factor).round() as i32;
                let value_row_h = (19.0 * scale_factor).round() as i32;

                let show_player = true;
                let show_ship = true;
                let mut text_group_h = 0;
                if show_player {
                    text_group_h += (22.0 * scale_factor).round() as i32;
                }
                if show_ship {
                    text_group_h += (24.0 * scale_factor).round() as i32;
                }

                let top_section_h = *height;
                let emblem_size = (72.0 * scale_factor).round() as i32;
                let gap1 = (12.0 * scale_factor).round() as i32;
                let target_sil_h = (90.0 * scale_factor).round() as i32;
                let gap2 = (10.0 * scale_factor).round() as i32;
                let hp_text_h = (20.0 * scale_factor).round() as i32;
                let gap3 = (14.0 * scale_factor).round() as i32;
                let cons_size = (32.0 * scale_factor).round() as i32;

                let left_total_h = text_group_h + gap1 + target_sil_h + gap2 + hp_text_h + gap3 + cons_size;
                
                let start_y = *y + (top_section_h - left_total_h).max(0) / 4;
                let emblem_y = start_y + (4.0 * scale_factor).round() as i32;

                let left_second_y = start_y + text_group_h + gap1;
                let sil_y = left_second_y;
                let hp_text_y = sil_y + target_sil_h + gap2;
                let cons_y = hp_text_y + hp_text_h + gap3;

                // Metrics start below emblem with ample gap and fill down nicely
                let show_uwpa = self.inkpads_layout;
                let num_metrics = if show_uwpa { 4 } else { 3 };
                let metrics_start_y = emblem_y + emblem_size + (16.0 * scale_factor).round() as i32;
                let metrics_end_y = cons_y + cons_size;
                let region_h = (metrics_end_y - metrics_start_y).max(100);
                let metrics_total_h_unspaced = num_metrics * label_row_h + num_metrics * value_row_h;
                let metric_gap = ((region_h - metrics_total_h_unspaced) / (num_metrics - 1)).max(3);
                
                let mut cur_y = metrics_start_y + (region_h - (metrics_total_h_unspaced + (num_metrics - 1) * metric_gap)).max(0) / 2;

                // DAMAGE
                let total_damage: f64 = breakdowns.iter().map(|e| e.damage).sum();
                let label = "DAMAGE";
                let (lw, _) = text_size(label_scale, &self.fonts.cond_bold, label);
                let lx = right_center_x - lw as i32 / 2;
                draw_text_shadow(&mut self.canvas, [140, 140, 140], lx, cur_y, label_scale, &self.fonts.cond_bold, label);
                cur_y += label_row_h;
                
                let total_str = format_number(total_damage as i64);
                let (vw, _) = text_size(value_scale, &self.fonts.cond_bold, &total_str);
                let vx = right_center_x - vw as i32 / 2;
                draw_text_shadow(&mut self.canvas, [255, 220, 100], vx, cur_y, value_scale, &self.fonts.cond_bold, &total_str);
                cur_y += value_row_h + metric_gap;

                // SPOTTING
                let label = "SPOTTING";
                let (lw, _) = text_size(label_scale, &self.fonts.cond_bold, label);
                let lx = right_center_x - lw as i32 / 2;
                draw_text_shadow(&mut self.canvas, [140, 140, 140], lx, cur_y, label_scale, &self.fonts.cond_bold, label);
                cur_y += label_row_h;
                
                let spot_str = format_number(*damage_spotting as i64);
                let (vw, _) = text_size(value_scale, &self.fonts.cond_bold, &spot_str);
                let vx = right_center_x - vw as i32 / 2;
                draw_text_shadow(&mut self.canvas, [120, 200, 255], vx, cur_y, value_scale, &self.fonts.cond_bold, &spot_str);
                cur_y += value_row_h + metric_gap;

                // POTENTIAL
                let label = "POTENTIAL";
                let (lw, _) = text_size(label_scale, &self.fonts.cond_bold, label);
                let lx = right_center_x - lw as i32 / 2;
                draw_text_shadow(&mut self.canvas, [140, 140, 140], lx, cur_y, label_scale, &self.fonts.cond_bold, label);
                cur_y += label_row_h;
                
                let pot_str = format_number(*damage_potential as i64);
                let (vw, _) = text_size(value_scale, &self.fonts.cond_bold, &pot_str);
                let vx = right_center_x - vw as i32 / 2;
                draw_text_shadow(&mut self.canvas, [180, 180, 180], vx, cur_y, value_scale, &self.fonts.cond_bold, &pot_str);
                cur_y += value_row_h + metric_gap;

                if show_uwpa {
                    // uWPA
                    let label = "uWPA";
                    let (lw, _) = text_size(label_scale, &self.fonts.cond_bold, label);
                    let lx = right_center_x - lw as i32 / 2;
                    draw_text_shadow(&mut self.canvas, [140, 140, 140], lx, cur_y, label_scale, &self.fonts.cond_bold, label);
                    cur_y += label_row_h;
                    
                    let wpa_str = format!("{:.2}", wpa);
                    let (vw, _) = text_size(value_scale, &self.fonts.cond_bold, &wpa_str);
                    let vx = right_center_x - vw as i32 / 2;
                    draw_text_shadow(&mut self.canvas, wpa_color(*wpa, 1.0), vx, cur_y, value_scale, &self.fonts.cond_bold, &wpa_str);
                }

                // Draw active consumables centered horizontally in the left 57% column at cons_y
                let cons_gap = (6.0 * scale_factor).round() as i32;
                let max_cons_size = (36.0 * scale_factor).round() as i32;

                if !consumables.is_empty() {
                    let n = consumables.len() as i32;
                    let inner_x = *x + padding;
                    let inner_w = left_w - padding * 2;
                    let cons_size = ((inner_w - cons_gap * (n - 1)) / n).min(max_cons_size).max(16);
                    let total_cons_w = n * cons_size + (n - 1) * cons_gap;
                    let cons_start_x = inner_x + (inner_w - total_cons_w) / 2;

                    for (i, cons) in consumables.iter().enumerate() {
                        let cx = cons_start_x + i as i32 * (cons_size + cons_gap);

                        if let Some(icon) = self.consumable_icons.get(&cons.icon_key) {
                            let mut img = image::imageops::resize(
                                icon,
                                cons_size as u32,
                                cons_size as u32,
                                image::imageops::FilterType::Triangle,
                            );

                            if !cons.active {
                                for px in img.pixels_mut() {
                                    let r = px.0[0] as f32;
                                    let g = px.0[1] as f32;
                                    let b = px.0[2] as f32;
                                    let gray = (0.299 * r + 0.587 * g + 0.114 * b) as u8;
                                    px.0[0] = (gray as f32 * 0.65) as u8;
                                    px.0[1] = (gray as f32 * 0.65) as u8;
                                    px.0[2] = (gray as f32 * 0.65) as u8;
                                    px.0[3] = (px.0[3] as f32 * 0.85) as u8;
                                }
                            }

                            draw_icon_at(&mut self.canvas, &img, cx, cons_y);

                            if let crate::draw_command::ChargeCount::Finite(remaining) = cons.charges_remaining {
                                let count_str = format!("{}", remaining);
                                let count_scale = self.fonts.scale(10.0 * scale_factor);
                                let (cw, ch) = text_size(count_scale, &self.fonts.primary, &count_str);

                                let badge_w = cw as i32 + 4;
                                let badge_h = ch as i32 + 2;
                                let bx = cx + cons_size - badge_w;
                                let by = cons_y + cons_size - badge_h;
                                draw_filled_rect(
                                    &mut self.canvas,
                                    bx as f32,
                                    by as f32,
                                    badge_w as f32,
                                    badge_h as f32,
                                    [0, 0, 0],
                                    0.75,
                                );

                                draw_text_shadow(
                                    &mut self.canvas,
                                    [255, 255, 255],
                                    bx + 2,
                                    by + 1,
                                    count_scale,
                                    &self.fonts.primary,
                                    &count_str,
                                );
                            }
                        }
                    }
                }
            }
            DrawCommand::StatsRibbons { x, y, width, ribbons, large_format } => {
                let padding = 8;
                let inner_x = *x + padding;
                let inner_w = *width - padding * 2;

                // Base dimensions according to layout format:
                // Options A & B (large_format = true) use 2x base dimensions to fill the 228px stats panel slot.
                // Option C (--inkpads-layout / large_format = false) retains compact scale for placement under table.
                let (base_cell_w, base_row_h, base_icon_h, base_font_pt, max_h) = if *large_format {
                    (180.0, 90.0, 80.0, 26.0, 220.0)
                } else {
                    (90.0, 45.0, 40.0, 13.0, 55.0)
                };

                let mut best_s: f32 = 0.5;
                if !ribbons.is_empty() {
                    let mut s: f32 = 1.0;
                    while s >= 0.3 {
                        let cur_cell_w = (base_cell_w * s).round() as i32;
                        let cur_row_h = (base_row_h * s).round() as i32;
                        
                        let cols = (inner_w / cur_cell_w).max(1);
                        let rows = (ribbons.len() as i32 + cols - 1) / cols;
                        
                        if rows * cur_row_h <= max_h as i32 {
                            best_s = s;
                            break;
                        }
                        s -= 0.05;
                    }
                }
                
                let icon_h = (base_icon_h * best_s).round() as i32;
                let row_h = (base_row_h * best_s).round() as i32;
                let cell_w = (base_cell_w * best_s).round() as i32;
                let gap = (2.0 * best_s).round() as i32;
                let scale = self.fonts.scale(base_font_pt * best_s);
                let pill_pad_x = (4.0 * if *large_format { 2.0 * best_s } else { best_s }).max(2.0);
                let pill_pad_y = (1.0 * if *large_format { 2.0 * best_s } else { best_s }).max(1.0);
                let pill_radius = (2.0 * if *large_format { 2.0 * best_s } else { best_s }).max(1.0);

                // Group ribbons into rows to calculate centered layout
                let mut rows: Vec<Vec<&crate::draw_command::RibbonCount>> = Vec::new();
                let mut current_row: Vec<&crate::draw_command::RibbonCount> = Vec::new();
                let mut cur_row_w = 0;
                for rc in ribbons.iter() {
                    if cur_row_w + cell_w > inner_w && !current_row.is_empty() {
                        rows.push(current_row);
                        current_row = Vec::new();
                        cur_row_w = 0;
                    }
                    current_row.push(rc);
                    cur_row_w += cell_w;
                }
                if !current_row.is_empty() {
                    rows.push(current_row);
                }

                let mut cur_y = *y + 4;
                for row in rows.iter() {
                    let row_width = row.len() as i32 * cell_w;
                    let mut cur_x = inner_x + (inner_w - row_width) / 2;
                    for rc in row.iter() {
                        let sub_key = format!("sub{}", rc.icon_key);
                        let icon = if rc.is_subribbon {
                            self.subribbon_icons.get(&sub_key).or_else(|| self.ribbon_icons.get(&rc.icon_key))
                        } else {
                            self.ribbon_icons.get(&rc.icon_key).or_else(|| self.subribbon_icons.get(&sub_key))
                        };
                        
                        if let Some(img) = icon {
                            let mut draw_h = icon_h;
                            let mut w = if img.height() > 0 {
                                ((icon_h as f32 * img.width() as f32 / img.height() as f32).round() as i32).max(1)
                            } else {
                                icon_h
                            };
                            
                            let max_w = cell_w - (6.0 * if *large_format { 2.0 * best_s } else { 1.0 }).round() as i32;
                            if w > max_w {
                                let ratio = max_w as f32 / w as f32;
                                w = max_w;
                                draw_h = (draw_h as f32 * ratio).round() as i32;
                            }
                            
                            let resized = image::imageops::resize(
                                img,
                                w as u32,
                                draw_h as u32,
                                image::imageops::FilterType::Lanczos3,
                            );
                            
                            // Center icon horizontally and vertically within the cell
                            let rx = cur_x + (cell_w - w) / 2;
                            let ry = cur_y + (row_h - draw_h) / 2;
                            draw_icon_at(&mut self.canvas, &resized, rx, ry);
                            
                            let count_str = format!("x{}", rc.count);
                            let (tw, th) = text_size(scale, &self.fonts.primary, &count_str);
                            
                            let pill_w = tw as f32 + pill_pad_x * 2.0;
                            let pill_h = th as f32 + pill_pad_y * 2.0;
                            
                            // Badge at bottom-right of the centered icon
                            let pill_x = (rx + w) as f32 - pill_w;
                            let pill_y = (ry + draw_h) as f32 - pill_h;
                            
                            draw_rounded_rect(
                                &mut self.canvas,
                                pill_x,
                                pill_y,
                                pill_w,
                                pill_h,
                                pill_radius,
                                [0, 0, 0],
                                0.65,
                            );
                            
                            draw_text_shadow(
                                &mut self.canvas,
                                [255, 255, 255],
                                (pill_x + pill_pad_x) as i32,
                                (pill_y + pill_pad_y) as i32,
                                scale,
                                &self.fonts.primary,
                                &count_str,
                            );
                        } else {
                            let label: String = rc.display_name.chars().take(8).collect();
                            let (lw, _) = text_size(scale, &self.fonts.primary, &label);
                            
                            // Center text cell
                            let tx = cur_x + (cell_w - lw as i32) / 2;
                            let ty = cur_y + (row_h - (base_font_pt * best_s) as i32) / 2;
                            draw_text_shadow(
                                &mut self.canvas,
                                [180, 180, 180],
                                tx,
                                ty,
                                scale,
                                &self.fonts.primary,
                                &label,
                            );
                            
                            let count_str = format!("x{}", rc.count);
                            let (_cw, _) = text_size(scale, &self.fonts.primary, &count_str);
                            draw_text_shadow(
                                &mut self.canvas,
                                [255, 255, 255],
                                tx + lw as i32 + gap,
                                ty,
                                scale,
                                &self.fonts.primary,
                                &count_str,
                            );
                        }
                        cur_x += cell_w;
                    }
                    cur_y += row_h;
                }
            }
            DrawCommand::StatsActivityFeed { x, y, width, height, entries } => {
                let sf = if self.large_elements { 1.5f32 } else { 1.0f32 };
                let padding = (8.0 * sf).round() as i32;
                let inner_w = *width - padding * 2;

                let msg_scale = self.fonts.scale(13.0 * sf);
                let kill_font_size = 16.0 * sf;
                let kill_name_scale = self.fonts.scale(kill_font_size);
                let kill_row_h = (28.0 * sf).round() as i32;
                let kill_icon_size = (20.0 * sf).round() as i32;
                let chat_header_h = (18.0 * sf).round() as i32;
                let chat_line_h = (17.0 * sf).round() as i32;
                let icon_size = (16.0 * sf).round() as i32;
                let gap = (2.0 * sf).round() as i32;
                let font = &self.fonts.primary;

                // Fixed-size box background
                draw_filled_rect(
                    &mut self.canvas,
                    *x as f32,
                    *y as f32,
                    *width as f32,
                    *height as f32,
                    [24, 28, 36],
                    0.8,
                );
                // Top border
                draw_line(
                    &mut self.canvas,
                    *x as f32 + 4.0,
                    *y as f32,
                    (*x + *width - 4) as f32,
                    *y as f32,
                    [55, 60, 72],
                    0.6,
                    1.0,
                );

                let right_x = *x + padding;
                let right_w = inner_w;

                // Draw horizontal divider separating Kill Feed and Chat Feed
                let kill_h = 276;
                let divider_y = *y + kill_h;
                draw_line(
                    &mut self.canvas,
                    *x as f32 + 4.0,
                    divider_y as f32,
                    (*x + *width - 4) as f32,
                    divider_y as f32,
                    [55, 60, 72],
                    0.4,
                    1.0,
                );

                // Filter entries
                let kill_entries: Vec<&ActivityFeedEntry> =
                    entries.iter().filter(|e| matches!(e.kind, ActivityFeedKind::Kill(_))).collect();
                let chat_entries: Vec<&ActivityFeedEntry> =
                    entries.iter().filter(|e| matches!(e.kind, ActivityFeedKind::Chat(_))).collect();

                // 1. Render Kill Feed (Top region, half of total height)
                let mut kill_consumed = 0i32;
                let mut kill_start_idx = kill_entries.len();
                for i in (0..kill_entries.len()).rev() {
                    let needed = kill_consumed + kill_row_h;
                    if needed > kill_h - 8 {
                        break;
                    }
                    kill_consumed = needed;
                    kill_start_idx = i;
                }

                let mut ky = *y + 4;
                for entry in kill_entries.iter().skip(kill_start_idx) {
                    if ky + kill_row_h > *y + kill_h {
                        break;
                    }
                    if let ActivityFeedKind::Kill(kill) = &entry.kind {
                        let (_, text_h) = text_size(kill_name_scale, font, "Ag");
                        let icon_y = ky + (text_h as i32 - kill_icon_size) / 2;

                        // Measure icons
                        let has_killer_icon = kill.killer_species.is_some()
                            && self.ship_icons.contains_key(kill.killer_species.as_ref().unwrap());
                        let has_victim_icon = kill.victim_species.is_some()
                            && self.ship_icons.contains_key(kill.victim_species.as_ref().unwrap());
                        let cause_key = death_cause_icon_key(&kill.cause);
                        let has_cause_icon = self.death_cause_icons.contains_key(cause_key);

                        let mut icons_w = 0i32;
                        if has_killer_icon {
                            icons_w += kill_icon_size + gap;
                        }
                        if has_cause_icon {
                            icons_w += kill_icon_size + gap;
                        }
                        if has_victim_icon {
                            icons_w += kill_icon_size;
                        }

                        // Remaining space divided equally for names
                        let remaining_w = inner_w.saturating_sub(icons_w);
                        let name_max_w = (remaining_w / 2).max(40);

                        // Killer name
                        let (kf, ks) = self.fonts.font_and_scale(&kill.killer_name, kill_font_size);
                        let killer_display = truncate_to_fit(&kill.killer_name, name_max_w as u32, ks, kf);
                        let (kw, _) = text_size(ks, kf, &killer_display);

                        // Victim name
                        let (vf, vs) = self.fonts.font_and_scale(&kill.victim_name, kill_font_size);
                        let victim_display = truncate_to_fit(&kill.victim_name, name_max_w as u32, vs, vf);
                        let (vw, _) = text_size(vs, vf, &victim_display);

                        // Calculate total width of this line (prefix only)
                        let prefix = " | ";
                        let (pw, _) = text_size(kill_name_scale, font, prefix);

                        // Align left in the feed (like the chat feed)
                        let mut cx = right_x;

                        // Kill prefix
                        draw_text_shadow(&mut self.canvas, [140, 140, 140], cx, ky, kill_name_scale, font, prefix);
                        cx += pw as i32;

                        // Killer name
                        draw_text_shadow(&mut self.canvas, kill.killer_color, cx, ky, ks, kf, &killer_display);
                        cx += kw as i32 + gap;

                        // Killer ship icon
                        if has_killer_icon {
                            let icon = self.ship_icons.get(kill.killer_species.as_ref().unwrap()).unwrap();
                            draw_kill_feed_icon(
                                &mut self.canvas,
                                icon,
                                cx,
                                icon_y,
                                kill_icon_size,
                                kill.killer_color,
                                !kill.killer_is_friendly,
                            );
                            cx += kill_icon_size + gap;
                        }

                        // Death cause icon
                        if has_cause_icon {
                            let cause_icon = self.death_cause_icons.get(cause_key).unwrap();
                            let cause_center_y = icon_y + kill_icon_size / 2;
                            draw_icon(&mut self.canvas, cause_icon, (cx + kill_icon_size / 2) as f32, cause_center_y as f32);
                            cx += kill_icon_size + gap;
                        }

                        // Victim name
                        draw_text_shadow(&mut self.canvas, kill.victim_color, cx, ky, vs, vf, &victim_display);
                        cx += vw as i32 + gap;

                        // Victim ship icon
                        if has_victim_icon {
                            let icon = self.ship_icons.get(kill.victim_species.as_ref().unwrap()).unwrap();
                            draw_kill_feed_icon(
                                &mut self.canvas,
                                icon,
                                cx,
                                icon_y,
                                kill_icon_size,
                                kill.victim_color,
                                !kill.victim_is_friendly,
                            );
                        }

                        ky += kill_row_h;
                    }
                }

                // 2. Render Chat Feed (Bottom region, remaining height)
                let chat_h = *height - kill_h;
                let mut chat_heights: Vec<i32> = Vec::new();
                for entry in chat_entries.iter() {
                    if let ActivityFeedKind::Chat(chat) = &entry.kind {
                        let msg_font = match chat.font_hint {
                            FontHint::Primary => &self.fonts.primary,
                            FontHint::Fallback(i) => self.fonts.fallbacks.get(i).unwrap_or(&self.fonts.primary),
                        };
                        let msg_lines = word_wrap(&chat.message, right_w as u32, msg_scale, msg_font);
                        let h = chat_header_h + msg_lines.len().max(1) as i32 * chat_line_h + 2;
                        chat_heights.push(h);
                    }
                }

                let mut chat_consumed = 0i32;
                let mut chat_start_idx = chat_entries.len();
                for i in (0..chat_entries.len()).rev() {
                    let needed = chat_consumed + chat_heights[i] + 2;
                    if needed > chat_h - 8 {
                        break;
                    }
                    chat_consumed = needed;
                    chat_start_idx = i;
                }

                let mut cy = divider_y + 4;
                for entry in chat_entries.iter().skip(chat_start_idx) {
                    if cy >= *y + *height {
                        break;
                    }
                    if let ActivityFeedKind::Chat(chat) = &entry.kind {
                        let mut cx = right_x;

                        // Clan tag + Player name
                        let (chat_name_font, chat_name_scale) = self.fonts.font_and_scale(&chat.player_name, 14.0);

                        let ship_icon_w = if chat.ship_species.is_some() { icon_size + gap } else { 0 };
                        let mut ship_name_w = 0i32;
                        if let Some(ref ship_name) = chat.ship_name {
                            let (ship_font, ship_scale) = self.fonts.font_and_scale(ship_name, 14.0);
                            let (sw, _) = text_size(ship_scale, ship_font, ship_name);
                            ship_name_w = sw as i32;
                        }
                        let clan_w = if !chat.clan_tag.is_empty() {
                            let clan_text = format!("[{}] ", chat.clan_tag);
                            let (cw, _) = text_size(chat_name_scale, chat_name_font, &clan_text);
                            cw as i32
                        } else {
                            0
                        };

                        let available_player_w = right_w - clan_w - ship_icon_w - ship_name_w - gap * 3;
                        let player_name_w = if available_player_w > 30 {
                            available_player_w
                        } else {
                            ship_name_w = 0; // omit ship name if space is tight
                            (right_w - clan_w - ship_icon_w - gap * 2).max(20)
                        };

                        // Clan Tag
                        if !chat.clan_tag.is_empty() {
                            let clan_color = chat.clan_color.unwrap_or(chat.team_color);
                            let clan_text = format!("[{}] ", chat.clan_tag);
                            draw_text_shadow(
                                &mut self.canvas,
                                clan_color,
                                cx,
                                cy,
                                chat_name_scale,
                                chat_name_font,
                                &clan_text,
                            );
                            cx += clan_w;
                        }

                        // Player Name
                        let player_display =
                            truncate_to_fit(&chat.player_name, player_name_w as u32, chat_name_scale, chat_name_font);
                        draw_text_shadow(
                            &mut self.canvas,
                            chat.team_color,
                            cx,
                            cy,
                            chat_name_scale,
                            chat_name_font,
                            &player_display,
                        );
                        let (nw, text_h) = text_size(chat_name_scale, chat_name_font, &player_display);
                        cx += nw as i32 + gap;

                        // Ship Icon
                        let icon_y = cy + (text_h as i32 - icon_size) / 2;
                        if let Some(ref species) = chat.ship_species
                            && let Some(icon) = self.ship_icons.get(species.as_str())
                        {
                            draw_kill_feed_icon(&mut self.canvas, icon, cx, icon_y, icon_size, chat.team_color, false);
                            cx += icon_size + gap;
                        }

                        // Ship Name
                        if ship_name_w > 0 {
                            if let Some(ref ship_name) = chat.ship_name {
                                let (ship_font, ship_scale) = self.fonts.font_and_scale(ship_name, 14.0);
                                draw_text_shadow(
                                    &mut self.canvas,
                                    chat.team_color,
                                    cx,
                                    cy,
                                    ship_scale,
                                    ship_font,
                                    ship_name,
                                );
                            }
                        }

                        cy += chat_header_h;

                        // Message lines (word-wrapped)
                        let msg_font = match chat.font_hint {
                            FontHint::Primary => &self.fonts.primary,
                            FontHint::Fallback(idx) => self.fonts.fallbacks.get(idx).unwrap_or(&self.fonts.primary),
                        };
                        let msg_lines = word_wrap(&chat.message, right_w as u32, msg_scale, msg_font);
                        for line in &msg_lines {
                            draw_text_shadow(
                                &mut self.canvas,
                                chat.message_color,
                                right_x,
                                cy,
                                msg_scale,
                                msg_font,
                                line,
                            );
                            cy += chat_line_h;
                        }
                        if msg_lines.is_empty() {
                            cy += chat_line_h;
                        }
                        cy += 2; // small gap after chat
                    }
                }
            }

            DrawCommand::InkpadsTeammateTable { x, y, width, height, rows } => {
                draw_inkpads_teammate_table(
                    &mut self.canvas,
                    *x,
                    *y,
                    *width,
                    *height,
                    rows,
                    &self.fonts,
                    &self.ship_icons,
                );
            }
            DrawCommand::InkpadsInLineFeeds { x, y, width, height, entries } => {
                draw_inkpads_inline_feeds(
                    &mut self.canvas,
                    *x,
                    *y,
                    *width,
                    *height,
                    entries,
                    &self.fonts,
                    &self.ship_icons,
                    &self.death_cause_icons,
                );
            }
        }
    }

    fn end_frame(&mut self) {
        // No-op — frame is ready to read via frame()
    }
}

fn draw_inkpads_teammate_table(
    pm: &mut Pixmap,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    rows: &[crate::draw_command::TeammateTableRow],
    fonts: &GameFonts,
    _ship_icons: &HashMap<String, ShipIcon>,
) {
    if rows.is_empty() {
        return;
    }

    // Panel background
    draw_filled_rect(pm, x as f32, y as f32, width as f32, height as f32, [24, 28, 36], 0.85);
    draw_line(pm, x as f32, y as f32, (x + width) as f32, y as f32, [55, 60, 72], 0.5, 1.0);

    let padding = 12i32;
    let header_h = 32i32;
    let row_count = rows.len() as i32;
    let available_h = height - header_h - padding * 2;
    let row_h = if row_count > 0 { (available_h / row_count).clamp(24, 48) } else { 32 };

    let font = &fonts.primary;
    let font_size = if row_count > 9 { 14.0f32 } else { 16.0f32 };
    let font_scale = fonts.scale(font_size);

    // Draw Column Headers: PLAYER / SHIP | WPA | DAMAGE | RECEIVED | DMG TRADE | HP | KILLS
    let player_col_w = (width as f32 * 0.32) as i32;
    let stat_col_w = (width - player_col_w - padding * 2) / 6;

    let headers = ["PLAYER / SHIP", "WPA", "DAMAGE", "RECEIVED", "DMG TRADE", "HP", "KILLS"];
    let mut hx = x + padding;

    // Player/Ship Header
    draw_text_shadow(pm, [160, 175, 200], hx, y + 6, font_scale, font, headers[0]);
    hx += player_col_w;

    for &h in &headers[1..] {
        draw_text_shadow(pm, [160, 175, 200], hx, y + 6, font_scale, font, h);
        hx += stat_col_w;
    }

    // Header divider line
    let div_y = y + header_h;
    draw_line(pm, x as f32 + 4.0, div_y as f32, (x + width - 4) as f32, div_y as f32, [55, 60, 72], 0.4, 1.0);

    // Draw Rows
    let team_avg = if !rows.is_empty() {
        rows.iter().map(|r| r.wpa as f64).sum::<f64>() / rows.len() as f64
    } else {
        1.0
    };

    let mut ry = div_y + 4;
    for row in rows {
        if ry + row_h > y + height {
            break;
        }

        let text_color = if row.is_dead { [120, 125, 135] } else { [230, 235, 245] };
        let mut rx = x + padding;

        // Player Name & Clan Tag
        let player_label = if let Some(ref tag) = row.clan_tag {
            if !tag.is_empty() {
                format!("[{}] {}", tag, row.player_name)
            } else {
                row.player_name.clone()
            }
        } else {
            row.player_name.clone()
        };

        let label_with_ship = if !row.ship_name.is_empty() {
            format!("{} ({})", player_label, row.ship_name)
        } else {
            player_label
        };

        let name_display = truncate_to_fit(&label_with_ship, (player_col_w - 6) as u32, font_scale, font);
        draw_text_shadow(pm, text_color, rx, ry + (row_h - font_size as i32) / 2, font_scale, font, &name_display);
        rx += player_col_w;

        // Stats
        let dmg_trade_str = if row.received == 0 {
            "-".to_string()
        } else {
            format!("{:.1}x", row.damage as f64 / row.received as f64)
        };

        let hp_pct = if row.is_dead || row.hp_max == 0 {
            0
        } else {
            ((row.hp_cur as f64 / row.hp_max as f64 * 100.0).round() as u32).min(100)
        };
        let hp_str = format!("{}%", hp_pct);

        let stats_formatted = [
            format!("{:.2}", row.wpa),
            format_number_commas(row.damage),
            format_number_commas(row.received),
            dmg_trade_str,
            hp_str,
            row.kills.to_string(),
        ];

        for (idx, val) in stats_formatted.iter().enumerate() {
            let col_color = if idx == 0 {
                wpa_color(row.wpa as f64, team_avg)
            } else {
                text_color
            };

            draw_text_shadow(pm, col_color, rx, ry + (row_h - font_size as i32) / 2, font_scale, font, val);
            rx += stat_col_w;
        }

        ry += row_h;
    }
}

fn draw_inkpads_inline_feeds(
    pm: &mut Pixmap,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    entries: &[ActivityFeedEntry],
    fonts: &GameFonts,
    _ship_icons: &HashMap<String, ShipIcon>,
    death_cause_icons: &HashMap<String, RgbaImage>,
) {
    // 40% / 60% horizontal split
    let left_w = (width as f32 * 0.40) as i32;
    let right_w = width - left_w;
    let padding = 10i32;
    let font = &fonts.primary;
    let font_size = 18.0f32;
    let font_scale = fonts.scale(font_size);

    // Box background
    draw_filled_rect(pm, x as f32, y as f32, width as f32, height as f32, [24, 28, 36], 0.85);
    draw_line(pm, x as f32 + 4.0, y as f32, (x + width - 4) as f32, y as f32, [55, 60, 72], 0.5, 1.0);

    // Vertical column divider (40% / 60% line)
    let div_x = x + left_w;
    draw_line(pm, div_x as f32, y as f32 + 4.0, div_x as f32, (y + height - 4) as f32, [55, 60, 72], 0.4, 1.0);

    // Split entries
    let kill_entries: Vec<&ActivityFeedEntry> = entries.iter().filter(|e| matches!(e.kind, ActivityFeedKind::Kill(_))).collect();
    let chat_entries: Vec<&ActivityFeedEntry> = entries.iter().filter(|e| matches!(e.kind, ActivityFeedKind::Chat(_))).collect();

    // 1. LEFT 40% COLUMN: Kill Feed (Enlarged)
    let kill_x = x + padding;
    let kill_row_h = 30i32;
    let kill_icon_size = 22i32;

    let mut ky = y + padding;
    for entry in kill_entries.iter().rev() {
        if ky + kill_row_h > y + height {
            break;
        }
        if let ActivityFeedKind::Kill(kill) = &entry.kind {
            let killer_short = truncate_to_fit(&kill.killer_name, (left_w / 2 - 16) as u32, font_scale, font);
            let victim_short = truncate_to_fit(&kill.victim_name, (left_w / 2 - 16) as u32, font_scale, font);

            let mut cx = kill_x;
            draw_text_shadow(pm, kill.killer_color, cx, ky, font_scale, font, &killer_short);
            let (kw, _) = text_size(font_scale, font, &killer_short);
            cx += kw as i32 + 6;

            let cause_key = death_cause_icon_key(&kill.cause);
            if let Some(cause_icon) = death_cause_icons.get(cause_key) {
                draw_icon(pm, cause_icon, (cx + kill_icon_size / 2) as f32, (ky + kill_icon_size / 2) as f32);
                cx += kill_icon_size + 6;
            }

            draw_text_shadow(pm, kill.victim_color, cx, ky, font_scale, font, &victim_short);
            ky += kill_row_h;
        }
    }

    // 2. RIGHT 60% COLUMN: Chat Feed (Enlarged with 14px inter-block gap)
    let chat_x = div_x + padding;
    let chat_w = right_w - padding * 2;
    let inter_block_gap = 14i32;
    let line_h = 24i32;
    let msg_scale = fonts.scale(17.0f32);

    let mut cy = y + padding;
    for entry in chat_entries.iter() {
        if cy + line_h * 2 > y + height {
            break;
        }
        if let ActivityFeedKind::Chat(chat) = &entry.kind {
            // Header line: [Clan] Player Name
            let header_text = if !chat.clan_tag.is_empty() {
                format!("[{}] {}", chat.clan_tag, chat.player_name)
            } else {
                chat.player_name.clone()
            };

            let header_disp = truncate_to_fit(&header_text, chat_w as u32, font_scale, font);
            draw_text_shadow(pm, chat.team_color, chat_x, cy, font_scale, font, &header_disp);
            cy += line_h;

            // Message text line
            let msg_disp = truncate_to_fit(&chat.message, chat_w as u32, msg_scale, font);
            draw_text_shadow(pm, chat.message_color, chat_x, cy, msg_scale, font, &msg_disp);
            cy += line_h;

            // Explicit 14px Inter-block Spacing Gap
            cy += inter_block_gap;
        }
    }
}

fn format_number_commas(n: u32) -> String {
    let s = n.to_string();
    let mut result = String::new();
    let len = s.len();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result
}
