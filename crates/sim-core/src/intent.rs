use crate::entity::EntityId;

pub struct IntentBuffer<K, I> {
    entries: Vec<(K, EntityId, I)>,
}

impl<K: Ord, I> IntentBuffer<K, I> {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
        }
    }

    /// Each source may submit at most one intent per conflict key.
    pub fn push(&mut self, key: K, source: EntityId, intent: I) {
        self.entries.push((key, source, intent));
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Smaller priorities win. Remaining entries in the group are the losers in rank order.
    pub fn resolve<P: Ord>(
        &mut self,
        priority: impl Fn(&I) -> P,
        mut group: impl FnMut(&(K, EntityId, I), &[(K, EntityId, I)]),
    ) {
        self.entries.sort_unstable_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| priority(&a.2).cmp(&priority(&b.2)))
                .then_with(|| a.1.cmp(&b.1))
        });
        for entries in self.entries.chunk_by(|a, b| a.0 == b.0) {
            if let Some((winner, losers)) = entries.split_first() {
                group(winner, losers);
            }
        }
    }
}
