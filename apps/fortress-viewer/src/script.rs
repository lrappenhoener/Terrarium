//! Scripted UI actions for headless tests: the same tool and selection logic as
//! the window, without graphics.

use sim_core::space::Pos3;

use crate::{
    session::{Session, ViewerError},
    tools::{Names, Selection, Tool, commands_for},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptAction {
    /// Drag a selection from one tile and level to another with a tool.
    Drag { tool: Tool, from: Pos3, to: Pos3 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptStep {
    pub tick: u64,
    pub action: ScriptAction,
}

/// Runs a session for `ticks` ticks and performs each action before its tick.
pub fn run_script(
    session: &mut Session,
    script: &[ScriptStep],
    ticks: u64,
) -> Result<(), ViewerError> {
    let names = Names::from_content(session.sim().view().content());
    let end = session.sim().view().tick() + ticks;
    while session.sim().view().tick() < end {
        let tick = session.sim().view().tick();
        for step in script.iter().filter(|step| step.tick == tick) {
            let ScriptAction::Drag { tool, from, to } = step.action;
            let (low, high) = Selection { start: from }.bounds(to.x, to.y, to.z);
            session.queue(commands_for(tool, low, high, &names));
        }
        session.step()?;
    }
    Ok(())
}
