//! Developer text view of one level.

use sim_core::space::{Pos3, TileKind};

use crate::{FortressSim, types::ItemLocation, world};

pub const LEGEND: &str = "@ resident  o stone  $ ore item  f food  i other item  # rock  * ore  : soil  W wall  \
. floor  = stockpile  < up  > down  X up/down  ^ ramp  + door  (blank) air";

impl FortressSim {
    /// Renders level `z`: residents over items over tiles.
    pub fn render_level(&self, z: i32) -> Option<String> {
        if !(0..self.world.depth()).contains(&z) {
            return None;
        }
        let generated = world::generate(self.seed, &self.config, &self.content).ok()?;
        let (w, h) = (self.world.width(), self.world.height());
        let mut cells = vec![b' '; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                let p = Pos3::new(x, y, z);
                let (Some(tile), Some(original)) = (self.world.get(p), generated.get(p)) else {
                    continue;
                };
                let ore = self
                    .content
                    .material(tile.material)
                    .is_some_and(|m| m.class == crate::content::MaterialClass::Ore);
                cells[(y * w + x) as usize] = match tile.kind {
                    TileKind::Solid if tile != original => b'W',
                    TileKind::Solid if ore => b'*',
                    TileKind::Solid if tile.material == self.content.soil => b':',
                    TileKind::Solid => b'#',
                    TileKind::Open if tile.floor() && self.stockpiles.binary_search(&p).is_ok() => {
                        b'='
                    }
                    TileKind::Open if tile.floor() => b'.',
                    TileKind::Open => b' ',
                    TileKind::StairUp => b'<',
                    TileKind::StairDown => b'>',
                    TileKind::StairUpDown => b'X',
                    TileKind::Ramp => b'^',
                    TileKind::Door => b'+',
                };
            }
        }
        for slot in 0..self.icols.len() {
            if let ItemLocation::Ground(p) = self.icols.location[slot]
                && p.z == z
            {
                let category = self.content.category_of(self.icols.kind[slot].0);
                cells[(p.y * w + p.x) as usize] = match category {
                    c if c == self.content.stone => b'o',
                    c if c == self.content.food => b'f',
                    c if self.content.ids.categories[usize::from(c)] == "ore" => b'$',
                    _ => b'i',
                };
            }
        }
        for p in &self.rcols.position {
            if p.z == z {
                cells[(p.y * w + p.x) as usize] = b'@';
            }
        }
        let mut out = format!("level z={z} tick={}\n{LEGEND}\n", self.tick);
        for row in cells.chunks(w as usize) {
            out.push_str(String::from_utf8_lossy(row).trim_end());
            out.push('\n');
        }
        Some(out)
    }
}
