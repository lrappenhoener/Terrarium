//! Workshops and production.
//!
//! A workshop has a square footprint of standable floor tiles. Ingredients arrive on
//! the input tile (the footprint corner), the worker stands on the center tile and
//! products appear on the output tile (the opposite corner). Building a workshop is
//! a job like a recipe without products: its materials are fetched and consumed.
//!
//! A job reserves all of its ingredients atomically when it is assigned
//! (`GroupClaims`), so two workshops can never hold half of the same ingredients
//! each. The worker then carries them one by one, works and consumes exactly the
//! recipe amounts; leftover units stay on the input tile and are hauled away.

use sim_core::{
    entity::EntityCommand,
    space::{Pos3, standable},
};

use crate::{
    FortressSim, ItemId, ResKey, ResidentId, Route,
    content::{Input, OutputMaterial},
    types::{ItemKind, ItemLocation, JobKind, Op, Task, Workshop},
};

pub(crate) const CRAFT_PRIORITY: i32 = 4;
/// Planning stage of `Task::Craft`.
pub(crate) const PLAN: u8 = 3;
/// Items one job may reserve; bounds the planning search.
pub(crate) const MAX_JOB_ITEMS: usize = 16;

impl FortressSim {
    pub(crate) fn workshop_index(&self, id: u32) -> Option<usize> {
        self.workshops.binary_search_by_key(&id, |w| w.id).ok()
    }

    fn size_of(&self, kind: u16) -> i32 {
        self.content
            .workshops
            .get(usize::from(kind))
            .map_or(1, |w| w.size)
    }

    pub(crate) fn input_tile(&self, w: &Workshop) -> Pos3 {
        w.pos
    }

    pub(crate) fn output_tile(&self, w: &Workshop) -> Pos3 {
        let s = self.size_of(w.kind) - 1;
        w.pos.offset(s, s, 0)
    }

    pub(crate) fn work_tile(&self, w: &Workshop) -> Pos3 {
        let half = self.size_of(w.kind) / 2;
        w.pos.offset(half, half, 0)
    }

    /// Whether `p` lies in any workshop footprint.
    pub(crate) fn is_workshop_tile(&self, p: Pos3) -> bool {
        self.workshops.iter().any(|w| {
            let s = self.size_of(w.kind);
            p.z == w.pos.z
                && (w.pos.x..w.pos.x + s).contains(&p.x)
                && (w.pos.y..w.pos.y + s).contains(&p.y)
        })
    }

    /// Places a workshop if its whole footprint is free, standable floor.
    pub(crate) fn place_workshop(&mut self, kind: u16, pos: Pos3) -> bool {
        let s = self.size_of(kind);
        if self.content.workshops.get(usize::from(kind)).is_none() {
            return false;
        }
        for dy in 0..s {
            for dx in 0..s {
                let p = pos.offset(dx, dy, 0);
                let free = self.world.get(p).is_some_and(|t| standable(*t))
                    && !self.is_stockpile(p)
                    && self.farms.binary_search_by_key(&p, |f| f.pos).is_err()
                    && !self.is_workshop_tile(p);
                if !free {
                    return false;
                }
            }
        }
        let id = self.next_workshop;
        self.next_workshop += 1;
        self.workshops.push(Workshop {
            id,
            kind,
            pos,
            built: false,
            job: None,
        });
        self.add_craft_job(id, None).is_some()
    }

    /// Adds a construction (`recipe` none) or production job for an idle workshop.
    pub(crate) fn add_craft_job(
        &mut self,
        workshop: u32,
        recipe: Option<u16>,
    ) -> Option<sim_core::jobs::JobId> {
        let i = self.workshop_index(workshop)?;
        let w = self.workshops[i];
        if w.job.is_some() || w.built == recipe.is_none() {
            return None;
        }
        let at = self.work_tile(&w);
        let job = self
            .jobs
            .add(CRAFT_PRIORITY, at, JobKind::Craft { workshop, recipe })?;
        self.workshops[i].job = Some(job);
        Some(job)
    }

    fn inputs_of(&self, workshop: u32, recipe: Option<u16>) -> Option<&[Input]> {
        let i = self.workshop_index(workshop)?;
        match recipe {
            Some(r) => self
                .content
                .recipes
                .get(usize::from(r))
                .map(|r| r.inputs.as_slice()),
            None => self
                .content
                .workshops
                .get(usize::from(self.workshops[i].kind))
                .map(|w| w.build.as_slice()),
        }
    }

    /// Chooses free, reachable stacks near the work tile that cover every input,
    /// writing them to `out`. Fails without side effects if any input is missing.
    pub(crate) fn plan_ingredients(
        &self,
        workshop: u32,
        recipe: Option<u16>,
        out: &mut Vec<ItemId>,
    ) -> bool {
        out.clear();
        let (Some(i), Some(inputs)) = (
            self.workshop_index(workshop),
            self.inputs_of(workshop, recipe),
        ) else {
            return false;
        };
        let at = self.work_tile(&self.workshops[i]);
        for input in inputs {
            let mut units = 0;
            while units < input.amount {
                if out.len() >= MAX_JOB_ITEMS {
                    return false;
                }
                let Some((_, item)) = self.nearest_item(at, input, None, out) else {
                    return false;
                };
                units += self.islot(item).map_or(0, |s| self.icols.quantity[s]);
                out.push(item);
            }
        }
        true
    }

    /// Reserved ingredients of `holder` that are not yet on the input tile, lowest
    /// ID first.
    fn next_ingredient(&self, holder: ResidentId, input: Pos3) -> Option<ItemId> {
        self.reservations
            .iter()
            .filter(|r| r.holder == holder)
            .filter_map(|r| match r.key {
                ResKey::Item(item) => Some(item),
                ResKey::Slot(_) => None,
            })
            .filter(|&item| {
                self.islot(item)
                    .is_some_and(|s| self.icols.location[s] != ItemLocation::Ground(input))
            })
            .min()
    }

    /// Advances a workshop task; returns true when the job is finished.
    pub(crate) fn work_craft(&mut self, slot: usize, id: ResidentId, task: Task) -> bool {
        let Task::Craft {
            job,
            workshop,
            item,
            stage,
        } = task
        else {
            return false;
        };
        let Some(w) = self.workshop_index(workshop).map(|i| self.workshops[i]) else {
            self.scratch.abort.push(id);
            return false;
        };
        let Some(recipe) = self.jobs.get(job).and_then(|j| match j.data {
            JobKind::Craft { recipe, .. } => Some(recipe),
            _ => None,
        }) else {
            self.scratch.abort.push(id);
            return false;
        };
        let input = self.input_tile(&w);
        let at = self.rcols.position[slot];
        let set = |sim: &mut Self, item, stage| {
            sim.rcols.task[slot] = Task::Craft {
                job,
                workshop,
                item,
                stage,
            };
            sim.rcols.route[slot] = Route::NONE;
        };
        match stage {
            0 => {
                let Some(item) = item.filter(|&i| self.item_ground(i) == Some(at)) else {
                    self.scratch.abort.push(id);
                    return false;
                };
                self.pick_up(slot, id, item);
                set(self, Some(item), 1);
                false
            }
            1 => {
                if let Some(item) = self.rcols.carried[slot].take() {
                    self.place_on_ground(item, input);
                }
                set(self, None, PLAN);
                false
            }
            2 => {
                if self.next_ingredient(id, input).is_some() {
                    set(self, None, PLAN);
                    return false;
                }
                let (skill, base) = match recipe {
                    Some(r) => {
                        let def = &self.content.recipes[usize::from(r)];
                        (Some(def.skill), def.work_ticks)
                    }
                    None => (
                        None,
                        self.content.workshops[usize::from(w.kind)].build_ticks,
                    ),
                };
                let duration = skill.map_or(base, |s| self.skilled_ticks(slot, s, base));
                if !crate::tick_work(&mut self.rcols.work[slot], duration) {
                    return false;
                }
                self.finish_craft(slot, id, &w, recipe);
                if let Some(skill) = skill {
                    self.gain_experience(slot, skill, base);
                }
                if let Some(i) = self.workshop_index(workshop) {
                    self.workshops[i].job = None;
                    if recipe.is_none() {
                        self.workshops[i].built = true;
                        self.counters.workshops_built += 1;
                    }
                }
                self.jobs.complete(job);
                self.counters.completed_jobs += 1;
                true
            }
            _ => {
                match self.next_ingredient(id, input) {
                    Some(next) => set(self, Some(next), 0),
                    None => set(self, None, 2),
                }
                false
            }
        }
    }

    /// Consumes the recipe amounts from the reserved items on the input tile and
    /// creates the products on the output tile.
    fn finish_craft(&mut self, slot: usize, id: ResidentId, w: &Workshop, recipe: Option<u16>) {
        let input = self.input_tile(w);
        let count = self.inputs_of(w.id, recipe).map_or(0, <[Input]>::len);
        let mut held = std::mem::take(&mut self.scratch.ingredients);
        held.clear();
        held.extend(
            self.reservations
                .iter()
                .filter(|r| r.holder == id)
                .filter_map(|r| match r.key {
                    ResKey::Item(item) => Some(item),
                    ResKey::Slot(_) => None,
                }),
        );
        held.sort_unstable();
        let mut materials = [0u16; crate::content::MAX_INPUTS];
        self.scratch.used.clear();
        for k in 0..count {
            let Some(wanted) = self.inputs_of(w.id, recipe).and_then(|i| i.get(k)).copied() else {
                continue;
            };
            let mut needed = wanted.amount;
            for &item in &held {
                if needed == 0 {
                    break;
                }
                let Some(s) = self.islot(item) else { continue };
                if self.scratch.used.contains(&item)
                    || self.icols.location[s] != ItemLocation::Ground(input)
                    || self.icols.quantity[s] == 0
                    || !self.content.accepts(&wanted, self.icols.kind[s].0)
                {
                    continue;
                }
                if needed == wanted.amount
                    && let Some(m) = materials.get_mut(k)
                {
                    *m = self.icols.material[s];
                }
                let take = needed.min(self.icols.quantity[s]);
                needed -= take;
                if take == self.icols.quantity[s] {
                    // The item leaves at the tick end; later inputs must skip it.
                    self.scratch.used.push(item);
                }
                self.consume_units(id, item, take);
            }
        }
        self.scratch.used.clear();
        self.scratch.ingredients = held;
        self.reservations.release_holder(id);
        let Some(r) = recipe else { return };
        let skill = self.content.recipes[usize::from(r)].skill;
        let quality = self.skilled_quality(slot, skill);
        let out = self.output_tile(w);
        for k in 0..self.content.recipes[usize::from(r)].outputs.len() {
            let output = self.content.recipes[usize::from(r)].outputs[k];
            let material = match output.material {
                OutputMaterial::Fixed(m) => m,
                OutputMaterial::Of(i) => materials[usize::from(i)],
                OutputMaterial::Smelted(i) => {
                    let ore = materials[usize::from(i)];
                    self.content
                        .material(ore)
                        .and_then(|m| m.smelts_into)
                        .unwrap_or(ore)
                }
            };
            self.ops.push(
                (crate::SOURCE_RESIDENT, id.to_u64()),
                k as u32,
                EntityCommand::Spawn(Op::SpawnItem {
                    at: out,
                    kind: ItemKind(output.item),
                    material,
                    quantity: output.amount,
                    quality,
                }),
            );
            self.counters.crafted += u64::from(output.amount);
        }
    }
}
