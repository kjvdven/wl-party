# wl-party

Click-through confetti overlay for Wayland compositors. Plays an effect on top of
everything, never steals focus or clicks, and exits when the effect is done.

Effects are built in (Rust) or written as small [Rhai](https://rhai.rs) scripts that
load at runtime, no recompiling needed.

## Requirements

- A compositor with `zwlr_layer_shell_v1`: niri, Sway, Hyprland, river, KDE, and others.
  GNOME doesn't support it.
- Rust 1.88+ to build. No system libraries needed: everything is pure Rust.

## Install

Download a static binary from the [latest release](../../releases/latest); it runs on any
Linux distro (`x86_64` and `aarch64`):

```sh
curl -LO https://github.com/kjvdven/wl-party/releases/latest/download/wl-party-x86_64-linux
sha256sum -c <(curl -L https://github.com/kjvdven/wl-party/releases/latest/download/wl-party-x86_64-linux.sha256)
install -Dm755 wl-party-x86_64-linux ~/.local/bin/wl-party
```

Or build from source:

```sh
cargo install --path .
```

This builds an optimized binary and puts it in `~/.cargo/bin/`.

### Build the binary yourself

```sh
cargo build --release
```

The binary is `target/release/wl-party` (~3 MB, stripped). It only links against libc,
so you can copy it anywhere on your `PATH`:

```sh
install -Dm755 target/release/wl-party ~/.local/bin/wl-party
```

The release profile uses LTO, a single codegen unit and `panic = "abort"` for a small,
fast binary; expect a ~30 s build.

Check your setup:

```sh
wl-party doctor
```

## Usage

```sh
wl-party                      # confetti
wl-party rain                 # another effect
wl-party fireworks -c 600     # more particles (1-5000, default 300)
wl-party snow -o all          # on every output
wl-party -o DP-2              # on one output
```

| Command | What it does |
|---|---|
| `wl-party [EFFECT]` | Play an effect (default `confetti`) |
| `wl-party list` | List built-in effects and scripts |
| `wl-party new <name>` | Create a starter script |
| `wl-party check <name\|file.rhai>` | Run a script headless and report errors |
| `wl-party doctor` | Check the compositor, the effects directory and every script |

| Option | |
|---|---|
| `-c, --count <N>` | Number of particles |
| `-o, --output <NAME\|all>` | Output to play on; default lets the compositor pick |
| `--seed <N>` | Repeat a run exactly |
| `--stats` | Print frame timings afterwards |

### Bind it to a key

niri (`~/.config/niri/config.kdl`):

```kdl
binds {
    Mod+Shift+C { spawn "wl-party"; }
}
```

Sway: `bindsym $mod+Shift+c exec wl-party`

## Write your own effect

Scripts live in `~/.config/wl-party/effects/` (or `$XDG_CONFIG_HOME/wl-party/effects/`).
The file name is the effect name.

**1. Create one from the template:**

```sh
wl-party new bubbles
```

This writes `bubbles.rhai` with the full API in a comment on top and a working example.

**2. Edit it.** A script has up to two hooks:

```rhai
// Rockets that burst into gold sparks       <- first line: description in `wl-party list`

const ROCKET = "M0 -10 C4 -6 4 2 3 6 L5 10 L-5 10 L-3 6 C-4 2 -4 -6 0 -10 Z";

// Runs once when the overlay opens.
fn start(ctx) {
    for i in 0..5 {
        ctx.emit(#{
            x: ctx.width * (0.2 + 0.15 * i),
            y: ctx.height,
            vy: -ctx.height,
            fuse: 1.0,            // expires after 1 second...
            shape: ROCKET,
            size: 28,
            spin: false,
            tag: "rocket",
        });
    }
}

// ...and then this runs for it.
fn expire(ctx, p) {
    if p.tag == "rocket" {
        ctx.burst(p.x, p.y, 60, "#ffd700");
    }
}
```

**3. Check it, then play it:**

```sh
wl-party check bubbles     # ok: 30 particles at start, peak 300, ends after 3.1 s
wl-party bubbles
```

`check` simulates the effect for up to 60 seconds without opening an overlay, so errors in
`expire` show up too, with line numbers.

### How an effect runs

Rust does all physics and drawing; your script only runs in `start` and when a particle
expires, so even large effects stay cheap. Each frame, every particle:

1. gets gravity added to `vy`, and its speed slowed by `drag`
2. moves, spins and sways (`wobble`)
3. counts down its `fuse`: in the last 0.4 s it fades out, at 0 it calls `expire` and is removed
4. is removed when it leaves the screen and can't come back

The effect ends when no particles are left. An effect that is still running after
60 seconds fails `check`.

### API

**Hooks** (both optional)

| | |
|---|---|
| `fn start(ctx)` | Runs once when the overlay opens |
| `fn expire(ctx, p)` | Runs when a particle's fuse runs out; `p` has `x`, `y`, `vx`, `vy`, `tag` |

**`ctx`**

| | |
|---|---|
| `ctx.width`, `ctx.height` | Overlay size in pixels |
| `ctx.count` | Value of `--count` |
| `ctx.gravity` | Downward pull in px/s², settable; negative pulls up. Default `height * 0.6` |
| `ctx.drag` | Air resistance per second, settable. Default `1.5` |
| `ctx.emit(#{ ... })` | Add a particle, see below |
| `ctx.burst(x, y, n)` | `n` sparks in all directions; optional 4th argument: color |
| `rand()` | Random number between 0 and 1 |

**`ctx.emit` fields** (all optional)

| Field | Default | |
|---|---|---|
| `x`, `y` | `0` | Position in pixels; `0, 0` is the top left |
| `vx`, `vy` | `0` | Velocity in px/s; negative `vy` goes up |
| `fuse` | never | Seconds until it fades, expires and calls `expire` |
| `shape` | `"rect"` | `"rect"`, `"circle"` or SVG path data |
| `size` | random confetti / `16` | Pixels, 1-512 |
| `color` | random confetti color | `"#rrggbb"` |
| `spin` | `true` | `false` keeps it upright and flat |
| `wobble` | `0` | Sideways sway in pixels |
| `tag` | `""` | Any string, handed back to `expire` |

### Shapes from SVG

Any SVG path works as a shape: copy the `d="..."` attribute from an SVG file (or from an
icon set) and pass it as `shape`. It is centered and scaled to fit `size`, so the
original coordinates don't matter. Shapes are filled with a single color.

```rhai
const HEART = "M12 21 C12 21 3 14 3 8 A4.5 4.5 0 0 1 12 6 A4.5 4.5 0 0 1 21 8 C21 14 12 21 12 21 Z";
ctx.emit(#{ shape: HEART, size: 40, color: "#ff4d6d", spin: false });
```

### Examples

[`scripts/`](scripts) has `fireworks.rhai` (bursts, rings and hearts), `balloons.rhai`
and `snow.rhai`. Copy them to try:

```sh
mkdir -p ~/.config/wl-party/effects
cp scripts/*.rhai ~/.config/wl-party/effects/
```

### Limits

Scripts are sandboxed. They can't read files, use the network or load modules, and `eval`
is disabled. Each hook call is limited in operations and memory, so an endless loop is
stopped ("Too many operations") instead of hanging your desktop. At most 5000 particles
and 64 shapes are live at a time; scripts are at most 64 KB.

## Built-in effects

Built-in effects are Rust, for when a script isn't enough: add a file in `src/effects/`
that implements the `Effect` trait (`src/effect.rs`) and add its `SPEC` to `REGISTRY`.
`src/effects/rain.rs` is a small example.

## Releasing

Bump `version` in `Cargo.toml`, merge to `main`, then tag it:

```sh
git tag v0.2.0
git push origin v0.2.0
```

The release workflow builds the static binaries and publishes them with checksums.

## License

MIT
