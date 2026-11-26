// VRedze: hands, controllers and pointer beams as capsules, drawn over the
// video and the panels in a projection layer of their own. Each capsule is
// one instance: a box around it is rasterised, and each pixel casts its ray
// against the capsule, so the shapes are exact at any size without meshes.

struct Eye {
    // World-to-view rotation (rows).
    row0: vec4<f32>,
    row1: vec4<f32>,
    row2: vec4<f32>,
    // tan(angle) of the eye's field of view: left, right, down, up.
    tan: vec4<f32>,
    // xyz: eye position in the reference space; w: tan(angle) of one pixel,
    // for smoothing the edges.
    origin: vec4<f32>,
}

var<immediate> eye: Eye;

const NEAR: f32 = 0.02;
const FAR: f32 = 50.0;

struct Capsule {
    // xyz: one end; w: radius (metres).
    @location(0) a: vec4<f32>,
    // xyz: the other end; w: opacity there.
    @location(1) b: vec4<f32>,
    // rgb: linear colour; a: opacity at the first end.
    @location(2) color: vec4<f32>,
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) @interpolate(flat) a: vec4<f32>,
    @location(2) @interpolate(flat) b: vec4<f32>,
    @location(3) @interpolate(flat) color: vec4<f32>,
}

// Reference space to clip space for this eye (Vulkan: y down, depth 0..1).
fn to_clip(p: vec3<f32>) -> vec4<f32> {
    let d = p - eye.origin.xyz;
    let v = vec3<f32>(dot(eye.row0.xyz, d), dot(eye.row1.xyz, d), dot(eye.row2.xyz, d));
    let l = eye.tan.x;
    let r = eye.tan.y;
    let down = eye.tan.z;
    let up = eye.tan.w;
    return vec4<f32>(
        (2.0 * v.x + (r + l) * v.z) / (r - l),
        (-2.0 * v.y - (up + down) * v.z) / (up - down),
        (FAR * v.z + NEAR * FAR) / (NEAR - FAR),
        -v.z,
    );
}

// A box around the capsule (padded for the smoothed edge), as a 14-vertex
// triangle strip.
@vertex
fn vs_main(@builtin(vertex_index) i: u32, c: Capsule) -> VsOut {
    let bit = 1u << i;
    let corner = vec3<f32>(
        select(0.0, 1.0, (0x287au & bit) != 0u),
        select(0.0, 1.0, (0x02afu & bit) != 0u),
        select(0.0, 1.0, (0x31e3u & bit) != 0u),
    );
    let axis = c.b.xyz - c.a.xyz;
    let len = max(length(axis), 1e-5);
    let u = axis / len;
    let helper = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(u.y) > 0.9);
    let v = normalize(cross(u, helper));
    let w = cross(u, v);
    let dist = length(c.a.xyz - eye.origin.xyz);
    let r = c.a.w + 2.0 * dist * eye.origin.w + 0.001;
    let world = c.a.xyz + u * mix(-r, len + r, corner.x) + v * mix(-r, r, corner.y)
        + w * mix(-r, r, corner.z);
    var out: VsOut;
    out.pos = to_clip(world);
    out.world = world;
    out.a = c.a;
    out.b = c.b;
    out.color = c.color;
    return out;
}

// Distance along the ray to the capsule, or -1 (Inigo Quilez's formula).
fn hit(ro: vec3<f32>, rd: vec3<f32>, pa: vec3<f32>, pb: vec3<f32>, r: f32) -> f32 {
    let ba = pb - pa;
    let oa = ro - pa;
    let baba = dot(ba, ba);
    let bard = dot(ba, rd);
    let baoa = dot(ba, oa);
    let rdoa = dot(rd, oa);
    let oaoa = dot(oa, oa);
    let a = baba - bard * bard;
    let b = baba * rdoa - baoa * bard;
    let c = baba * oaoa - baoa * baoa - r * r * baba;
    let h = b * b - a * c;
    if h >= 0.0 && a > 1e-9 {
        let t = (-b - sqrt(h)) / a;
        let y = baoa + t * bard;
        if y > 0.0 && y < baba {
            return t;
        }
    }
    // The rounded ends.
    var best = -1.0;
    for (var k = 0; k < 2; k++) {
        let oc = select(ro - pb, oa, k == 0);
        let bb = dot(rd, oc);
        let cc = dot(oc, oc) - r * r;
        let hh = bb * bb - cc;
        if hh > 0.0 {
            let t = -bb - sqrt(hh);
            if t > 0.0 && (best < 0.0 || t < best) {
                best = t;
            }
        }
    }
    return best;
}

// The point of the segment nearest to `p`.
fn nearest(p: vec3<f32>, pa: vec3<f32>, pb: vec3<f32>) -> vec3<f32> {
    let ba = pb - pa;
    let s = clamp(dot(p - pa, ba) / max(dot(ba, ba), 1e-10), 0.0, 1.0);
    return pa + s * ba;
}

struct FsOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

@fragment
fn fs_main(in: VsOut) -> FsOut {
    let ro = eye.origin.xyz;
    let rd = normalize(in.world - ro);
    let pa = in.a.xyz;
    let pb = in.b.xyz;
    let r = in.a.w;
    var out: FsOut;
    var p: vec3<f32>;
    var n: vec3<f32>;
    var cover = 1.0;
    let t = hit(ro, rd, pa, pb, r);
    if t > 0.0 {
        p = ro + t * rd;
        n = normalize(p - nearest(p, pa, pb));
        let clip = to_clip(p);
        out.depth = clip.z / clip.w;
    } else {
        // Just outside the edge: partly covered. Where the ray passes
        // nearest to the segment, and how far off it is in pixels.
        let ba = pb - pa;
        let oa = ro - pa;
        let baba = max(dot(ba, ba), 1e-10);
        let bard = dot(ba, rd);
        let s = clamp((dot(ba, oa) - dot(rd, oa) * bard) / max(baba - bard * bard, 1e-10), 0.0, 1.0);
        let q = pa + s * ba;
        let along = max(dot(q - ro, rd), 0.0);
        let near_ray = ro + along * rd;
        let miss = length(near_ray - q) - r;
        cover = clamp(0.5 - miss / max(along * eye.origin.w, 1e-6), 0.0, 1.0);
        if cover <= 0.0 {
            discard;
        }
        n = normalize(near_ray - q);
        p = q + n * r;
        // Edge pixels never hide what is drawn after them.
        out.depth = 1.0;
    }
    let ba = pb - pa;
    let h = clamp(dot(p - pa, ba) / max(dot(ba, ba), 1e-10), 0.0, 1.0);
    let alpha = mix(in.color.a, in.b.w, h) * cover;
    let light = normalize(vec3<f32>(0.3, 1.0, 0.5));
    let diffuse = 0.5 + 0.5 * max(dot(n, light), 0.0);
    let rim = pow(1.0 - abs(dot(n, rd)), 2.0) * 0.3;
    let rgb = min(in.color.rgb * diffuse + vec3<f32>(rim), vec3<f32>(1.0));
    // Premultiplied, as the compositor expects for a blended layer.
    out.color = vec4<f32>(rgb * alpha, alpha);
    return out;
}
