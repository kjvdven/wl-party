use std::error::Error;
use std::time::{Duration, Instant};

use smithay_client_toolkit::compositor::{
    CompositorHandler, CompositorState, FrameCallbackData, Region,
};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_registry, registry_handlers};
use tiny_skia::PixmapMut;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_output, wl_shm, wl_surface};
use wayland_client::{Connection, QueueHandle};

use crate::effect::{Area, Build, Effect, Size};

/// Longest step we simulate, so a stalled frame doesn't teleport particles.
const MAX_DT: f32 = 1.0 / 20.0;
/// Triple buffering is plenty; if all are still held by the compositor we skip a frame.
const MAX_BUFFERS: usize = 3;

/// Interfaces the overlay needs from the compositor.
const REQUIRED: [&str; 3] = ["wl_compositor", "wl_shm", "zwlr_layer_shell_v1"];

/// Each required interface with the highest version the compositor offers; 0 if missing.
pub fn probe() -> Result<Vec<(&'static str, u32)>, Box<dyn Error>> {
    let conn = Connection::connect_to_env()?;
    let (globals, _) = registry_queue_init::<Overlay>(&conn)?;
    Ok(globals.contents().with_list(|list| {
        REQUIRED
            .iter()
            .map(|&name| {
                let version = list
                    .iter()
                    .filter(|global| global.interface == name)
                    .map(|global| global.version)
                    .max()
                    .unwrap_or(0);
                (name, version)
            })
            .collect()
    }))
}

/// Opens a fullscreen, click-through overlay and plays the effect until it finishes.
/// `output` is an output name, `"all"`, or `None` to let the compositor pick one.
/// With `stats`, prints frame timings per screen afterwards.
pub fn run(build: Build, output: Option<&str>, stats: bool) -> Result<(), Box<dyn Error>> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init(&conn)?;
    let qh = queue.handle();

    let compositor = CompositorState::bind(&globals, &qh)?;
    let layer_shell = LayerShell::bind(&globals, &qh)?;
    let shm = Shm::bind(&globals, &qh)?;

    let mut app = Overlay {
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &qh),
        pool: SlotPool::new(4, &shm)?,
        shm,
        build,
        screens: Vec::new(),
        stats,
        error: None,
    };
    // Output names arrive as events, so wait for them before picking.
    queue.roundtrip(&mut app)?;

    for (name, output) in app.pick(output)? {
        let screen = Screen::new(name, output.as_ref(), &compositor, &layer_shell, &qh)?;
        app.screens.push(screen);
    }

    while !app.screens.is_empty() && app.error.is_none() {
        queue.blocking_dispatch(&mut app)?;
    }
    app.error.map_or(Ok(()), Err)
}

/// Time spent per phase, summed over all frames.
#[derive(Default)]
struct Stats {
    frames: u32,
    update: Duration,
    clear: Duration,
    draw: Duration,
}

impl Stats {
    fn print(&self, name: &str, (width, height): (u32, u32), elapsed: Duration) {
        let frames = self.frames.max(1);
        let secs = elapsed.as_secs_f32();
        eprintln!(
            "{name}: {width}x{height}, {} frames in {secs:.2} s ({:.0} fps)",
            self.frames,
            f64::from(self.frames) / elapsed.as_secs_f64(),
        );
        eprintln!(
            "{name}: per frame: update {} us, clear {} us, draw {} us",
            (self.update / frames).as_micros(),
            (self.clear / frames).as_micros(),
            (self.draw / frames).as_micros(),
        );
    }
}

/// A reusable shm buffer and the area that still holds pixels from its last use.
struct Frame {
    buffer: Buffer,
    drawn: Area,
}

/// One overlay surface with its own effect, on one output.
struct Screen {
    name: String,
    layer: LayerSurface,
    effect: Option<Box<dyn Effect>>,
    size: (u32, u32),
    frames: Vec<Frame>,
    /// Area drawn in the buffer that is currently on screen.
    shown: Area,
    started: Instant,
    last_frame: Instant,
    stats: Stats,
}

impl Screen {
    fn new(
        name: String,
        output: Option<&wl_output::WlOutput>,
        compositor: &CompositorState,
        layer_shell: &LayerShell,
        qh: &QueueHandle<Overlay>,
    ) -> Result<Self, Box<dyn Error>> {
        let surface = compositor.create_surface(qh);
        let layer =
            layer_shell.create_layer_surface(qh, surface, Layer::Overlay, Some("wl-party"), output);
        layer.set_anchor(Anchor::all());
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        // An empty input region makes every click fall through to the windows below.
        let region = Region::new(compositor)?;
        layer
            .wl_surface()
            .set_input_region(Some(region.wl_region()));
        layer.commit();
        Ok(Self {
            name,
            layer,
            effect: None,
            size: (0, 0),
            frames: Vec::new(),
            shown: Area::default(),
            started: Instant::now(),
            last_frame: Instant::now(),
            stats: Stats::default(),
        })
    }

    /// Steps and draws one frame. Returns `Ok(false)` once the effect has finished.
    fn draw(
        &mut self,
        pool: &mut SlotPool,
        qh: &QueueHandle<Overlay>,
    ) -> Result<bool, Box<dyn Error>> {
        let Some(effect) = self.effect.as_mut() else {
            return Ok(true);
        };

        let now = Instant::now();
        let dt = now
            .duration_since(self.last_frame)
            .as_secs_f32()
            .min(MAX_DT);
        self.last_frame = now;
        let running = effect.update(dt)?;
        self.stats.update += now.elapsed();
        if !running {
            return Ok(false);
        }

        let (width, height) = self.size;
        let stride = i32::try_from(width * 4)?;
        let surface = self.layer.wl_surface();
        surface.frame(qh, FrameCallbackData(surface.clone()));

        let free = self
            .frames
            .iter()
            .position(|frame| frame.buffer.canvas(pool).is_some());
        let index = match free {
            Some(index) => index,
            None if self.frames.len() < MAX_BUFFERS => {
                let (buffer, _) = pool.create_buffer(
                    width.cast_signed(),
                    height.cast_signed(),
                    stride,
                    wl_shm::Format::Argb8888,
                )?;
                // A fresh slot may hold anything, so clear it fully the first time.
                self.frames.push(Frame {
                    buffer,
                    drawn: Area::full(width, height),
                });
                self.frames.len() - 1
            }
            None => {
                surface.commit();
                return Ok(true);
            }
        };
        let frame = &mut self.frames[index];
        let Some(canvas) = frame.buffer.canvas(pool) else {
            surface.commit();
            return Ok(true);
        };

        let start = Instant::now();
        clear(canvas, width, frame.drawn);
        let cleared = Instant::now();
        let drawn = PixmapMut::from_bytes(canvas, width, height)
            .map_or(Area::default(), |mut pixmap| effect.draw(&mut pixmap));
        self.stats.clear += cleared - start;
        self.stats.draw += cleared.elapsed();
        self.stats.frames += 1;

        // Damage what was on screen plus what is new; at least one pixel, so frame callbacks keep coming.
        let damage = drawn.union(self.shown).union(Area::full(1, 1));
        frame.drawn = drawn;
        self.shown = drawn;
        surface.damage_buffer(
            damage.left.cast_signed(),
            damage.top.cast_signed(),
            (damage.right - damage.left).cast_signed(),
            (damage.bottom - damage.top).cast_signed(),
        );
        frame.buffer.attach_to(surface)?;
        self.layer.commit();
        Ok(true)
    }
}

/// Zeroes `area` in a canvas of `width` pixels per row.
fn clear(canvas: &mut [u8], width: u32, area: Area) {
    if area.is_empty() {
        return;
    }
    let stride = width as usize * 4;
    let (left, right) = (area.left as usize * 4, area.right as usize * 4);
    for row in canvas
        .chunks_exact_mut(stride)
        .take(area.bottom as usize)
        .skip(area.top as usize)
    {
        row[left..right].fill(0);
    }
}

struct Overlay {
    registry: RegistryState,
    outputs: OutputState,
    shm: Shm,
    pool: SlotPool,
    build: Build,
    screens: Vec<Screen>,
    stats: bool,
    error: Option<Box<dyn Error>>,
}

type Target = (String, Option<wl_output::WlOutput>);

impl Overlay {
    /// The outputs to play on, by name.
    fn pick(&self, output: Option<&str>) -> Result<Vec<Target>, Box<dyn Error>> {
        let Some(wanted) = output else {
            return Ok(vec![("screen".into(), None)]);
        };
        let all: Vec<Target> = self
            .outputs
            .outputs()
            .map(|output| {
                let name = self.outputs.info(&output).and_then(|info| info.name);
                (name.unwrap_or_default(), Some(output))
            })
            .collect();
        if wanted == "all" {
            return Ok(all);
        }
        let names: Vec<_> = all.iter().map(|(name, _)| name.clone()).collect();
        let picked: Vec<_> = all.into_iter().filter(|(name, _)| name == wanted).collect();
        if picked.is_empty() {
            return Err(
                format!("no output '{wanted}', available: all, {}", names.join(", ")).into(),
            );
        }
        Ok(picked)
    }

    fn screen(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.screens
            .iter()
            .position(|screen| screen.layer.wl_surface() == surface)
    }

    fn draw(&mut self, qh: &QueueHandle<Self>, index: usize) {
        match self.screens[index].draw(&mut self.pool, qh) {
            Ok(true) => {}
            Ok(false) => self.finish(index),
            Err(err) => {
                self.error = Some(err);
                self.finish(index);
            }
        }
    }

    /// Removes a screen; dropping its layer surface takes it off the output.
    fn finish(&mut self, index: usize) {
        let screen = self.screens.swap_remove(index);
        if self.stats {
            screen
                .stats
                .print(&screen.name, screen.size, screen.started.elapsed());
        }
    }
}

impl LayerShellHandler for Overlay {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if let Some(index) = self.screen(layer.wl_surface()) {
            self.finish(index);
        }
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(index) = self.screen(layer.wl_surface()) else {
            return;
        };
        let (width, height) = configure.new_size;
        if width == 0 || height == 0 {
            self.finish(index);
            return;
        }
        let screen = &mut self.screens[index];
        if screen.size != (width, height) {
            screen.frames.clear();
            screen.shown = Area::full(width, height);
        }
        screen.size = (width, height);
        if screen.effect.is_none() {
            #[allow(clippy::cast_precision_loss)] // screen sizes fit easily in f32
            let size = Size {
                width: width as f32,
                height: height as f32,
            };
            screen.effect = Some((self.build)(size));
            screen.started = Instant::now();
            screen.last_frame = Instant::now();
            self.draw(qh, index);
        }
    }
}

impl CompositorHandler for Overlay {
    fn frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if let Some(index) = self.screen(surface) {
            self.draw(qh, index);
        }
    }

    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for Overlay {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ShmHandler for Overlay {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Overlay {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState];
}

delegate_registry!(Overlay);
smithay_client_toolkit::delegate_dispatch2!(Overlay);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_zeroes_only_the_area() {
        let (width, height) = (4, 3);
        let mut canvas = vec![0xff; width * height * 4];
        let area = Area {
            left: 1,
            top: 1,
            right: 3,
            bottom: 2,
        };
        clear(&mut canvas, 4, area);
        for (index, pixel) in canvas.as_chunks::<4>().0.iter().enumerate() {
            let (x, y) = (index % width, index / width);
            let inside = (1..3).contains(&x) && y == 1;
            assert_eq!(*pixel == [0; 4], inside, "pixel {x},{y}");
        }
    }
}
