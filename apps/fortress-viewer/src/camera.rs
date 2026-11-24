//! Top-down camera: tile and screen coordinates, panning, zoom and levels.

use sim_core::space::Pos3;

pub const MIN_TILE_PX: f32 = 4.0;
pub const MAX_TILE_PX: f32 = 64.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// World tile coordinate at the screen center.
    pub center_x: f32,
    pub center_y: f32,
    pub tile_px: f32,
    pub level: i32,
}

impl Camera {
    pub fn new(center_x: f32, center_y: f32, level: i32) -> Self {
        Self {
            center_x,
            center_y,
            tile_px: 12.0,
            level,
        }
    }

    /// Top-left screen corner of tile `(x, y)`.
    pub fn tile_to_screen(&self, x: f32, y: f32, screen: (f32, f32)) -> (f32, f32) {
        (
            (x - self.center_x) * self.tile_px + screen.0 / 2.0,
            (y - self.center_y) * self.tile_px + screen.1 / 2.0,
        )
    }

    /// Tile under a screen point.
    pub fn screen_to_tile(&self, sx: f32, sy: f32, screen: (f32, f32)) -> (i32, i32) {
        (
            ((sx - screen.0 / 2.0) / self.tile_px + self.center_x).floor() as i32,
            ((sy - screen.1 / 2.0) / self.tile_px + self.center_y).floor() as i32,
        )
    }

    pub fn pan_pixels(&mut self, dx: f32, dy: f32) {
        self.center_x -= dx / self.tile_px;
        self.center_y -= dy / self.tile_px;
    }

    pub fn pan_tiles(&mut self, dx: f32, dy: f32) {
        self.center_x += dx;
        self.center_y += dy;
    }

    /// Zooms by `factor`, keeping the world point under the cursor fixed.
    pub fn zoom_at(&mut self, factor: f32, sx: f32, sy: f32, screen: (f32, f32)) {
        let wx = (sx - screen.0 / 2.0) / self.tile_px + self.center_x;
        let wy = (sy - screen.1 / 2.0) / self.tile_px + self.center_y;
        self.tile_px = (self.tile_px * factor).clamp(MIN_TILE_PX, MAX_TILE_PX);
        self.center_x = wx - (sx - screen.0 / 2.0) / self.tile_px;
        self.center_y = wy - (sy - screen.1 / 2.0) / self.tile_px;
    }

    pub fn change_level(&mut self, delta: i32, depth: i32) {
        self.level = (self.level + delta).clamp(0, depth - 1);
    }

    pub fn follow(&mut self, p: Pos3) {
        self.center_x = p.x as f32 + 0.5;
        self.center_y = p.y as f32 + 0.5;
        self.level = p.z;
    }

    /// Visible tile range `(x0, y0, x1, y1)`, inclusive, clipped to the world.
    pub fn visible(&self, screen: (f32, f32), width: i32, height: i32) -> (i32, i32, i32, i32) {
        let (x0, y0) = self.screen_to_tile(0.0, 0.0, screen);
        let (x1, y1) = self.screen_to_tile(screen.0, screen.1, screen);
        (x0.max(0), y0.max(0), x1.min(width - 1), y1.min(height - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (f32, f32) = (800.0, 600.0);

    #[test]
    fn screen_and_tile_coordinates_round_trip() {
        let camera = Camera::new(10.0, 20.0, 3);
        for (x, y) in [(0, 0), (10, 20), (37, 5)] {
            let (sx, sy) = camera.tile_to_screen(x as f32, y as f32, SCREEN);
            assert_eq!(camera.screen_to_tile(sx + 1.0, sy + 1.0, SCREEN), (x, y));
        }
        assert_eq!(camera.screen_to_tile(400.0, 300.0, SCREEN), (10, 20));
    }

    #[test]
    fn zoom_keeps_the_cursor_tile_and_clamps() {
        let mut camera = Camera::new(10.0, 10.0, 0);
        let before = camera.screen_to_tile(123.0, 77.0, SCREEN);
        camera.zoom_at(2.0, 123.0, 77.0, SCREEN);
        assert_eq!(camera.screen_to_tile(123.0, 77.0, SCREEN), before);
        camera.zoom_at(100.0, 0.0, 0.0, SCREEN);
        assert_eq!(camera.tile_px, MAX_TILE_PX);
        camera.zoom_at(0.0001, 0.0, 0.0, SCREEN);
        assert_eq!(camera.tile_px, MIN_TILE_PX);
    }

    #[test]
    fn panning_levels_and_following() {
        let mut camera = Camera::new(5.0, 5.0, 2);
        camera.pan_pixels(24.0, -12.0);
        assert_eq!((camera.center_x, camera.center_y), (3.0, 6.0));
        camera.change_level(-5, 32);
        assert_eq!(camera.level, 0);
        camera.change_level(40, 32);
        assert_eq!(camera.level, 31);
        camera.follow(Pos3::new(7, 8, 9));
        assert_eq!(
            (camera.center_x, camera.center_y, camera.level),
            (7.5, 8.5, 9)
        );
        assert_eq!(Camera::new(5.0, 5.0, 0).visible(SCREEN, 8, 8), (0, 0, 7, 7));
    }
}
