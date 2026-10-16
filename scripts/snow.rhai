// Snowflakes drift down from the top

fn start(ctx) {
    ctx.gravity = 90.0;
    ctx.drag = 0.6;
    for i in 0..ctx.count {
        ctx.emit(#{
            x: rand() * ctx.width,
            y: -10.0 - rand() * ctx.height * 0.3,
            vx: (rand() - 0.5) * 40.0,
            vy: 60.0 + rand() * 40.0,
            shape: "circle",
            size: 3.0 + rand() * 6.0,
            color: "#ffffff",
            spin: false,
            wobble: 10.0 + rand() * 25.0,
        });
    }
}
