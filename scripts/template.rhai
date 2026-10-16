// A fountain of bubbles that pop into confetti

// The first line above is the description shown by `wl-party list`.
//
// Hooks (both optional):
//   fn start(ctx)       runs once when the overlay opens
//   fn expire(ctx, p)   runs when a particle's fuse runs out; p has x, y, vx, vy, tag
//
// ctx:
//   ctx.width, ctx.height     overlay size in pixels
//   ctx.count                 value of --count
//   ctx.gravity, ctx.drag     physics for all particles, settable
//   ctx.emit(#{ ... })        add a particle, every field optional:
//       x, y, vx, vy          position and velocity in pixels (per second)
//       fuse                  seconds until it expires and fades out
//       shape                 "rect" (default), "circle" or SVG path data ("M0 0 L10 0 ...")
//       size                  pixels, 1-512
//       color                 "#rrggbb", default is a random confetti color
//       shadow                "#rrggbb", a soft copy behind it so it shows on light backgrounds
//       spin                  false keeps it upright
//       wobble                sideways sway in pixels
//       tag                   any string, handed back to expire
//   ctx.burst(x, y, n)        n sparks in all directions, optional 4th argument: color
//   rand()                    random number between 0 and 1
//
// Particles that leave the screen are removed; the effect ends when none are left.

fn start(ctx) {
    ctx.gravity = ctx.height * 0.4;
    for i in 0..ctx.count / 10 {
        ctx.emit(#{
            x: ctx.width / 2.0,
            y: ctx.height,
            vx: (rand() - 0.5) * ctx.width * 0.3,
            vy: -ctx.height * (0.6 + rand() * 0.3),
            fuse: 0.8 + rand() * 0.8,
            shape: "circle",
            size: 24,
            color: "#5ce1e6",
            wobble: 6,
            tag: "bubble",
        });
    }
}

fn expire(ctx, p) {
    if p.tag == "bubble" {
        ctx.burst(p.x, p.y, 10);
    }
}
