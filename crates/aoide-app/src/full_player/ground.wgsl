// The full player's moving ground, drawn from the cover's four main colours.
// Mode 1 is silk (colours folding slowly over each other), 2 aurora
// (curtains of light drifting over the darkest colour), 3 ink (clouds of
// colour spreading in dark water). Every pixel is held at most 0.18 linear
// light, as the still ground is, so white words read on it at 4.5:1.

struct Ground {
    colours: array<vec4<f32>, 4>,
    time: f32,
    aspect: f32,
    mode: f32,
    // 1 when the target isn't sRGB, so the shader writes sRGB itself.
    encode_srgb: f32,
}

@group(0) @binding(0) var<uniform> ground: Ground;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// One triangle over the whole of the widget's viewport.
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> Out {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    var out: Out;
    out.position = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash(i);
    let b = hash(i + vec2<f32>(1.0, 0.0));
    let c = hash(i + vec2<f32>(0.0, 1.0));
    let d = hash(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm(start: vec2<f32>) -> f32 {
    var p = start;
    var sum = 0.0;
    var amplitude = 0.5;
    for (var octave = 0; octave < 4; octave = octave + 1) {
        sum = sum + amplitude * noise(p);
        p = p * 2.03 + vec2<f32>(1.7, 9.2);
        amplitude = amplitude * 0.5;
    }
    return sum;
}

fn colour(index: i32) -> vec3<f32> {
    return ground.colours[index].rgb;
}

fn silk(p: vec2<f32>, t: f32) -> vec3<f32> {
    let q = vec2<f32>(
        fbm(p * 1.1 + vec2<f32>(0.0, t * 0.05)),
        fbm(p * 1.1 + vec2<f32>(5.2, -t * 0.04)),
    );
    let r = vec2<f32>(
        fbm(p * 1.4 + 2.2 * q + vec2<f32>(1.7, 9.2) + t * 0.03),
        fbm(p * 1.4 + 2.2 * q + vec2<f32>(8.3, 2.8) - t * 0.025),
    );
    let f = fbm(p + 2.0 * r);
    var c = mix(colour(0), colour(1), clamp(f * f * 2.2, 0.0, 1.0));
    c = mix(c, colour(2), clamp(length(q) * 0.7, 0.0, 1.0) * 0.6);
    c = mix(c, colour(3), clamp(r.x * r.x, 0.0, 1.0) * 0.5);
    // A sheen along the folds.
    let sheen = 0.5 + 0.5 * sin(7.0 * f + t * 0.15);
    return c * (0.75 + 0.45 * sheen);
}

fn aurora(uv: vec2<f32>, p: vec2<f32>, t: f32) -> vec3<f32> {
    var c = colour(3) * 0.35;
    for (var i = 0; i < 3; i = i + 1) {
        let n = f32(i);
        let drift = fbm(vec2<f32>(p.x * 0.8 + t * (0.04 + 0.02 * n), n * 3.1));
        let crest = 0.22 + 0.16 * n + 0.10 * sin(p.x * 1.3 + t * 0.09 * (n + 1.0)) + 0.18 * (drift - 0.5);
        // Light hangs below each crest and fades downwards, in streaks.
        let below = uv.y - crest;
        let fall = select(exp(-below * 4.0), exp(below * 40.0), below < 0.0);
        let streaks = 0.65 + 0.35 * sin(p.x * 34.0 + drift * 9.0 + t * 0.2);
        c = c + colour(i) * fall * streaks * 0.9;
    }
    return c;
}

fn ink(p: vec2<f32>, t: f32) -> vec3<f32> {
    var c = colour(3) * 0.3;
    for (var i = 0; i < 3; i = i + 1) {
        let n = f32(i);
        let centre = vec2<f32>(
            ground.aspect * (0.5 + 0.38 * sin(t * (0.031 + 0.011 * n) + n * 2.1)),
            0.5 + 0.36 * cos(t * (0.027 + 0.013 * n) + n * 1.3),
        );
        let warp = vec2<f32>(fbm(p * 1.6 + t * 0.03 + n), fbm(p * 1.6 - t * 0.025 + n * 4.0));
        let reach = length(p + (warp - 0.5) * 0.7 - centre);
        let cloud = (1.0 - smoothstep(0.0, 0.75, reach)) * (0.6 + 0.4 * fbm(p * 3.0 + warp * 2.0));
        c = mix(c, colour(i), cloud);
    }
    return c;
}

fn to_srgb(linear: vec3<f32>) -> vec3<f32> {
    let low = linear * 12.92;
    let high = 1.055 * pow(linear, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, linear <= vec3<f32>(0.0031308));
}

fn to_linear(srgb: vec3<f32>) -> vec3<f32> {
    let low = srgb / 12.92;
    let high = pow((srgb + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, srgb <= vec3<f32>(0.04045));
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let p = vec2<f32>(in.uv.x * ground.aspect, in.uv.y) * 1.6;
    let t = ground.time;
    var c: vec3<f32>;
    if (ground.mode < 1.5) {
        c = silk(p, t);
    } else if (ground.mode < 2.5) {
        c = aurora(in.uv, p, t);
    } else {
        c = ink(p, t);
    }
    c = max(c, vec3<f32>(0.0));
    let light = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    if (light > 0.18) {
        c = c * (0.18 / light);
    }
    // A grain under one 8-bit step of the sRGB the screen shows, so slow
    // gradients don't band; an sRGB target encodes linear light itself.
    let grain = (hash(in.position.xy + fract(t)) - 0.5) / 255.0;
    let shown = clamp(to_srgb(c) + grain, vec3<f32>(0.0), vec3<f32>(1.0));
    if (ground.encode_srgb > 0.5) {
        return vec4<f32>(shown, 1.0);
    }
    return vec4<f32>(to_linear(shown), 1.0);
}
