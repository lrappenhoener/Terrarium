//! Drawing with macroquad. Everything graphics-specific lives here and in
//! `main.rs`, so the library could be replaced without touching the logic.
//! All graphics are simple shapes drawn in code (see ASSETS.md).

use fortress_sim::view::JobOverlay;
use fortress_viewer::{
    camera::Camera,
    frame::{Frame, Mirror},
    tools::Overlay,
};
use macroquad::prelude::*;
use sim_core::space::{Pos3, Tile, TileKind};

pub struct Materials {
    pub names: Vec<String>,
    pub ore: Vec<bool>,
    /// Category ID per item kind.
    pub item_categories: Vec<String>,
}

impl Materials {
    fn color(&self, material: u16) -> Color {
        let index = usize::from(material);
        let name = self.names.get(index).map_or("", String::as_str);
        if name.starts_with("soil") {
            return Color::from_rgba(112, 82, 52, 255);
        }
        if self.ore.get(index).copied().unwrap_or(false) {
            return if name.starts_with("gold") {
                Color::from_rgba(214, 180, 40, 255)
            } else if name.starts_with("copper") {
                Color::from_rgba(196, 110, 52, 255)
            } else {
                Color::from_rgba(150, 84, 70, 255)
            };
        }
        let shade = 120 + (material as u8).wrapping_mul(29) % 90;
        let warm = if name.starts_with("sand") { 30 } else { 0 };
        Color::from_rgba(
            shade.saturating_add(warm),
            shade.saturating_add(warm / 2),
            shade,
            255,
        )
    }
}

pub struct Overlays {
    pub jobs: bool,
    pub path: bool,
    pub regions: bool,
    pub heatmap: bool,
    pub temperature: bool,
    pub active: bool,
    pub support: bool,
}

impl Overlays {
    pub fn toggle(&mut self, overlay: Overlay) {
        let flag = match overlay {
            Overlay::Jobs => &mut self.jobs,
            Overlay::Path => &mut self.path,
            Overlay::Regions => &mut self.regions,
            Overlay::Heatmap => &mut self.heatmap,
            Overlay::Temperature => &mut self.temperature,
            Overlay::ActiveChunks => &mut self.active,
            Overlay::Support => &mut self.support,
        };
        *flag = !*flag;
    }
}

fn darken(color: Color, factor: f32) -> Color {
    Color::new(
        color.r * factor,
        color.g * factor,
        color.b * factor,
        color.a,
    )
}

const FLOOR: Color = Color::new(0.30, 0.26, 0.21, 1.0);
const AIR: Color = Color::new(0.05, 0.06, 0.09, 1.0);
const STOCKPILE: Color = Color::new(0.25, 0.55, 0.30, 0.35);

fn draw_tile(tile: Tile, built: bool, materials: &Materials, x: f32, y: f32, s: f32, dim: f32) {
    let base = materials.color(tile.material);
    match tile.kind {
        TileKind::Solid => {
            draw_rectangle(x, y, s, s, darken(base, dim));
            if built {
                // Constructed walls: brick pattern.
                let line = darken(base, dim * 0.55);
                draw_line(x, y + s / 2.0, x + s, y + s / 2.0, 1.0, line);
                draw_line(x + s / 2.0, y, x + s / 2.0, y + s / 2.0, 1.0, line);
                draw_rectangle_lines(x, y, s, s, 1.0, line);
            } else if materials
                .ore
                .get(usize::from(tile.material))
                .copied()
                .unwrap_or(false)
            {
                draw_rectangle(
                    x + s * 0.3,
                    y + s * 0.3,
                    s * 0.4,
                    s * 0.4,
                    darken(WHITE, dim * 0.8),
                );
            }
        }
        TileKind::Open => {
            draw_rectangle(
                x,
                y,
                s,
                s,
                darken(if tile.floor() { FLOOR } else { AIR }, dim),
            );
        }
        TileKind::Door => {
            draw_rectangle(x, y, s, s, darken(FLOOR, dim));
            draw_rectangle(
                x + s * 0.15,
                y + s * 0.15,
                s * 0.7,
                s * 0.7,
                darken(Color::from_rgba(120, 72, 30, 255), dim),
            );
        }
        TileKind::StairUp | TileKind::StairDown | TileKind::StairUpDown | TileKind::Ramp => {
            draw_rectangle(x, y, s, s, darken(FLOOR, dim));
            let color = darken(Color::from_rgba(230, 230, 210, 255), dim);
            let (cx, top, bottom, mid) = (x + s / 2.0, y + s * 0.15, y + s * 0.85, y + s / 2.0);
            let (left, right) = (x + s * 0.2, x + s * 0.8);
            match tile.kind {
                TileKind::StairUp => draw_triangle(
                    vec2(cx, top),
                    vec2(left, bottom),
                    vec2(right, bottom),
                    color,
                ),
                TileKind::StairDown => {
                    draw_triangle(vec2(left, top), vec2(right, top), vec2(cx, bottom), color)
                }
                TileKind::StairUpDown => {
                    draw_triangle(vec2(cx, top), vec2(left, mid), vec2(right, mid), color);
                    draw_triangle(
                        vec2(left, mid),
                        vec2(right, mid),
                        vec2(cx, bottom),
                        darken(color, 0.7),
                    );
                }
                _ => draw_triangle(
                    vec2(left, bottom),
                    vec2(right, bottom),
                    vec2(right, top),
                    color,
                ),
            }
        }
    }
}

fn region_color(region: u32) -> Color {
    let h = region.wrapping_mul(2_654_435_761);
    Color::from_rgba((h >> 24) as u8, (h >> 16) as u8, (h >> 8) as u8, 90)
}

#[allow(clippy::too_many_arguments)]
pub fn draw_world(
    mirror: &Mirror,
    frame: &Frame,
    camera: &Camera,
    materials: &Materials,
    overlays: &Overlays,
    selected: Option<u64>,
    selection: Option<(Pos3, Pos3)>,
) {
    let screen = (screen_width(), screen_height());
    let (w, h, _) = mirror.dims;
    let z = camera.level;
    let s = camera.tile_px;
    let (x0, y0, x1, y1) = camera.visible(screen, w, h);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (sx, sy) = camera.tile_to_screen(x as f32, y as f32, screen);
            let p = Pos3::new(x, y, z);
            let Some(tile) = mirror.get(p) else { continue };
            if tile.kind == TileKind::Open && !tile.floor() {
                // Look down one level through open tiles without a floor.
                let below = p.offset(0, 0, -1);
                match mirror.get(below) {
                    Some(lower) if !(lower.kind == TileKind::Open && !lower.floor()) => {
                        draw_tile(lower, mirror.is_built(below), materials, sx, sy, s, 0.45);
                    }
                    _ => draw_rectangle(sx, sy, s, s, AIR),
                }
            } else {
                draw_tile(tile, mirror.is_built(p), materials, sx, sy, s, 1.0);
            }
        }
    }
    // Fluids: water blue, magma orange, opacity and digit by level.
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (level, magma) = mirror.fluid(Pos3::new(x, y, z));
            if level == 0 {
                continue;
            }
            let (sx, sy) = camera.tile_to_screen(x as f32, y as f32, screen);
            let alpha = 0.25 + 0.09 * f32::from(level);
            let color = if magma {
                Color::new(1.0, 0.45, 0.05, alpha)
            } else {
                Color::new(0.15, 0.4, 1.0, alpha)
            };
            draw_rectangle(sx, sy, s, s, color);
            if s >= 14.0 {
                draw_text(
                    level.to_string(),
                    sx + s * 0.3,
                    sy + s * 0.75,
                    s * 0.8,
                    WHITE,
                );
            }
        }
    }
    if overlays.temperature
        && let Some((level, temperatures)) = &frame.temperatures
        && *level == z
    {
        for y in y0..=y1 {
            for x in x0..=x1 {
                if let Some(&t) = temperatures.get((y * w + x) as usize) {
                    let heat = ((t - 10) as f32 / 150.0).clamp(0.0, 1.0);
                    if heat > 0.05 {
                        let (sx, sy) = camera.tile_to_screen(x as f32, y as f32, screen);
                        draw_rectangle(sx, sy, s, s, Color::new(1.0, 0.1, 0.0, 0.15 + 0.6 * heat));
                    }
                }
            }
        }
    }
    if overlays.active {
        for (chunk, bounds) in mirror.bounds.iter().enumerate() {
            let Some((low, high)) = *bounds else { continue };
            let Some(&(fluid, heat)) = frame.activity.get(chunk) else {
                continue;
            };
            if !(low.z..high.z).contains(&z) || !(fluid || heat) {
                continue;
            }
            let (sx, sy) = camera.tile_to_screen(low.x as f32, low.y as f32, screen);
            let (width, height) = ((high.x - low.x) as f32 * s, (high.y - low.y) as f32 * s);
            let color = if heat {
                Color::new(1.0, 0.3, 0.2, 0.9)
            } else {
                Color::new(0.3, 0.6, 1.0, 0.9)
            };
            draw_rectangle_lines(sx, sy, width, height, 3.0, color);
        }
    }
    if overlays.support {
        for p in &frame.support_pending {
            if p.z == z {
                let (sx, sy) = camera.tile_to_screen(p.x as f32, p.y as f32, screen);
                draw_rectangle_lines(sx, sy, s, s, 2.0, Color::new(1.0, 0.0, 1.0, 1.0));
            }
        }
    }
    for p in &frame.stockpiles {
        if p.z == z {
            let (sx, sy) = camera.tile_to_screen(p.x as f32, p.y as f32, screen);
            draw_rectangle(sx, sy, s, s, STOCKPILE);
        }
    }
    if overlays.regions
        && let Some((level, regions)) = &frame.regions
        && *level == z
    {
        for y in y0..=y1 {
            for x in x0..=x1 {
                if let Some(Some(region)) = regions.get((y * w + x) as usize) {
                    let (sx, sy) = camera.tile_to_screen(x as f32, y as f32, screen);
                    draw_rectangle(sx, sy, s, s, region_color(*region));
                }
            }
        }
    }
    if overlays.heatmap {
        let max = frame
            .chunk_changes
            .iter()
            .copied()
            .max()
            .unwrap_or(0)
            .max(1) as f32;
        for (chunk, bounds) in mirror.bounds.iter().enumerate() {
            let count = frame.chunk_changes.get(chunk).copied().unwrap_or(0);
            let Some((low, high)) = *bounds else { continue };
            if count == 0 || !(low.z..high.z).contains(&z) {
                continue;
            }
            let (sx, sy) = camera.tile_to_screen(low.x as f32, low.y as f32, screen);
            let alpha = 0.15 + 0.5 * count as f32 / max;
            let (width, height) = ((high.x - low.x) as f32 * s, (high.y - low.y) as f32 * s);
            draw_rectangle(sx, sy, width, height, Color::new(1.0, 0.2, 0.1, alpha));
        }
    }
    if overlays.jobs {
        for job in &frame.jobs {
            if job.pos.z != z {
                continue;
            }
            let (sx, sy) = camera.tile_to_screen(job.pos.x as f32, job.pos.y as f32, screen);
            let color = match job.kind {
                JobOverlay::Dig(_) => Color::new(1.0, 0.85, 0.1, 0.9),
                JobOverlay::Build(_) => Color::new(0.2, 0.9, 1.0, 0.9),
                JobOverlay::Haul => Color::new(0.6, 0.6, 0.6, 0.6),
                JobOverlay::Fell => Color::new(0.6, 0.35, 0.1, 0.9),
                JobOverlay::Farm => Color::new(0.3, 0.9, 0.3, 0.9),
            };
            let thickness = if job.claimed_by.is_some() { 2.5 } else { 1.0 };
            draw_rectangle_lines(sx + 1.0, sy + 1.0, s - 2.0, s - 2.0, thickness, color);
        }
    }
    for item in &frame.items {
        if item.pos.z != z || item.carried_by.is_some() {
            continue;
        }
        let (sx, sy) = camera.tile_to_screen(item.pos.x as f32, item.pos.y as f32, screen);
        let category = materials
            .item_categories
            .get(usize::from(item.kind.0))
            .map_or("", String::as_str);
        let color = match category {
            "stone" | "block" => Color::from_rgba(170, 170, 160, 255),
            "ore" | "bar" => Color::from_rgba(220, 160, 60, 255),
            "food" | "seed" => Color::from_rgba(80, 200, 80, 255),
            "log" | "fuel" => Color::from_rgba(150, 100, 50, 255),
            _ => Color::from_rgba(120, 170, 230, 255),
        };
        draw_rectangle(sx + s * 0.3, sy + s * 0.3, s * 0.4, s * 0.4, color);
    }
    if overlays.path && selected.is_some() {
        let mut previous = None;
        for p in &frame.selected_path {
            let (sx, sy) = camera.tile_to_screen(p.x as f32 + 0.5, p.y as f32 + 0.5, screen);
            let color = if p.z == z {
                Color::new(1.0, 0.3, 1.0, 0.9)
            } else {
                Color::new(1.0, 0.3, 1.0, 0.3)
            };
            if let Some((px, py)) = previous {
                draw_line(px, py, sx, sy, 2.0, color);
            }
            previous = Some((sx, sy));
        }
    }
    for resident in &frame.residents {
        if resident.pos.z != z {
            continue;
        }
        let (sx, sy) = camera.tile_to_screen(
            resident.pos.x as f32 + 0.5,
            resident.pos.y as f32 + 0.5,
            screen,
        );
        let color = if resident.carried.is_some() {
            Color::from_rgba(255, 170, 60, 255)
        } else {
            Color::from_rgba(250, 230, 90, 255)
        };
        draw_circle(sx, sy, s * 0.35, color);
        if selected == Some(resident.id) {
            draw_circle_lines(sx, sy, s * 0.55, 2.0, WHITE);
        }
    }
    if let Some((low, high)) = selection
        && (low.z..=high.z).contains(&z)
    {
        let (sx, sy) = camera.tile_to_screen(low.x as f32, low.y as f32, screen);
        let (ex, ey) = camera.tile_to_screen(high.x as f32 + 1.0, high.y as f32 + 1.0, screen);
        draw_rectangle(sx, sy, ex - sx, ey - sy, Color::new(1.0, 1.0, 1.0, 0.12));
        draw_rectangle_lines(sx, sy, ex - sx, ey - sy, 2.0, WHITE);
    }
}

pub fn draw_panel(lines: &[String], x: f32, y: f32) {
    let height = 18.0 * lines.len() as f32 + 10.0;
    let width = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as f32 * 8.5 + 16.0;
    draw_rectangle(x, y, width, height, Color::new(0.0, 0.0, 0.0, 0.75));
    for (i, line) in lines.iter().enumerate() {
        draw_text(line, x + 8.0, y + 20.0 + 18.0 * i as f32, 18.0, WHITE);
    }
}
