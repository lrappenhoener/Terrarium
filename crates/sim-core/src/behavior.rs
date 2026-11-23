pub type ActionId = u32;

pub fn select_best(candidates: &[(ActionId, i32)]) -> Option<ActionId> {
    candidates
        .iter()
        .max_by_key(|&&(action, score)| (score, std::cmp::Reverse(action)))
        .map(|&(action, _)| action)
}
