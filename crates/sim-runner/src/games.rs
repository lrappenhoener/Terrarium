use sim_runner::{RunOptions, RunReport, RunnerError, run_simulation};

pub struct GameEntry {
    pub name: &'static str,
    pub run: fn(&RunOptions) -> Result<RunReport, RunnerError>,
}

pub const GAMES: &[GameEntry] = &[
    GameEntry {
        name: "dummy",
        run: run_simulation::<dummy_sim::DummySim>,
    },
    GameEntry {
        name: "jungle",
        run: run_simulation::<jungle_sim::JungleSim>,
    },
];

pub const DEFAULT_GAME: &str = GAMES[0].name;
