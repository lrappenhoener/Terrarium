//! Work orders and their distribution to workshops.
//!
//! Every `order_interval` ticks the manager visits idle, built workshops in ID
//! order and gives each the first due order (in order ID order) that its kind can
//! run and whose ingredients are available. An order counts a run when its
//! production job finishes; `Times` orders disappear after their last run.

use sim_core::jobs::JobState;

use crate::{
    FortressSim,
    types::{JobKind, OrderCondition, WorkOrder},
};

/// Orders per fortress.
pub(crate) const MAX_ORDERS: usize = 256;

impl FortressSim {
    pub(crate) fn order_index(&self, id: u32) -> Option<usize> {
        self.orders.binary_search_by_key(&id, |o| o.id).ok()
    }

    pub(crate) fn add_order(
        &mut self,
        workshop: Option<u32>,
        recipe: u16,
        amount: u32,
        condition: OrderCondition,
    ) -> bool {
        let Some(def) = self.content.recipes.get(usize::from(recipe)) else {
            return false;
        };
        let fits = workshop.is_none_or(|id| {
            self.workshop_index(id)
                .is_some_and(|i| self.workshops[i].kind == def.workshop)
        });
        if !fits
            || self.orders.len() >= MAX_ORDERS
            || (amount == 0 && condition != OrderCondition::Repeat)
        {
            return false;
        }
        self.orders.push(WorkOrder {
            id: self.next_order,
            workshop,
            recipe,
            amount,
            condition,
            done: 0,
        });
        self.next_order += 1;
        true
    }

    /// Removes an order and its production jobs; their workers abort.
    pub(crate) fn cancel_order(&mut self, id: u32) -> bool {
        let Some(i) = self.order_index(id) else {
            return false;
        };
        self.orders.remove(i);
        for w in 0..self.workshops.len() {
            if self.workshops[w].order != Some(id) {
                continue;
            }
            if let Some(job) = self.workshops[w].job.take() {
                if let Some(JobState::Claimed { by, .. }) = self.jobs.get(job).map(|j| j.state) {
                    self.scratch.abort.push(by);
                }
                self.jobs.remove(job);
            }
            self.workshops[w].order = None;
        }
        true
    }

    fn active_runs(&self, order: u32) -> u32 {
        self.workshops
            .iter()
            .filter(|w| w.order == Some(order) && w.job.is_some())
            .count() as u32
    }

    fn order_due(&self, order: &WorkOrder) -> bool {
        let active = self.active_runs(order.id);
        match order.condition {
            OrderCondition::Times => order.done + active < order.amount,
            OrderCondition::Repeat => true,
            OrderCondition::UntilStock => {
                let Some(output) = self
                    .content
                    .recipes
                    .get(usize::from(order.recipe))
                    .and_then(|r| r.outputs.first())
                else {
                    return false;
                };
                let ledger = self.ledger[usize::from(output.item)];
                let live = ledger.produced.saturating_sub(ledger.removed);
                live + u64::from(active) * u64::from(output.amount) < u64::from(order.amount)
            }
        }
    }

    /// Phase `orders`: gives idle workshops due orders with available ingredients.
    pub(crate) fn manage_orders(&mut self) {
        if self.orders.is_empty() || !self.tick.is_multiple_of(self.config.order_interval) {
            return;
        }
        for wi in 0..self.workshops.len() {
            let w = self.workshops[wi];
            if !w.built || w.job.is_some() {
                continue;
            }
            for oi in 0..self.orders.len() {
                let order = self.orders[oi];
                let runs_here = self
                    .content
                    .recipes
                    .get(usize::from(order.recipe))
                    .is_some_and(|r| r.workshop == w.kind)
                    && order.workshop.is_none_or(|id| id == w.id);
                if !runs_here || !self.order_due(&order) {
                    continue;
                }
                let mut items = std::mem::take(&mut self.scratch.ingredients);
                let available = self.plan_ingredients(w.id, Some(order.recipe), &mut items);
                self.scratch.ingredients = items;
                if !available {
                    self.diag.orders_waiting += 1;
                    continue;
                }
                if self.add_craft_job(w.id, Some(order.recipe)).is_some() {
                    self.workshops[wi].order = Some(order.id);
                }
                break;
            }
        }
    }

    /// Counts a finished run of the workshop's order.
    pub(crate) fn finish_order(&mut self, workshop: u32) {
        let Some(wi) = self.workshop_index(workshop) else {
            return;
        };
        let Some(id) = self.workshops[wi].order.take() else {
            return;
        };
        if let Some(i) = self.order_index(id) {
            self.orders[i].done = self.orders[i].done.saturating_add(1);
            let order = self.orders[i];
            if order.condition == OrderCondition::Times && order.done >= order.amount {
                self.orders.remove(i);
            }
        }
    }

    /// A production job whose ingredients vanished is dropped; the manager may
    /// offer the order again later.
    pub(crate) fn drop_craft_job(&mut self, job: sim_core::jobs::JobId) {
        let Some(JobKind::Craft {
            workshop,
            recipe: Some(_),
        }) = self
            .jobs
            .get(job)
            .filter(|j| j.state == JobState::Open)
            .map(|j| j.data)
        else {
            return;
        };
        if let Some(wi) = self.workshop_index(workshop) {
            self.workshops[wi].job = None;
            self.workshops[wi].order = None;
        }
        self.jobs.remove(job);
    }
}
