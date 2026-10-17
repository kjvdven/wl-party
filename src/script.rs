use std::cell::RefCell;
use std::error::Error;
use std::f32::consts::TAU;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::{env, fs};

use rhai::{
    AST, CallFnOptions, Dynamic, Engine, EvalAltResult, FLOAT, FuncArgs, INT, ImmutableString, Map,
    Scope,
};
use tiny_skia::{Path as Shape, PixmapMut};

use crate::effect::{Area, Effect, Size};
use crate::particle::{Particle, Swarm};
use crate::shape;

/// Hard cap on live particles, whatever a script asks for.
const MAX_PARTICLES: usize = 5000;
const MAX_TAGS: usize = 256;
const MAX_SHAPES: usize = 64;
/// Largest particle edge in pixels, so a script can't make every particle fill the screen.
const MAX_SIZE: f32 = 512.0;
/// Size for shaped particles without an explicit `size`.
const SHAPE_SIZE: f32 = 16.0;
/// Largest sideways sway in pixels.
const MAX_WOBBLE: f32 = 200.0;
const MAX_SCRIPT_BYTES: u64 = 64 * 1024;
const TEMPLATE: &str = include_str!("../scripts/template.rhai");

/// `$XDG_CONFIG_HOME/wl-party/effects`, falling back to `~/.config`.
pub fn dir() -> Option<PathBuf> {
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("wl-party").join("effects"))
}

/// Script path for `name`. Names are restricted so they can't escape the effects dir.
pub fn path(name: &str) -> Option<PathBuf> {
    let valid = !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    valid
        .then(dir)
        .flatten()
        .map(|dir| dir.join(format!("{name}.rhai")))
}

/// Writes the starter script as `name`; never overwrites an existing file.
pub fn create(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let path = path(name)
        .ok_or_else(|| format!("invalid name '{name}', use letters, digits, '-' and '_'"))?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|err| format!("{}: {err}", path.display()))?;
    file.write_all(TEMPLATE.as_bytes())?;
    Ok(path)
}

/// `(name, about)` for each script; `about` is the script's first `//` line.
pub fn list() -> Vec<(String, String)> {
    let Some(entries) = dir().and_then(|dir| fs::read_dir(dir).ok()) else {
        return Vec::new();
    };
    let mut scripts: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()? != "rhai" {
                return None;
            }
            let name = path.file_stem()?.to_str()?.to_owned();
            let source = fs::read_to_string(&path).ok()?;
            let about = source
                .lines()
                .next()
                .and_then(|line| line.strip_prefix("//"))
                .unwrap_or_default()
                .trim()
                .to_owned();
            Some((name, about))
        })
        .collect();
    scripts.sort();
    scripts
}

/// A compiled script, ready to become an effect.
pub struct Script {
    engine: Engine,
    ast: AST,
}

impl Script {
    pub fn load(path: &Path) -> Result<Self, Box<dyn Error>> {
        if fs::metadata(path)?.len() > MAX_SCRIPT_BYTES {
            return Err(format!("script is larger than {MAX_SCRIPT_BYTES} bytes").into());
        }
        Self::compile(&fs::read_to_string(path)?)
    }

    pub fn compile(source: &str) -> Result<Self, Box<dyn Error>> {
        let engine = engine();
        let ast = engine.compile(source)?;
        Ok(Self { engine, ast })
    }

    pub fn into_effect(self: Rc<Self>, size: Size, count: usize) -> Box<dyn Effect> {
        let has = |name: &str, params: usize| {
            self.ast
                .iter_functions()
                .any(|func| func.name == name && func.params.len() == params)
        };
        let has_expire = has("expire", 2);
        let has_start = has("start", 1);
        let ctx = Ctx {
            size,
            count: INT::try_from(count).unwrap_or(INT::MAX),
            state: Rc::new(RefCell::new(State {
                spawned: Vec::with_capacity(count),
                tags: vec![ImmutableString::new()],
                shape_keys: vec!["rect".into()],
                new_shapes: Vec::new(),
                gravity: size.height * 0.6,
                drag: 1.5,
            })),
        };
        let mut effect = Running {
            script: self,
            swarm: Swarm::new(count, 0.0, 0.0),
            ctx,
            has_expire,
            scope: Scope::new(),
            error: None,
        };
        // Top-level code runs once, so its constants are in scope for every hook.
        if let Err(err) = effect
            .script
            .engine
            .run_ast_with_scope(&mut effect.scope, &effect.script.ast)
        {
            effect.error = Some(err.to_string());
        }
        if has_start && effect.error.is_none() {
            let ctx = effect.ctx.clone();
            effect.call("start", (ctx,));
        }
        effect.flush();
        Box::new(effect)
    }
}

/// Sandbox: no modules, no `eval`, and every call is bounded in time and memory.
fn engine() -> Engine {
    let mut engine = Engine::new();
    engine
        .set_max_operations(200_000)
        .set_max_call_levels(32)
        .set_max_expr_depths(64, 32)
        .set_max_string_size(1024)
        .set_max_array_size(MAX_PARTICLES)
        .set_max_map_size(64)
        .disable_symbol("eval");
    engine
        .register_type_with_name::<Ctx>("Ctx")
        .register_get("width", |ctx: &mut Ctx| ctx.size.width)
        .register_get("height", |ctx: &mut Ctx| ctx.size.height)
        .register_get("count", |ctx: &mut Ctx| ctx.count)
        .register_get_set(
            "gravity",
            |ctx: &mut Ctx| ctx.state.borrow().gravity,
            |ctx: &mut Ctx, value: FLOAT| ctx.state.borrow_mut().gravity = value,
        )
        .register_get_set(
            "drag",
            |ctx: &mut Ctx| ctx.state.borrow().drag,
            |ctx: &mut Ctx, value: FLOAT| ctx.state.borrow_mut().drag = value,
        )
        .register_fn("emit", Ctx::emit)
        .register_fn("burst", |ctx: &mut Ctx, x: FLOAT, y: FLOAT, count: INT| {
            ctx.burst(x, y, count, None);
        })
        .register_fn(
            "burst",
            |ctx: &mut Ctx, x: FLOAT, y: FLOAT, count: INT, color: ImmutableString| {
                ctx.burst(x, y, count, Some(parse_color(&color)?));
                Ok::<_, Box<EvalAltResult>>(())
            },
        )
        .register_fn("rand", fastrand::f32);
    engine
}

/// Shared between the effect and the `ctx` handles a script receives.
struct State {
    spawned: Vec<Particle>,
    /// Tag strings, interned so particles stay `Copy`; index 0 is the empty tag.
    tags: Vec<ImmutableString>,
    /// Shape strings, index-aligned with `Swarm::shapes`; index 0 is the confetti rectangle.
    shape_keys: Vec<ImmutableString>,
    /// Shapes parsed since the last flush, still to be handed to the swarm.
    new_shapes: Vec<Shape>,
    gravity: f32,
    drag: f32,
}

impl State {
    fn push(&mut self, particle: Particle) {
        if self.spawned.len() < MAX_PARTICLES {
            self.spawned.push(particle);
        }
    }

    fn intern(&mut self, tag: ImmutableString) -> u16 {
        let index = match self.tags.iter().position(|known| *known == tag) {
            Some(index) => index,
            None if self.tags.len() < MAX_TAGS => {
                self.tags.push(tag);
                self.tags.len() - 1
            }
            None => 0,
        };
        u16::try_from(index).unwrap_or(0)
    }

    /// `"rect"`, `"circle"` or SVG path data, parsed once per distinct string.
    fn shape(&mut self, key: ImmutableString) -> Result<u16, Box<EvalAltResult>> {
        let index = match self.shape_keys.iter().position(|known| *known == key) {
            Some(index) => index,
            None if self.shape_keys.len() < MAX_SHAPES => {
                let path = if key == "circle" {
                    shape::circle()
                } else {
                    shape::parse(&key).ok_or_else(|| format!("invalid shape '{key}'"))?
                };
                self.shape_keys.push(key);
                self.new_shapes.push(path);
                self.shape_keys.len() - 1
            }
            None => return Err(format!("more than {MAX_SHAPES} shapes").into()),
        };
        Ok(u16::try_from(index).unwrap_or(0))
    }
}

/// The `ctx` object scripts see.
#[derive(Clone)]
struct Ctx {
    size: Size,
    count: INT,
    state: Rc<RefCell<State>>,
}

impl Ctx {
    // Rhai passes every argument after the first by value.
    #[allow(clippy::needless_pass_by_value)]
    fn emit(&mut self, spec: Map) -> Result<(), Box<EvalAltResult>> {
        let number = |key: &str| spec.get(key).and_then(to_f32);
        let text = |key: &str| {
            spec.get(key)
                .and_then(|value| value.clone().into_immutable_string().ok())
        };
        let mut particle = Particle::new(
            number("x").unwrap_or(0.0),
            number("y").unwrap_or(0.0),
            number("vx").unwrap_or(0.0),
            number("vy").unwrap_or(0.0),
        );
        particle.fuse = number("fuse").unwrap_or(f32::INFINITY);
        let shape = text("shape");
        if let Some(size) = number("size").or(shape.as_ref().map(|_| SHAPE_SIZE)) {
            particle.set_size(size.clamp(1.0, MAX_SIZE));
        }
        if let Some(wobble) = number("wobble") {
            particle.set_wobble(wobble.clamp(0.0, MAX_WOBBLE));
        }
        let flag = |key: &str| spec.get(key).and_then(|value| value.as_bool().ok());
        if flag("spin") == Some(false) {
            particle.stop_spin();
        }
        particle.gloss = flag("gloss") == Some(true);
        if let Some(color) = text("color") {
            particle.set_color(parse_color(&color)?);
        }
        if let Some(shadow) = text("shadow") {
            particle.set_shadow(parse_color(&shadow)?);
        }
        let mut state = self.state.borrow_mut();
        if let Some(shape) = shape {
            particle.shape = state.shape(shape)?;
        }
        if let Some(tag) = text("tag") {
            particle.tag = state.intern(tag);
        }
        state.push(particle);
        Ok(())
    }

    /// Sparks flying out in all directions from `(x, y)`, fading out within ~1.6 s.
    fn burst(&mut self, x: FLOAT, y: FLOAT, count: INT, color: Option<[u8; 4]>) {
        let count = usize::try_from(count).unwrap_or(0).min(MAX_PARTICLES);
        let speed = self.size.height * 0.35;
        let mut state = self.state.borrow_mut();
        for _ in 0..count {
            let (sin, cos) = (fastrand::f32() * TAU).sin_cos();
            let velocity = speed * (0.3 + fastrand::f32() * 0.7);
            let mut particle = Particle::new(x, y, cos * velocity, sin * velocity);
            particle.fuse = 0.8 + fastrand::f32() * 0.8;
            if let Some(color) = color {
                particle.set_color(color);
            }
            state.push(particle);
        }
    }
}

/// `#rrggbb` or `#rrggbbaa` to `[r, g, b, a]`.
fn parse_color(color: &str) -> Result<[u8; 4], Box<EvalAltResult>> {
    color
        .strip_prefix('#')
        .filter(|hex| {
            matches!(hex.len(), 6 | 8) && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .and_then(|hex| {
            let value = u32::from_str_radix(hex, 16).ok()?;
            Some(if hex.len() == 6 {
                value << 8 | 0xff
            } else {
                value
            })
        })
        .map(u32::to_be_bytes)
        .ok_or_else(|| format!("invalid color '{color}', expected #rrggbb or #rrggbbaa").into())
}

fn to_f32(value: &Dynamic) -> Option<f32> {
    // Scripts may write `x: 10` instead of `x: 10.0`; precision loss is irrelevant at pixel scale.
    #[allow(clippy::cast_precision_loss)]
    value
        .as_float()
        .ok()
        .or_else(|| value.as_int().ok().map(|int| int as f32))
}

struct Running {
    script: Rc<Script>,
    swarm: Swarm,
    ctx: Ctx,
    has_expire: bool,
    scope: Scope<'static>,
    /// First script error; reported by the next `update`.
    error: Option<String>,
}

impl Running {
    /// Calls a script function; a script error ends the effect.
    fn call(&mut self, name: &str, args: impl FuncArgs) {
        let options = CallFnOptions::new().eval_ast(false);
        let result = self.script.engine.call_fn_with_options::<Dynamic>(
            options,
            &mut self.scope,
            &self.script.ast,
            name,
            args,
        );
        if let Err(err) = result {
            self.error = Some(format!("{name}: {err}"));
        }
    }

    /// Moves particles the script spawned into the swarm, within the global cap.
    fn flush(&mut self) {
        let mut state = self.ctx.state.borrow_mut();
        self.swarm.shapes.append(&mut state.new_shapes);
        let room = MAX_PARTICLES.saturating_sub(self.swarm.particles.len());
        state.spawned.truncate(room);
        self.swarm.particles.append(&mut state.spawned);
        self.swarm.gravity = state.gravity;
        self.swarm.drag = state.drag;
    }

    fn to_map(&self, particle: &Particle) -> Map {
        let tag = self.ctx.state.borrow().tags[usize::from(particle.tag)].clone();
        Map::from([
            ("x".into(), particle.x.into()),
            ("y".into(), particle.y.into()),
            ("vx".into(), particle.vx.into()),
            ("vy".into(), particle.vy.into()),
            ("tag".into(), tag.into()),
        ])
    }
}

impl Effect for Running {
    fn update(&mut self, dt: f32) -> Result<bool, Box<dyn Error>> {
        self.swarm.update(dt, self.ctx.size);
        if self.has_expire {
            let expired = std::mem::take(&mut self.swarm.expired);
            for particle in &expired {
                if self.error.is_some() {
                    break;
                }
                let args = (self.ctx.clone(), self.to_map(particle));
                self.call("expire", args);
            }
            self.swarm.expired = expired;
        }
        self.flush();
        match self.error.take() {
            Some(err) => Err(err.into()),
            None => Ok(!self.swarm.is_empty()),
        }
    }

    fn draw(&self, canvas: &mut PixmapMut) -> Area {
        self.swarm.draw(canvas)
    }

    fn particles(&self) -> usize {
        self.swarm.particles.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: Size = Size {
        width: 800.0,
        height: 600.0,
    };

    fn run(source: &str) -> Result<Box<dyn Effect>, Box<dyn Error>> {
        let mut effect = Rc::new(Script::compile(source)?).into_effect(SIZE, 300);
        effect.update(1.0 / 60.0)?;
        Ok(effect)
    }

    fn error(source: &str) -> String {
        run(source).err().expect("script should fail").to_string()
    }

    #[test]
    fn path_rejects_names_that_escape_the_effects_dir() {
        for name in ["", "../x", "a/b", "a.b", "..", "snow flake", "sneeuw❄"] {
            assert!(path(name).is_none(), "{name:?} should be rejected");
        }
        let path = path("snow-2_x").expect("valid name");
        assert!(path.ends_with("wl-party/effects/snow-2_x.rhai"));
    }

    #[test]
    fn parse_color_accepts_rrggbb_and_rrggbbaa() {
        assert_eq!(parse_color("#ff4d6d").unwrap(), [0xff, 0x4d, 0x6d, 0xff]);
        assert_eq!(parse_color("#FFFFFF").unwrap(), [0xff, 0xff, 0xff, 0xff]);
        assert_eq!(parse_color("#ff4d6d80").unwrap(), [0xff, 0x4d, 0x6d, 0x80]);
        for bad in [
            "red",
            "#fff",
            "ff4d6d",
            "#gggggg",
            "#+fffff",
            "#ff4d6d0",
            "#ff4d6d000",
            "#ffé4d",
        ] {
            assert!(parse_color(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn particles_are_capped() {
        let effect =
            run("fn start(ctx) { for i in 0..10000 { ctx.emit(#{ y: 10.0 }); } }").unwrap();
        assert_eq!(effect.particles(), MAX_PARTICLES);
    }

    #[test]
    fn bursts_are_capped() {
        let effect = run("fn start(ctx) { ctx.burst(10.0, 10.0, 1000000); }").unwrap();
        assert_eq!(effect.particles(), MAX_PARTICLES);
    }

    #[test]
    fn shapes_are_capped() {
        let source = r#"fn start(ctx) { for i in 1..100 { ctx.emit(#{ shape: "M0 0 L" + i + " 1 L0 1 Z" }); } }"#;
        assert!(error(source).contains("more than 64 shapes"));
    }

    #[test]
    fn tags_beyond_the_cap_fall_back_to_the_empty_tag() {
        let mut state = State {
            spawned: Vec::new(),
            tags: vec![ImmutableString::new()],
            shape_keys: Vec::new(),
            new_shapes: Vec::new(),
            gravity: 0.0,
            drag: 0.0,
        };
        for i in 1..MAX_TAGS {
            assert_eq!(usize::from(state.intern(i.to_string().into())), i);
        }
        assert_eq!(state.intern("one too many".into()), 0);
        assert_eq!(state.intern("1".into()), 1);
    }

    #[test]
    fn endless_loops_are_stopped() {
        assert!(error("fn start(ctx) { loop {} }").contains("Too many operations"));
    }

    #[test]
    fn eval_is_disabled() {
        assert!(Script::compile(r#"fn start(ctx) { eval("1") }"#).is_err());
    }

    #[test]
    fn bad_input_reports_the_problem() {
        assert!(
            error(r#"fn start(ctx) { ctx.emit(#{ color: "red" }); }"#).contains("invalid color")
        );
        assert!(
            error(r#"fn start(ctx) { ctx.emit(#{ shape: "nonsense" }); }"#)
                .contains("invalid shape")
        );
        assert!(
            error(r#"fn start(ctx) { ctx.emit(#{ shadow: "grey" }); }"#).contains("invalid color")
        );
    }

    #[test]
    fn hooks_see_top_level_constants() {
        let source = r#"
            const SPEED = 50.0;
            fn start(ctx) { ctx.emit(#{ y: 10.0, vy: SPEED, fuse: 0.01, tag: "a" }); }
            fn expire(ctx, p) { ctx.emit(#{ y: p.y, vy: SPEED }); }
        "#;
        let mut effect = run(source).unwrap();
        effect.update(1.0 / 60.0).unwrap();
        assert_eq!(effect.particles(), 1);
    }

    #[test]
    fn oversized_scripts_are_refused() {
        let path = env::temp_dir().join(format!("wl-party-test-{}.rhai", std::process::id()));
        fs::write(&path, "// x\n".repeat(20_000)).unwrap();
        let result = Script::load(&path);
        fs::remove_file(&path).unwrap();
        assert!(result.err().unwrap().to_string().contains("larger than"));
    }
}
