//! Graphical fortress viewer and sandbox. See README.md for controls.

mod render;

use std::{path::PathBuf, sync::Arc, time::SystemTime};

use fortress_sim::FortressConfig;
use fortress_viewer::{
    camera::Camera,
    frame::{Frame, Mailbox, Mirror},
    session::{Control, RunSummary, Session, SessionConfig, spawn},
    tools::{Action, Key, Selection, Speed, Tool, action_for_key, commands_for},
};
use macroquad::{
    prelude::*,
    ui::{hash, root_ui},
};
use render::{Materials, Overlays, draw_panel, draw_world};
use sim_core::space::Pos3;

#[derive(Default)]
struct Args {
    seed: Option<u64>,
    config: Option<PathBuf>,
    scenario: Option<PathBuf>,
    load: Option<PathBuf>,
    speed: Option<Speed>,
    base_tps: Option<f64>,
    level: Option<i32>,
    session_dir: Option<PathBuf>,
    screenshot: Option<(u64, PathBuf)>,
    exit_after_screenshot: bool,
    exit_at_tick: Option<u64>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args::default();
    let mut iter = std::env::args().skip(1);
    let value = |iter: &mut dyn Iterator<Item = String>, name: &str| {
        iter.next().ok_or_else(|| format!("{name} needs a value"))
    };
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--seed" => {
                args.seed = Some(
                    value(&mut iter, &arg)?
                        .parse()
                        .map_err(|e| format!("--seed: {e}"))?,
                )
            }
            "--config" => args.config = Some(value(&mut iter, &arg)?.into()),
            "--scenario" | "--commands" => args.scenario = Some(value(&mut iter, &arg)?.into()),
            "--load" => args.load = Some(value(&mut iter, &arg)?.into()),
            "--base-tps" => {
                args.base_tps = Some(
                    value(&mut iter, &arg)?
                        .parse()
                        .map_err(|e| format!("--base-tps: {e}"))?,
                );
            }
            "--level" => {
                args.level = Some(
                    value(&mut iter, &arg)?
                        .parse()
                        .map_err(|e| format!("--level: {e}"))?,
                )
            }
            "--session-dir" => args.session_dir = Some(value(&mut iter, &arg)?.into()),
            "--speed" => {
                args.speed = Some(match value(&mut iter, &arg)?.as_str() {
                    "pause" => Speed::Paused,
                    "1" => Speed::X1,
                    "2" => Speed::X2,
                    "5" => Speed::X5,
                    "max" => Speed::Max,
                    other => return Err(format!("unknown speed {other}")),
                });
            }
            "--screenshot-at-tick" => {
                let tick = value(&mut iter, &arg)?
                    .parse()
                    .map_err(|e| format!("--screenshot-at-tick: {e}"))?;
                args.screenshot = Some((tick, value(&mut iter, &arg)?.into()));
            }
            "--exit-after-screenshot" => args.exit_after_screenshot = true,
            "--exit-at-tick" => {
                args.exit_at_tick = Some(
                    value(&mut iter, &arg)?
                        .parse()
                        .map_err(|e| format!("--exit-at-tick: {e}"))?,
                );
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(args)
}

fn conf() -> Conf {
    Conf {
        window_title: "Terrarium Fortress".into(),
        window_width: 1280,
        window_height: 800,
        ..Default::default()
    }
}

fn timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!("session-{seconds}")
}

fn session_config(args: &Args, seed: u64) -> Result<SessionConfig, String> {
    let dir = args
        .session_dir
        .clone()
        .unwrap_or_else(|| "sessions".into());
    let mut config = SessionConfig::new(seed, dir, timestamp());
    if let Some(path) = &args.config {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        config.config = toml_config(&text)?;
    }
    config.scenario = args.scenario.clone();
    config.load = args.load.clone();
    Ok(config)
}

fn toml_config(text: &str) -> Result<FortressConfig, String> {
    sim_io::config::ConfigText::Toml(text.into())
        .decode()
        .map_err(|e| e.to_string())
}

/// Start screen: new game with seed and optional scenario, or load a snapshot.
async fn start_screen(args: &mut Args) {
    let mut seed = "42".to_owned();
    let mut scenario = String::new();
    let mut load = String::new();
    loop {
        clear_background(Color::from_rgba(20, 22, 30, 255));
        draw_text("Terrarium Fortress", 40.0, 60.0, 40.0, WHITE);
        let mut chosen = false;
        root_ui().window(hash!(), vec2(40.0, 90.0), vec2(520.0, 230.0), |ui| {
            ui.label(None, "Neues Spiel");
            ui.input_text(hash!(), "Seed", &mut seed);
            ui.input_text(hash!(), "Szenario (optional)", &mut scenario);
            if ui.button(None, "Neues Spiel starten") {
                args.seed = seed.trim().parse().ok().or(Some(42));
                args.scenario = (!scenario.trim().is_empty()).then(|| scenario.trim().into());
                chosen = true;
            }
            ui.separator();
            ui.label(None, "Spielstand laden");
            ui.input_text(hash!(), "Snapshot-Datei", &mut load);
            if ui.button(None, "Snapshot laden") && !load.trim().is_empty() {
                args.load = Some(load.trim().into());
                chosen = true;
            }
        });
        if chosen {
            return;
        }
        next_frame().await;
    }
}

const KEYS: [(KeyCode, Key); 28] = [
    (KeyCode::W, Key::W),
    (KeyCode::A, Key::A),
    (KeyCode::S, Key::S),
    (KeyCode::D, Key::D),
    (KeyCode::Up, Key::Up),
    (KeyCode::Down, Key::Down),
    (KeyCode::Left, Key::Left),
    (KeyCode::Right, Key::Right),
    (KeyCode::PageUp, Key::PageUp),
    (KeyCode::PageDown, Key::PageDown),
    (KeyCode::Space, Key::Space),
    (KeyCode::N, Key::N),
    (KeyCode::Key0, Key::Digit(0)),
    (KeyCode::Key1, Key::Digit(1)),
    (KeyCode::Key2, Key::Digit(2)),
    (KeyCode::Key3, Key::Digit(3)),
    (KeyCode::Key4, Key::Digit(4)),
    (KeyCode::G, Key::G),
    (KeyCode::Z, Key::Z),
    (KeyCode::B, Key::B),
    (KeyCode::C, Key::C),
    (KeyCode::I, Key::I),
    (KeyCode::M, Key::M),
    (KeyCode::F, Key::F),
    (KeyCode::Escape, Key::Escape),
    (KeyCode::F1, Key::F1),
    (KeyCode::F5, Key::F5),
    (KeyCode::F9, Key::F9),
];
const MORE_KEYS: [(KeyCode, Key); 3] = [
    (KeyCode::F2, Key::F2),
    (KeyCode::F3, Key::F3),
    (KeyCode::F4, Key::F4),
];

fn print_summary(summary: &RunSummary, headless_ticks: u64) {
    let tps = if summary.step_seconds > 0.0 {
        summary.ticks as f64 / summary.step_seconds
    } else {
        0.0
    };
    let wall = if summary.wall_seconds > 0.0 {
        summary.ticks as f64 / summary.wall_seconds
    } else {
        0.0
    };
    let sim = if summary.sim_seconds > 0.0 {
        summary.ticks as f64 / summary.sim_seconds
    } else {
        0.0
    };
    println!(
        "viewer: {} ticks (UI at tick {headless_ticks}), sim TPS {sim:.1}, tick loop TPS {tps:.1}, wall TPS {wall:.1}, step p99 {:.2} µs",
        summary.ticks, summary.p99_us
    );
    for replay in &summary.replays {
        println!("replay {}", replay.display());
    }
    if let Some(error) = &summary.error {
        eprintln!("Fehler: {error}");
    }
}

#[macroquad::main(conf)]
async fn main() {
    let mut args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("Fehler: {message}");
            return;
        }
    };
    if args.seed.is_none() && args.load.is_none() && args.scenario.is_none() {
        start_screen(&mut args).await;
    }
    let config = match session_config(&args, args.seed.unwrap_or(42)) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("Fehler: {message}");
            return;
        }
    };
    let session = match Session::start(config) {
        Ok(session) => session,
        Err(error) => {
            eprintln!("Fehler: {error}");
            return;
        }
    };
    let view = session.sim().view();
    let dims = view.dims();
    let materials = Materials {
        names: (0..view.material_count() as u16)
            .map(|m| view.material_name(m).unwrap_or("").to_owned())
            .collect(),
        ore: (0..view.material_count() as u16)
            .map(|m| view.is_ore(m))
            .collect(),
    };
    let mut mirror = Mirror::new(dims, view.generated_tiles().unwrap_or_default());
    let mut residents = Vec::new();
    view.residents(&mut residents);
    let start_level = args
        .level
        .or_else(|| residents.first().map(|r| r.pos.z))
        .unwrap_or(dims.2 / 2);
    let mut camera = Camera::new(dims.0 as f32 / 2.0, dims.1 as f32 / 2.0, start_level);

    let mailbox = Arc::new(Mailbox::default());
    let speed = args.speed.unwrap_or(Speed::X1);
    let (control, handle) = spawn(
        session,
        Arc::clone(&mailbox),
        args.base_tps.unwrap_or(20.0),
        speed,
    );
    if let Some(tick) = args.exit_at_tick {
        let _ = control.send(Control::StopAt(tick));
    }
    let mut frame = Frame::default();
    let mut paused_speed = if speed == Speed::Paused {
        Speed::X1
    } else {
        speed
    };
    let mut current_speed = speed;
    let mut tool = Tool::Inspect;
    let mut selection: Option<Selection> = None;
    let mut selected: Option<u64> = None;
    let mut following = false;
    let mut overlays = Overlays {
        jobs: true,
        path: true,
        regions: false,
        heatmap: false,
    };
    let mut description: Option<Vec<(String, String)>> = None;
    let mut inspected_tile: Option<Pos3> = None;
    let mut messages: Vec<String> = Vec::new();
    let mut last_mouse = mouse_position();
    let mut screenshot_done = false;
    let save_path = PathBuf::from("sessions/quicksave.snap");
    let mut fitted = false;
    let mut frames_drawn = 0u32;
    let mut paused_at: Option<u32> = None;
    if let Some((tick, _)) = &args.screenshot {
        let _ = control.send(Control::PauseAt(*tick));
    }

    loop {
        frame.clear_dirty();
        mailbox.take(&mut frame);
        mirror.apply(&frame);
        if frame.description.is_some() {
            description = frame.description.take();
        }
        messages.append(&mut frame.messages);
        if messages.len() > 6 {
            let excess = messages.len() - 6;
            messages.drain(..excess);
        }
        let screen = (screen_width(), screen_height());
        if !fitted {
            // The window size settles only after the first frames; fit until the user zooms.
            camera.tile_px = (screen.1 / dims.1 as f32).clamp(4.0, 64.0);
        }
        frames_drawn += 1;
        let ui_hovered = root_ui().is_mouse_over(vec2(mouse_position().0, mouse_position().1));

        // Keyboard.
        let mut actions = Vec::new();
        for (code, key) in KEYS.iter().chain(MORE_KEYS.iter()) {
            let pan = matches!(
                key,
                Key::W | Key::A | Key::S | Key::D | Key::Up | Key::Down | Key::Left | Key::Right
            );
            if ((pan && is_key_down(*code)) || (!pan && is_key_pressed(*code)))
                && let Some(action) = action_for_key(*key)
            {
                actions.push(action);
            }
        }
        while let Some(ch) = get_char_pressed() {
            let key = match ch {
                '<' => Some(Key::LessThan),
                '>' => Some(Key::GreaterThan),
                _ => None,
            };
            if let Some(action) = key.and_then(action_for_key) {
                actions.push(action);
            }
        }
        for action in actions {
            match action {
                Action::Pan(dx, dy) => {
                    following = false;
                    let speed = 30.0 * get_frame_time();
                    camera.pan_tiles(dx * speed, dy * speed);
                }
                Action::Level(delta) => {
                    following = false;
                    camera.change_level(delta, dims.2);
                }
                Action::TogglePause => {
                    current_speed = if current_speed == Speed::Paused {
                        paused_speed
                    } else {
                        Speed::Paused
                    };
                    let _ = control.send(Control::Speed(current_speed));
                }
                Action::Step => {
                    current_speed = Speed::Paused;
                    let _ = control.send(Control::Speed(Speed::Paused));
                    let _ = control.send(Control::Step);
                }
                Action::Speed(new) => {
                    current_speed = new;
                    if new != Speed::Paused {
                        paused_speed = new;
                    }
                    let _ = control.send(Control::Speed(new));
                }
                Action::Tool(new) => tool = new,
                Action::CycleTool => tool = tool.cycle(),
                Action::Follow => following = selected.is_some(),
                Action::Deselect => {
                    selection = None;
                    selected = None;
                    description = None;
                    inspected_tile = None;
                    let _ = control.send(Control::Select(None));
                }
                Action::Overlay(overlay) => {
                    overlays.toggle(overlay);
                    let _ =
                        control.send(Control::Regions(overlays.regions.then_some(camera.level)));
                }
                Action::Save => {
                    let _ = std::fs::create_dir_all("sessions");
                    let _ = control.send(Control::Save(save_path.clone()));
                }
                Action::Load => {
                    let _ = control.send(Control::Load(save_path.clone()));
                }
            }
        }
        if overlays.regions {
            let _ = control.send(Control::Regions(Some(camera.level)));
        }

        // Mouse.
        let mouse = mouse_position();
        if !ui_hovered {
            let (_, wheel) = mouse_wheel();
            if wheel != 0.0 {
                fitted = true;
                camera.zoom_at(
                    if wheel > 0.0 { 1.15 } else { 1.0 / 1.15 },
                    mouse.0,
                    mouse.1,
                    screen,
                );
            }
            if is_mouse_button_down(MouseButton::Right) || is_mouse_button_down(MouseButton::Middle)
            {
                following = false;
                camera.pan_pixels(mouse.0 - last_mouse.0, mouse.1 - last_mouse.1);
            }
            let (tx, ty) = camera.screen_to_tile(mouse.0, mouse.1, screen);
            if is_mouse_button_pressed(MouseButton::Left) {
                selection = Some(Selection {
                    start: Pos3::new(tx, ty, camera.level),
                });
            }
            if is_mouse_button_released(MouseButton::Left)
                && let Some(current) = selection.take()
            {
                let (low, high) = current.bounds(tx, ty, camera.level);
                if tool == Tool::Inspect {
                    let clicked = Pos3::new(tx, ty, camera.level);
                    let resident = frame
                        .residents
                        .iter()
                        .find(|r| r.pos == clicked)
                        .map(|r| r.id);
                    let item = frame.items.iter().find(|i| i.pos == clicked).map(|i| i.id);
                    selected = resident;
                    let _ = control.send(Control::Select(resident));
                    let _ = control.send(Control::Inspect(resident.or(item)));
                    description = None;
                    inspected_tile = Some(clicked);
                } else {
                    let commands = commands_for(tool, low, high);
                    if !commands.is_empty() {
                        let _ = control.send(Control::Commands(commands));
                    }
                }
            }
        }
        last_mouse = mouse;
        if following && let Some(resident) = frame.residents.iter().find(|r| Some(r.id) == selected)
        {
            camera.follow(resident.pos);
        }

        // Drawing.
        clear_background(BLACK);
        let preview = selection.map(|s| {
            let (tx, ty) = camera.screen_to_tile(mouse.0, mouse.1, screen);
            s.bounds(tx, ty, camera.level)
        });
        draw_world(
            &mirror, &frame, &camera, &materials, &overlays, selected, preview,
        );
        let stats = &frame.stats;
        let mut hud = vec![
            format!(
                "Tick {} | {} | Sim-TPS {:.0} | p99 {:.0} µs (1000 Ticks)",
                stats.tick, stats.speed, stats.tps, stats.p99_us
            ),
            format!(
                "Ebene z={} | Bewohner {} | Gegenstände {} | offene Jobs {}",
                camera.level, stats.residents, stats.items, stats.open_jobs
            ),
            format!(
                "Werkzeug: {} | Overlays: Jobs {} Pfad {} Regionen {} Heatmap {}",
                tool.label(),
                overlays.jobs as u8,
                overlays.path as u8,
                overlays.regions as u8,
                overlays.heatmap as u8
            ),
        ];
        hud.extend(messages.iter().cloned());
        draw_panel(&hud, 8.0, 8.0);
        if let Some(p) = inspected_tile {
            let mut lines = vec![format!("Kachel {},{},{}", p.x, p.y, p.z)];
            if let Some(tile) = mirror.get(p) {
                lines.push(format!(
                    "{:?} Boden {} Material {} {}",
                    tile.kind,
                    tile.floor,
                    tile.material,
                    materials
                        .names
                        .get(usize::from(tile.material))
                        .map_or("", String::as_str)
                ));
            }
            if let Some(fields) = &description {
                lines.extend(fields.iter().map(|(k, v)| format!("{k}: {v}")));
            }
            if selected.is_some() {
                lines.push(format!("Pfad: {} Knoten", frame.selected_path.len()));
            }
            draw_panel(&lines, screen.0 - 360.0, 8.0);
        }

        if let Some((tick, path)) = &args.screenshot
            && !screenshot_done
            && frame.stats.tick >= *tick
            && frames_drawn >= 30
            && paused_at.get_or_insert(frames_drawn).saturating_add(10) <= frames_drawn
        {
            get_screen_data().export_png(&path.to_string_lossy());
            println!("screenshot {} at tick {}", path.display(), frame.stats.tick);
            screenshot_done = true;
            if args.exit_after_screenshot {
                let _ = control.send(Control::Quit);
                break;
            }
        }
        if frame.finished {
            break;
        }
        if is_key_pressed(KeyCode::Q)
            && (is_key_down(KeyCode::LeftControl) || is_key_down(KeyCode::RightControl))
        {
            let _ = control.send(Control::Quit);
            break;
        }
        next_frame().await;
    }
    let _ = control.send(Control::Quit);
    match handle.join() {
        Ok(summary) => print_summary(&summary, frame.stats.tick),
        Err(_) => eprintln!("Fehler: Simulations-Thread abgebrochen"),
    }
}
