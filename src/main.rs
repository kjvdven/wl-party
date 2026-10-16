#![forbid(unsafe_code)]

mod effect;
mod effects;
mod overlay;
mod particle;
mod script;
mod shape;

use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;

use clap::{Args, CommandFactory, Parser, Subcommand};

use crate::effect::{Build, Size};
use crate::script::Script;

/// Click-through confetti overlay for Wayland.
#[derive(Parser)]
#[command(version, about, args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    play: Play,
}

#[derive(Subcommand)]
enum Command {
    /// List built-in effects and scripts.
    List,
    /// Create a starter script in the effects directory.
    New { name: String },
    /// Compile a script and simulate it without an overlay.
    Check {
        /// Script name or path to a `.rhai` file.
        script: String,

        #[command(flatten)]
        sim: Sim,
    },
    /// Check the compositor, the effects directory and every script.
    Doctor,
    /// Play a random effect, from the given names or from everything in `list`.
    Random {
        /// Effects to choose from.
        names: Vec<String>,

        #[command(flatten)]
        show: Show,
    },
}

/// Options shared by playing and checking.
#[derive(Args)]
struct Sim {
    /// Number of particles.
    #[arg(short, long, default_value_t = DEFAULT_COUNT, value_parser = clap::value_parser!(u16).range(1..=5000))]
    count: u16,

    /// Seed for the random generator, so a run can be repeated exactly.
    #[arg(long)]
    seed: Option<u64>,
}

impl Sim {
    /// Seeds the random generator if asked and returns the particle count.
    fn prepare(&self) -> usize {
        if let Some(seed) = self.seed {
            fastrand::seed(seed);
        }
        self.count.into()
    }
}

#[derive(Args)]
struct Play {
    /// Effect to play, built-in or script.
    #[arg(default_value = "confetti")]
    effect: String,

    #[command(flatten)]
    show: Show,
}

/// Options for playing an effect on screen.
#[derive(Args)]
struct Show {
    #[command(flatten)]
    sim: Sim,

    /// Output to play on, e.g. `DP-1`, or `all`. Default: the compositor picks one.
    #[arg(short, long)]
    output: Option<String>,

    /// Print frame timings when the effect ends.
    #[arg(long)]
    stats: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::List) => {
            list();
            Ok(())
        }
        Some(Command::New { name }) => new(&name),
        Some(Command::Check { script, sim }) => check(&script, sim.prepare()),
        Some(Command::Doctor) => doctor(),
        Some(Command::Random { names, show }) => random(&names, &show),
        None => play(&cli.play.effect, &cli.play.show),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("wl-party: {err}");
            ExitCode::FAILURE
        }
    }
}

fn list() {
    for spec in effect::REGISTRY {
        println!("{:<10} {}", spec.name, spec.about);
    }
    for (name, about) in script::list() {
        println!("{name:<10} {about} (script)");
    }
    println!("{:<10} A random effect from this list", "random");
}

/// Every effect name `list` shows, built-ins first.
fn names() -> Vec<String> {
    let builtin = effect::REGISTRY.iter().map(|spec| spec.name.to_owned());
    builtin
        .chain(script::list().into_iter().map(|(name, _)| name))
        .collect()
}

fn new(name: &str) -> Result<(), Box<dyn Error>> {
    if effect::find(name).is_some() || Cli::command().find_subcommand(name).is_some() {
        return Err(format!("'{name}' is already a built-in effect or command").into());
    }
    let path = script::create(name)?;
    println!("created {}", path.display());
    println!("play it with: wl-party {name}");
    Ok(())
}

const DEFAULT_COUNT: u16 = 300;

/// Simulated screen and frame rate for `check`.
const CHECK_SIZE: Size = Size {
    width: 1920.0,
    height: 1080.0,
};
const CHECK_FPS: u32 = 60;
const CHECK_SECONDS: u32 = 60;

fn check(target: &str, count: usize) -> Result<(), Box<dyn Error>> {
    let path = if Path::new(target)
        .extension()
        .is_some_and(|ext| ext == "rhai")
    {
        PathBuf::from(target)
    } else {
        script::path(target).ok_or_else(|| format!("invalid script name '{target}'"))?
    };
    let report = simulate(&path, count).map_err(|err| format!("{}: {err}", path.display()))?;
    println!("ok: {report}");
    Ok(())
}

/// Runs a script headless until it ends; returns a one-line summary.
fn simulate(path: &Path, count: usize) -> Result<String, Box<dyn Error>> {
    let mut effect = Rc::new(Script::load(path)?).into_effect(CHECK_SIZE, count);
    let started = effect.particles();
    let mut peak = started;
    #[allow(clippy::cast_precision_loss)] // 1 / 60 is exact enough
    let dt = 1.0 / CHECK_FPS as f32;
    for frame in 1..=CHECK_FPS * CHECK_SECONDS {
        if !effect.update(dt)? {
            let seconds = f64::from(frame) / f64::from(CHECK_FPS);
            return Ok(format!(
                "{started} particles at start, peak {peak}, ends after {seconds:.1} s"
            ));
        }
        peak = peak.max(effect.particles());
    }
    Err(
        format!("still running after {CHECK_SECONDS} s, particles must expire or leave the screen")
            .into(),
    )
}

/// Prints what works and what doesn't; fails if anything is broken.
fn doctor() -> Result<(), Box<dyn Error>> {
    let mut healthy = true;
    let mut line = |ok: bool, text: &str| {
        healthy &= ok;
        println!("{} {text}", if ok { "ok " } else { "ERR" });
    };

    let display = env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    match overlay::probe() {
        Ok(interfaces) => {
            line(true, &format!("connected to {display}"));
            for (name, version) in interfaces {
                if version > 0 {
                    line(true, &format!("{name} v{version}"));
                } else {
                    line(
                        false,
                        &format!("{name} missing, the compositor can't show overlays"),
                    );
                }
            }
        }
        Err(err) => line(false, &format!("can't connect to {display}: {err}")),
    }

    let Some(dir) = script::dir() else {
        line(
            false,
            "no effects directory: neither $XDG_CONFIG_HOME nor $HOME is set",
        );
        return finish(healthy);
    };
    if !dir.is_dir() {
        println!(
            "--  {} doesn't exist yet, try `wl-party new <name>`",
            dir.display()
        );
        return finish(healthy);
    }
    let scripts = script::list();
    let plural = if scripts.len() == 1 { "" } else { "s" };
    line(
        true,
        &format!("{} ({} script{plural})", dir.display(), scripts.len()),
    );
    for (name, _) in scripts {
        let result = script::path(&name)
            .ok_or_else(|| "invalid name, use letters, digits, '-' and '_'".into())
            .and_then(|path| simulate(&path, DEFAULT_COUNT.into()));
        match result {
            Ok(report) => line(true, &format!("{name}: {report}")),
            Err(err) => line(false, &format!("{name}: {err}")),
        }
    }
    finish(healthy)
}

fn finish(healthy: bool) -> Result<(), Box<dyn Error>> {
    if healthy {
        Ok(())
    } else {
        Err("some checks failed".into())
    }
}

fn play(name: &str, show: &Show) -> Result<(), Box<dyn Error>> {
    let count = show.sim.prepare();
    overlay::run(build(name, count)?, show.output.as_deref(), show.stats)
}

fn random(names: &[String], show: &Show) -> Result<(), Box<dyn Error>> {
    // Seed before picking, so `--seed` repeats the choice too.
    let count = show.sim.prepare();
    let name = pick(names)?;
    overlay::run(build(&name, count)?, show.output.as_deref(), show.stats)
}

/// One of `names`, or of every effect when empty. Checks all names first, so a typo
/// fails every time instead of only when it happens to be picked.
fn pick(names: &[String]) -> Result<String, Box<dyn Error>> {
    let names = if names.is_empty() {
        self::names()
    } else {
        names.to_vec()
    };
    for name in &names {
        let known =
            effect::find(name).is_some() || script::path(name).is_some_and(|path| path.is_file());
        if !known {
            return Err(format!("unknown effect '{name}', see `wl-party list`").into());
        }
    }
    Ok(names[fastrand::usize(..names.len())].clone())
}

/// Built-in effects win over scripts with the same name.
fn build(name: &str, count: usize) -> Result<Build, Box<dyn Error>> {
    if let Some(spec) = effect::find(name) {
        let build = spec.build;
        return Ok(Box::new(move |size| build(size, count)));
    }
    let path = script::path(name)
        .filter(|path| path.is_file())
        .ok_or_else(|| format!("unknown effect '{name}', see `wl-party list`"))?;
    let script = Script::load(&path).map_err(|err| format!("{}: {err}", path.display()))?;
    let script = Rc::new(script);
    Ok(Box::new(move |size| {
        Rc::clone(&script).into_effect(size, count)
    }))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn temp_script(name: &str, source: &str) -> PathBuf {
        let path = env::temp_dir().join(format!("wl-party-{name}-{}.rhai", std::process::id()));
        fs::write(&path, source).unwrap();
        path
    }

    #[test]
    fn example_scripts_finish() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts");
        let mut checked = 0;
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            fastrand::seed(1);
            if let Err(err) = simulate(&path, DEFAULT_COUNT.into()) {
                panic!("{}: {err}", path.display());
            }
            checked += 1;
        }
        assert!(checked >= 4);
    }

    #[test]
    fn same_seed_same_run() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/snow.rhai");
        let run = || {
            fastrand::seed(7);
            simulate(&path, DEFAULT_COUNT.into()).unwrap()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn effects_that_never_end_fail() {
        let path = temp_script(
            "forever",
            "fn start(ctx) { ctx.gravity = 0.0; ctx.emit(#{ x: 100.0, y: 100.0 }); }",
        );
        let result = simulate(&path, 1);
        fs::remove_file(&path).unwrap();
        assert!(result.unwrap_err().to_string().contains("still running"));
    }

    #[test]
    fn errors_in_expire_are_reported() {
        let path = temp_script(
            "expire",
            r#"fn start(ctx) { ctx.emit(#{ fuse: 0.1 }); } fn expire(ctx, p) { ctx.emit(#{ color: "red" }); }"#,
        );
        let result = simulate(&path, 1);
        fs::remove_file(&path).unwrap();
        assert!(result.unwrap_err().to_string().contains("expire"));
    }

    #[test]
    fn pick_chooses_from_the_given_names() {
        let names = ["confetti".to_owned(), "rain".to_owned()];
        for _ in 0..20 {
            assert!(names.contains(&pick(&names).unwrap()));
        }
    }

    #[test]
    fn pick_from_everything_includes_built_ins() {
        let all = names();
        assert!(all.iter().any(|name| name == "confetti"));
        assert!(all.contains(&pick(&[]).unwrap()));
    }

    #[test]
    fn pick_rejects_unknown_names_even_if_not_picked() {
        let names = ["confetti".to_owned(), "nope".to_owned()];
        for _ in 0..5 {
            assert!(pick(&names).unwrap_err().to_string().contains("'nope'"));
        }
    }

    #[test]
    fn pick_repeats_with_the_same_seed() {
        let names: Vec<String> = ["confetti", "rain"].map(String::from).to_vec();
        let run = || {
            fastrand::seed(9);
            (0..10).map(|_| pick(&names).unwrap()).collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
