//! Calendar, trees, farm plots and skills.
//!
//! Trees and crops are plant entities on a tile; they never block movement, so the
//! topology does not change when they grow or are felled. Trees are placed at
//! embark on surface floors outside the mountain and do not regrow. Farm plots ask
//! for sowing when a seed exists and for harvesting when their plant is ripe. Plants
//! grow in staggered updates, only in the seasons their content allows.

use sim_core::{
    entity::EntityCommand,
    jobs::JobState,
    rng::{below, stateless_u64},
    space::{Pos3, standable},
};

use crate::{
    FortressSim, MAX_QUALITY, SOURCE_EVENT, box_range, box_volume,
    content::Ingredient,
    types::{FarmPlot, ItemKind, JobKind, Op, PlantId, ResidentId, Task},
    world,
};

const FELL_PRIORITY: i32 = 3;
const FARM_PRIORITY: i32 = 4;
/// Salt of the stateless tree placement draw.
const TREE_SALT: u64 = 0x7472_6565;

impl FortressSim {
    /// Season index 0..4 (spring, summer, autumn, winter) of the current tick.
    pub(crate) fn season(&self) -> usize {
        let days = self.tick / self.config.ticks_per_day;
        ((days / self.config.days_per_season) % 4) as usize
    }

    fn grows_now(&self, kind: u16) -> bool {
        self.content
            .plants
            .get(usize::from(kind))
            .is_some_and(|p| p.seasons & (1 << self.season()) != 0)
    }

    /// Grown trees on surface floors outside the mountain, away from the embark.
    pub(crate) fn plant_initial_trees(&mut self) {
        let Some(tree) = self.content.plants.iter().position(|p| p.tree) else {
            return;
        };
        let growth = self.content.plants[tree].growth_ticks;
        let size = self.config.world_size_xy;
        let embark = self.embark();
        let r2 = i64::from(self.config.mountain_radius).pow(2);
        for y in 0..size {
            for x in 0..size {
                let dx = i64::from(x - self.config.mountain_center_x);
                let dy = i64::from(y - self.config.mountain_center_y);
                let p = Pos3::new(x, y, world::surface(self.seed, &self.config, x, y));
                let roll = below(
                    stateless_u64(self.seed, x as u64, y as u64, TREE_SALT),
                    1000,
                );
                if roll < u64::from(self.config.tree_density)
                    && dx * dx + dy * dy >= r2
                    && (x - embark.x).abs().max((y - embark.y).abs()) > 4
                    && self.world.get(p).is_some_and(|t| standable(*t))
                    && self.fluid_at(p) == 0
                {
                    self.create_plant(p, tree as u16, growth);
                }
            }
        }
    }

    pub(crate) fn create_plant(&mut self, at: Pos3, kind: u16, growth: u32) -> Option<PlantId> {
        let id = self.plants.create()?;
        self.pcols.push(at, kind, growth, None);
        if let Ok(i) = self.farms.binary_search_by_key(&at, |plot| plot.pos)
            && self.farms[i].plant.is_none()
        {
            self.farms[i].plant = Some(id);
        }
        Some(id)
    }

    pub(crate) fn remove_plant(&mut self, id: PlantId) {
        let Some(slot) = self.pslot(id) else { return };
        if let Some(job) = self.pcols.job[slot]
            && let Some(JobState::Claimed { by, .. }) = self.jobs.get(job).map(|j| j.state)
        {
            self.scratch.abort.push(by);
        }
        if let Some(job) = self.pcols.job[slot] {
            self.jobs.remove(job);
        }
        let at = self.pcols.pos[slot];
        if let Ok(i) = self.farms.binary_search_by_key(&at, |plot| plot.pos)
            && self.farms[i].plant == Some(id)
        {
            self.farms[i].plant = None;
        }
        if let Some(slot) = self.plants.destroy(id) {
            self.pcols.swap_remove(slot);
        }
    }

    pub(crate) fn pslot(&self, id: PlantId) -> Option<usize> {
        self.plants.slot(id).map(|slot| slot as usize)
    }

    pub(crate) fn plant_ripe(&self, id: PlantId) -> bool {
        self.pslot(id).is_some_and(|slot| {
            self.content
                .plants
                .get(usize::from(self.pcols.kind[slot]))
                .is_some_and(|p| self.pcols.growth[slot] >= p.growth_ticks)
        })
    }

    /// Phase `plants`: staggered growth, then farm plot requests.
    pub(crate) fn plants_step(&mut self) {
        let interval = self.config.plant_interval;
        for slot in 0..self.pcols.len() {
            let Some(id) = self.plants.id(slot as u32) else {
                continue;
            };
            if !(self.tick + u64::from(id.index)).is_multiple_of(interval) {
                continue;
            }
            let kind = self.pcols.kind[slot];
            let Some(full) = self
                .content
                .plants
                .get(usize::from(kind))
                .map(|p| p.growth_ticks)
            else {
                continue;
            };
            if self.pcols.growth[slot] < full && self.grows_now(kind) {
                let grown = self.pcols.growth[slot].saturating_add(interval as u32);
                self.pcols.growth[slot] = grown.min(full);
            }
        }
        self.manage_farms();
    }

    fn manage_farms(&mut self) {
        let interval = self.config.farm_interval;
        for i in 0..self.farms.len() {
            if !(self.tick + i as u64).is_multiple_of(interval) {
                continue;
            }
            let plot = self.farms[i];
            if let Some(job) = plot.job
                && self.jobs.get(job).is_none_or(|j| j.state == JobState::Done)
            {
                self.farms[i].job = None;
                continue;
            }
            if plot.job.is_some() || !self.connectivity.is_standable(plot.pos) {
                continue;
            }
            match plot.plant {
                Some(plant) if !self.plants.is_alive(plant) => self.farms[i].plant = None,
                Some(plant) => {
                    let free = self
                        .pslot(plant)
                        .is_some_and(|s| self.pcols.job[s].is_none());
                    if free && self.plant_ripe(plant) {
                        let job = self
                            .jobs
                            .add(FARM_PRIORITY, plot.pos, JobKind::Harvest(plant));
                        if let (Some(job), Some(slot)) = (job, self.pslot(plant)) {
                            self.pcols.job[slot] = Some(job);
                            self.farms[i].job = Some(job);
                        }
                    }
                }
                None => {
                    let seed = self
                        .content
                        .plants
                        .get(usize::from(plot.crop))
                        .and_then(|p| p.seed);
                    let has_seed = seed.is_some_and(|s| {
                        let ledger = self.ledger[usize::from(s)];
                        ledger.produced > ledger.removed
                    });
                    if has_seed && self.grows_now(plot.crop) {
                        self.farms[i].job = self.jobs.add(FARM_PRIORITY, plot.pos, JobKind::Sow);
                    }
                }
            }
        }
    }

    /// Designates farm plots; returns the number of rejected tiles.
    pub(crate) fn designate_farm(&mut self, from: Pos3, to: Pos3, crop: u16) -> u64 {
        let (low, high) = box_range(from, to);
        if box_volume(low, high) > u64::from(self.config.max_farm_plots) {
            return box_volume(low, high);
        }
        let Some(def) = self.content.plants.get(usize::from(crop)) else {
            return box_volume(low, high);
        };
        let (underground, tree) = (def.underground, def.tree);
        let mut rejected = 0;
        for p in (low.z..=high.z).flat_map(|z| {
            (low.y..=high.y).flat_map(move |y| (low.x..=high.x).map(move |x| Pos3::new(x, y, z)))
        }) {
            let surface = world::surface(self.seed, &self.config, p.x, p.y);
            let level_ok = if underground {
                p.z < surface
            } else {
                p.z == surface
            };
            let at = self.farms.partition_point(|plot| plot.pos < p);
            let free = self.farms.get(at).is_none_or(|plot| plot.pos != p);
            if !tree
                && level_ok
                && free
                && self.farms.len() < self.config.max_farm_plots as usize
                && self.world.get(p).is_some_and(|t| standable(*t))
                && !self.is_stockpile(p)
                && !self.is_workshop_tile(p)
            {
                self.farms.insert(
                    at,
                    FarmPlot {
                        pos: p,
                        crop,
                        plant: None,
                        job: None,
                    },
                );
            } else {
                rejected += 1;
            }
        }
        rejected
    }

    /// Adds felling jobs for grown trees in the box; returns the number added.
    pub(crate) fn designate_felling(&mut self, from: Pos3, to: Pos3) -> u64 {
        let (low, high) = box_range(from, to);
        let inside = |p: Pos3| {
            (low.x..=high.x).contains(&p.x)
                && (low.y..=high.y).contains(&p.y)
                && (low.z..=high.z).contains(&p.z)
        };
        let mut added = 0;
        let mut ids = std::mem::take(&mut self.scratch.plant_ids);
        ids.clear();
        ids.extend(self.plants.iter_by_id());
        for &id in &ids {
            let Some(slot) = self.pslot(id) else { continue };
            let p = self.pcols.pos[slot];
            let tree = self
                .content
                .plants
                .get(usize::from(self.pcols.kind[slot]))
                .is_some_and(|def| def.tree);
            if tree
                && inside(p)
                && self.pcols.job[slot].is_none()
                && self.plant_ripe(id)
                && let Some(job) = self.jobs.add(FELL_PRIORITY, p, JobKind::Fell(id))
            {
                self.pcols.job[slot] = Some(job);
                added += 1;
            }
        }
        self.scratch.plant_ids = ids;
        added
    }

    pub(crate) fn skill_level(&self, slot: usize, skill: u16) -> u32 {
        let experience = self.rcols.skills[slot]
            .0
            .get(usize::from(skill))
            .copied()
            .unwrap_or(0);
        self.content
            .skills
            .get(usize::from(skill))
            .map_or(0, |s| s.level(experience))
    }

    /// Work ticks shrink with the level: level 5 needs half the base time.
    pub(crate) fn skilled_ticks(&self, slot: usize, skill: u16, base: u32) -> u32 {
        let level = self.skill_level(slot, skill);
        (base.saturating_mul(5) / (5 + level)).max(1)
    }

    /// Quality of a product: the level, at most `MAX_QUALITY`.
    pub(crate) fn skilled_quality(&self, slot: usize, skill: u16) -> u8 {
        self.skill_level(slot, skill).min(u32::from(MAX_QUALITY)) as u8
    }

    pub(crate) fn gain_experience(&mut self, slot: usize, skill: u16, base: u32) {
        if let Some(xp) = self.rcols.skills[slot].0.get_mut(usize::from(skill)) {
            *xp = xp.saturating_add(base / 10 + 5);
        }
    }

    /// Felling or harvesting on the plant's tile. Returns true when done.
    pub(crate) fn work_gather(
        &mut self,
        slot: usize,
        id: ResidentId,
        job: sim_core::jobs::JobId,
        plant: PlantId,
    ) -> bool {
        let Some(pslot) = self.pslot(plant) else {
            self.scratch.abort.push(id);
            return false;
        };
        let kind = self.pcols.kind[pslot];
        let Some(def) = self.content.plants.get(usize::from(kind)) else {
            self.scratch.abort.push(id);
            return false;
        };
        let (skill, base, material, at) = (
            def.skill,
            def.work_ticks,
            def.material,
            self.pcols.pos[pslot],
        );
        let duration = self.skilled_ticks(slot, skill, base);
        if !crate::tick_work(&mut self.rcols.work[slot], duration) {
            return false;
        }
        let quality = self.skilled_quality(slot, skill);
        for k in 0..self.content.plants[usize::from(kind)].harvest.len() {
            let (item, amount) = self.content.plants[usize::from(kind)].harvest[k];
            self.ops.push(
                (crate::SOURCE_RESIDENT, id.to_u64()),
                k as u32,
                EntityCommand::Spawn(Op::SpawnItem {
                    at,
                    kind: ItemKind(item),
                    material,
                    quantity: amount,
                    quality,
                }),
            );
        }
        self.ops.push(
            (crate::SOURCE_RESIDENT, id.to_u64()),
            8,
            EntityCommand::Spawn(Op::DespawnPlant(plant)),
        );
        self.pcols.job[pslot] = None;
        self.gain_experience(slot, skill, base);
        if self.content.plants[usize::from(kind)].tree {
            self.counters.felled += 1;
        } else {
            self.counters.harvested += 1;
        }
        self.complete_job(job);
        self.counters.completed_jobs += 1;
        true
    }

    /// Sowing: stage 0 takes one unit from the seed stack, stage 1 plants it.
    pub(crate) fn work_sow(&mut self, slot: usize, id: ResidentId, task: Task) -> bool {
        let Task::Sow {
            job,
            seed,
            plot,
            stage,
        } = task
        else {
            return false;
        };
        if stage == 0 {
            if self.item_ground(seed) != Some(self.rcols.position[slot]) {
                self.scratch.abort.push(id);
                return false;
            }
            self.consume_units(id, seed, 1);
            self.rcols.task[slot] = Task::Sow {
                job,
                seed,
                plot,
                stage: 1,
            };
            self.rcols.route[slot] = crate::Route::NONE;
            return false;
        }
        let Some(i) = self.farms.binary_search_by_key(&plot, |p| p.pos).ok() else {
            self.scratch.abort.push(id);
            return false;
        };
        let crop = self.farms[i].crop;
        let Some(def) = self.content.plants.get(usize::from(crop)) else {
            return false;
        };
        let (skill, base) = (def.skill, def.work_ticks);
        if self.farms[i].plant.is_some() {
            self.scratch.abort.push(id);
            return false;
        }
        let duration = self.skilled_ticks(slot, skill, base);
        if !crate::tick_work(&mut self.rcols.work[slot], duration) {
            return false;
        }
        self.ops.push(
            (crate::SOURCE_RESIDENT, id.to_u64()),
            0,
            EntityCommand::Spawn(Op::SpawnPlant {
                at: plot,
                kind: crop,
                growth: 0,
            }),
        );
        self.gain_experience(slot, skill, base);
        self.counters.sown += 1;
        // The plot keeps the finished job until the plant exists at the tick end.
        self.complete_job(job);
        self.counters.completed_jobs += 1;
        true
    }

    /// A sower aborted while holding a seed unit drops it again.
    pub(crate) fn return_seed(&mut self, id: ResidentId, at: Pos3, plot: Pos3) {
        let Ok(i) = self.farms.binary_search_by_key(&plot, |p| p.pos) else {
            return;
        };
        let crop = self.farms[i].crop;
        let Some(def) = self.content.plants.get(usize::from(crop)) else {
            return;
        };
        if let Some(seed) = def.seed {
            self.ops.push(
                (SOURCE_EVENT, id.to_u64()),
                u32::MAX,
                EntityCommand::Spawn(Op::SpawnItem {
                    at,
                    kind: ItemKind(seed),
                    material: def.material,
                    quantity: 1,
                    quality: 0,
                }),
            );
        }
    }

    /// The seed kind for a sow job at `plot`, as an ingredient.
    pub(crate) fn seed_input(&self, plot: Pos3) -> Option<crate::content::Input> {
        let i = self.farms.binary_search_by_key(&plot, |p| p.pos).ok()?;
        let seed = self
            .content
            .plants
            .get(usize::from(self.farms[i].crop))?
            .seed?;
        Some(crate::content::Input {
            what: Ingredient::Item(seed),
            amount: 1,
            raw: false,
        })
    }
}
