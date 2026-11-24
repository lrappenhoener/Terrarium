//! Tools, selections and key bindings. Every action produces existing
//! `FortressCommand`s; there are no other ways to change the simulation.

use fortress_sim::{BuildKind, DigMode, FortressCommand};
use sim_core::space::Pos3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Dig(DigMode),
    Stockpile,
    Build(BuildKind),
    /// Removes designations and stockpiles in the selection.
    Cancel,
    Inspect,
}

impl Tool {
    pub fn label(self) -> String {
        match self {
            Self::Dig(mode) => format!("Graben ({mode:?})"),
            Self::Stockpile => "Lagerzone".into(),
            Self::Build(kind) => format!("Bauen ({kind:?})"),
            Self::Cancel => "Aufheben".into(),
            Self::Inspect => "Untersuchen".into(),
        }
    }

    /// Next variant of the same tool family.
    pub fn cycle(self) -> Self {
        match self {
            Self::Dig(DigMode::Dig) => Self::Dig(DigMode::StairUpDown),
            Self::Dig(DigMode::StairUpDown) => Self::Dig(DigMode::StairDown),
            Self::Dig(DigMode::StairDown) => Self::Dig(DigMode::StairUp),
            Self::Dig(DigMode::StairUp) => Self::Dig(DigMode::Ramp),
            Self::Dig(DigMode::Ramp) => Self::Dig(DigMode::Dig),
            Self::Build(BuildKind::Wall) => Self::Build(BuildKind::Floor),
            Self::Build(BuildKind::Floor) => Self::Build(BuildKind::StairUpDown),
            Self::Build(BuildKind::StairUpDown) => Self::Build(BuildKind::Door),
            Self::Build(BuildKind::Door) => Self::Build(BuildKind::Wall),
            other => other,
        }
    }
}

/// A selection started at `start`; its far corner follows the cursor and the
/// current level, so a drag can span several levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub start: Pos3,
}

impl Selection {
    /// Normalized inclusive box from the start to the current tile and level.
    pub fn bounds(self, x: i32, y: i32, level: i32) -> (Pos3, Pos3) {
        let s = self.start;
        (
            Pos3::new(s.x.min(x), s.y.min(y), s.z.min(level)),
            Pos3::new(s.x.max(x), s.y.max(y), s.z.max(level)),
        )
    }
}

/// Maximum number of single-tile build commands one selection may create.
pub const MAX_BUILD_TILES: usize = 4096;

/// Commands of a tool applied to an inclusive box.
pub fn commands_for(tool: Tool, low: Pos3, high: Pos3) -> Vec<FortressCommand> {
    match tool {
        Tool::Dig(mode) => vec![FortressCommand::DesignateDig {
            from: low,
            to: high,
            mode,
        }],
        Tool::Stockpile => vec![FortressCommand::DesignateStockpile {
            from: low,
            to: high,
        }],
        Tool::Build(kind) => {
            let mut out = Vec::new();
            'outer: for z in low.z..=high.z {
                for y in low.y..=high.y {
                    for x in low.x..=high.x {
                        if out.len() >= MAX_BUILD_TILES {
                            break 'outer;
                        }
                        out.push(FortressCommand::Build {
                            pos: Pos3::new(x, y, z),
                            kind,
                        });
                    }
                }
            }
            out
        }
        Tool::Cancel => vec![FortressCommand::CancelDesignations {
            from: low,
            to: high,
        }],
        Tool::Inspect => Vec::new(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speed {
    Paused,
    X1,
    X2,
    X5,
    Max,
}

impl Speed {
    /// Target ticks per second; `None` runs as fast as possible.
    pub fn ticks_per_second(self, base: f64) -> Option<f64> {
        match self {
            Self::Paused => Some(0.0),
            Self::X1 => Some(base),
            Self::X2 => Some(base * 2.0),
            Self::X5 => Some(base * 5.0),
            Self::Max => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Paused => "Pause",
            Self::X1 => "1×",
            Self::X2 => "2×",
            Self::X5 => "5×",
            Self::Max => "Max",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    Jobs,
    Path,
    Regions,
    Heatmap,
    Temperature,
    ActiveChunks,
    Support,
}

/// Keys the viewer reacts to, independent of the graphics library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    W,
    A,
    S,
    D,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    LessThan,
    GreaterThan,
    Space,
    N,
    Digit(u8),
    G,
    Z,
    B,
    C,
    I,
    M,
    F,
    Escape,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Pan(f32, f32),
    Level(i32),
    TogglePause,
    Step,
    Speed(Speed),
    Tool(Tool),
    CycleTool,
    Follow,
    Deselect,
    Overlay(Overlay),
    Save,
    Load,
}

/// Fixed key map, documented in `README.md`.
pub fn action_for_key(key: Key) -> Option<Action> {
    Some(match key {
        Key::W | Key::Up => Action::Pan(0.0, -1.0),
        Key::S | Key::Down => Action::Pan(0.0, 1.0),
        Key::A | Key::Left => Action::Pan(-1.0, 0.0),
        Key::D | Key::Right => Action::Pan(1.0, 0.0),
        Key::PageUp | Key::LessThan => Action::Level(1),
        Key::PageDown | Key::GreaterThan => Action::Level(-1),
        Key::Space => Action::TogglePause,
        Key::N => Action::Step,
        Key::Digit(0) => Action::Speed(Speed::Paused),
        Key::Digit(1) => Action::Speed(Speed::X1),
        Key::Digit(2) => Action::Speed(Speed::X2),
        Key::Digit(3) => Action::Speed(Speed::X5),
        Key::Digit(4) => Action::Speed(Speed::Max),
        Key::Digit(_) => return None,
        Key::G => Action::Tool(Tool::Dig(DigMode::Dig)),
        Key::Z => Action::Tool(Tool::Stockpile),
        Key::B => Action::Tool(Tool::Build(BuildKind::Wall)),
        Key::C => Action::Tool(Tool::Cancel),
        Key::I => Action::Tool(Tool::Inspect),
        Key::M => Action::CycleTool,
        Key::F => Action::Follow,
        Key::Escape => Action::Deselect,
        Key::F1 => Action::Overlay(Overlay::Jobs),
        Key::F2 => Action::Overlay(Overlay::Path),
        Key::F3 => Action::Overlay(Overlay::Regions),
        Key::F4 => Action::Overlay(Overlay::Heatmap),
        Key::F5 => Action::Save,
        Key::F6 => Action::Overlay(Overlay::Temperature),
        Key::F7 => Action::Overlay(Overlay::ActiveChunks),
        Key::F8 => Action::Overlay(Overlay::Support),
        Key::F9 => Action::Load,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selections_normalize_and_span_levels() {
        let selection = Selection {
            start: Pos3::new(10, 4, 14),
        };
        assert_eq!(
            selection.bounds(3, 9, 11),
            (Pos3::new(3, 4, 11), Pos3::new(10, 9, 14))
        );
        assert_eq!(
            selection.bounds(10, 4, 14),
            (Pos3::new(10, 4, 14), Pos3::new(10, 4, 14))
        );
    }

    #[test]
    fn tools_map_to_existing_commands() {
        let (low, high) = (Pos3::new(1, 2, 3), Pos3::new(2, 3, 4));
        assert_eq!(
            commands_for(Tool::Dig(DigMode::StairDown), low, high),
            [FortressCommand::DesignateDig {
                from: low,
                to: high,
                mode: DigMode::StairDown
            }]
        );
        assert_eq!(
            commands_for(Tool::Stockpile, low, high),
            [FortressCommand::DesignateStockpile {
                from: low,
                to: high
            }]
        );
        assert_eq!(
            commands_for(Tool::Cancel, low, high),
            [FortressCommand::CancelDesignations {
                from: low,
                to: high
            }]
        );
        let builds = commands_for(Tool::Build(BuildKind::Door), low, high);
        assert_eq!(builds.len(), 8);
        assert_eq!(
            builds[0],
            FortressCommand::Build {
                pos: low,
                kind: BuildKind::Door
            }
        );
        assert!(commands_for(Tool::Inspect, low, high).is_empty());
        let huge = commands_for(Tool::Build(BuildKind::Wall), low, Pos3::new(100, 100, 3));
        assert_eq!(huge.len(), MAX_BUILD_TILES);
    }

    #[test]
    fn keys_map_to_actions_and_tools_cycle() {
        assert_eq!(action_for_key(Key::PageUp), Some(Action::Level(1)));
        assert_eq!(action_for_key(Key::GreaterThan), Some(Action::Level(-1)));
        assert_eq!(
            action_for_key(Key::Digit(4)),
            Some(Action::Speed(Speed::Max))
        );
        assert_eq!(action_for_key(Key::Digit(9)), None);
        assert_eq!(action_for_key(Key::F5), Some(Action::Save));
        let mut tool = Tool::Dig(DigMode::Dig);
        for _ in 0..5 {
            tool = tool.cycle();
        }
        assert_eq!(tool, Tool::Dig(DigMode::Dig));
        assert_eq!(
            Tool::Build(BuildKind::Door).cycle(),
            Tool::Build(BuildKind::Wall)
        );
        assert_eq!(Speed::X5.ticks_per_second(20.0), Some(100.0));
        assert_eq!(Speed::Max.ticks_per_second(20.0), None);
    }
}
