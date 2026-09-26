//! The OpenXR frame loop for the whole app: a browser panel, video playback
//! with a control bar, and controller input (point, click, drag).

use super::context::{VIEW_TYPE, XrContext};
use super::input::{Input, InputState, Ray};
use super::player::{PlayOptions, PlayStats, Placement, Playback, ViewOptions, eye_params};
use super::renderer::{QuadTarget, Renderer};
use crate::ui::canvas::Fonts;
use crate::ui::navigator::Navigator;
use crate::ui::{browser, controls};
use crate::vr::Projection;
use anyhow::Context;
use openxr as xr;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const CURSOR_PX: u32 = 64;
const CURSOR_SIZE: f32 = 0.035;
const DEFAULT_VOLUME: u32 = 6;

pub struct AppOptions {
    pub view: ViewOptions,
    pub play: PlayOptions,
    pub quit: Arc<AtomicBool>,
}

/// A flat UI panel floating in the LOCAL space.
#[derive(Clone, Copy)]
struct Panel {
    center: [f32; 3],
    /// Turn around +Y (radians), then tilt around the panel's X axis.
    yaw: f32,
    tilt: f32,
    size: [f32; 2],
    pixels: [u32; 2],
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

impl Panel {
    fn orientation(&self) -> xr::Quaternionf {
        let (sy, cy) = (self.yaw / 2.0).sin_cos();
        let (sx, cx) = (self.tilt / 2.0).sin_cos();
        // q = q_yaw · q_tilt
        xr::Quaternionf { x: cy * sx, y: sy * cx, z: -sy * sx, w: cy * cx }
    }

    /// Unit vectors: right, up, normal (towards the viewer).
    fn basis(&self) -> [[f32; 3]; 3] {
        let (sy, cy) = self.yaw.sin_cos();
        let (st, ct) = self.tilt.sin_cos();
        let right = [cy, 0.0, -sy];
        let up = [sy * st, ct, cy * st];
        let normal = [sy * ct, -st, cy * ct];
        [right, up, normal]
    }

    /// Canvas pixel hit by `ray`, if any.
    fn hit(&self, ray: &Ray) -> Option<(f32, f32)> {
        let [right, up, normal] = self.basis();
        let denom = dot(ray.direction, normal);
        if denom.abs() < 1e-4 {
            return None;
        }
        let to_center = [
            self.center[0] - ray.origin[0],
            self.center[1] - ray.origin[1],
            self.center[2] - ray.origin[2],
        ];
        let t = dot(to_center, normal) / denom;
        if t <= 0.0 {
            return None;
        }
        let p = add(ray.origin, scale(ray.direction, t));
        let local = [p[0] - self.center[0], p[1] - self.center[1], p[2] - self.center[2]];
        let u = dot(local, right) / self.size[0] + 0.5;
        let v = 0.5 - dot(local, up) / self.size[1];
        ((0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v))
            .then_some((u * self.pixels[0] as f32, v * self.pixels[1] as f32))
    }

    /// World position of canvas pixel (x, y), `lift` metres towards the viewer.
    fn point(&self, x: f32, y: f32, lift: f32) -> [f32; 3] {
        let [right, up, normal] = self.basis();
        let u = (x / self.pixels[0] as f32 - 0.5) * self.size[0];
        let v = (0.5 - y / self.pixels[1] as f32) * self.size[1];
        add(add(add(self.center, scale(right, u)), scale(up, v)), scale(normal, lift))
    }

    fn pose(&self) -> xr::Posef {
        xr::Posef {
            orientation: self.orientation(),
            position: xr::Vector3f { x: self.center[0], y: self.center[1], z: self.center[2] },
        }
    }
}

const BROWSER_PANEL: Panel = Panel {
    center: [0.0, -0.1, -1.5],
    yaw: 0.0,
    tilt: 0.0,
    size: [1.6, 1.0],
    pixels: [browser::WIDTH, browser::HEIGHT],
};

/// The control bar 1 m ahead of the head, below eye level, tilted towards it.
fn controls_panel(head: &xr::Posef) -> Panel {
    let q = head.orientation;
    // Forward (-Z) of the head, flattened to the horizon.
    let forward = [-2.0 * (q.x * q.z + q.w * q.y), 0.0, -(1.0 - 2.0 * (q.x * q.x + q.y * q.y))];
    let yaw = (-forward[0]).atan2(-forward[2]);
    let (sy, cy) = yaw.sin_cos();
    let p = head.position;
    Panel {
        center: [p.x - sy, p.y - 0.42, p.z - cy],
        yaw,
        tilt: -0.45,
        size: [1.2, 0.26],
        pixels: [controls::WIDTH, controls::HEIGHT],
    }
}

/// A white-ringed blue dot with premultiplied alpha.
fn cursor_image() -> Vec<u8> {
    let mut pixels = vec![0u8; (CURSOR_PX * CURSOR_PX * 4) as usize];
    let c = CURSOR_PX as f32 / 2.0;
    for y in 0..CURSOR_PX {
        for x in 0..CURSOR_PX {
            let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt();
            let outer = (c - 2.0 - d).clamp(0.0, 1.0);
            let inner = (c - 10.0 - d).clamp(0.0, 1.0);
            let rgb = [255.0 * (1.0 - inner) + 79.0 * inner, 255.0 * (1.0 - inner) + 140.0 * inner, 255.0];
            let i = ((y * CURSOR_PX + x) * 4) as usize;
            for (k, channel) in rgb.iter().enumerate() {
                pixels[i + k] = (channel * outer) as u8;
            }
            pixels[i + 3] = (255.0 * outer) as u8;
        }
    }
    pixels
}

fn quad_layer<'a>(
    space: &'a xr::Space,
    target: &'a QuadTarget,
    pose: xr::Posef,
    size: [f32; 2],
    blend: bool,
) -> xr::CompositionLayerQuad<'a, xr::Vulkan> {
    let layer = xr::CompositionLayerQuad::new()
        .space(space)
        .eye_visibility(xr::EyeVisibility::BOTH)
        .sub_image(
            xr::SwapchainSubImage::new()
                .swapchain(&target.swapchain)
                .image_array_index(0)
                .image_rect(xr::Rect2Di {
                    offset: xr::Offset2Di { x: 0, y: 0 },
                    extent: xr::Extent2Di { width: target.width as i32, height: target.height as i32 },
                }),
        )
        .pose(pose)
        .size(xr::Extent2Df { width: size[0], height: size[1] });
    if blend {
        layer.layer_flags(xr::CompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA)
    } else {
        layer
    }
}

enum Mode {
    Browser,
    Playing(Box<Playback>),
}

/// An active screen drag: aim direction and placement when it started.
struct Drag {
    hand: usize,
    yaw: f32,
    pitch: f32,
    start: Placement,
}

fn aim_angles(ray: &Ray) -> (f32, f32) {
    let d = ray.direction;
    ((-d[0]).atan2(-d[2]), d[1].clamp(-1.0, 1.0).asin())
}

fn wrap_angle(a: f32) -> f32 {
    (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

/// Runs the app. With a navigator the browser is shown; `initial` starts
/// playing immediately (and the app exits when it ends if there is no browser).
pub fn run(mut navigator: Option<Navigator>, initial: Option<Playback>, options: AppOptions) -> anyhow::Result<PlayStats> {
    let mut ctx = XrContext::new()?;
    let mut renderer = Renderer::new(&ctx)?;
    let input = Input::new(&ctx)?;
    let space = ctx
        .session
        .create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY)
        .context("Create LOCAL reference space")?;
    let mut fonts = Fonts::load()?;
    let mut browser_target = match navigator {
        Some(_) => Some(renderer.create_quad(&ctx, browser::WIDTH, browser::HEIGHT)?),
        None => None,
    };
    let mut controls_target = renderer.create_quad(&ctx, controls::WIDTH, controls::HEIGHT)?;
    let mut cursor = renderer.create_quad(&ctx, CURSOR_PX, CURSOR_PX)?;
    renderer.upload_quad(&mut cursor, &cursor_image())?;

    let mut mode = match initial {
        Some(playback) => Mode::Playing(Box::new(playback)),
        None => Mode::Browser,
    };
    let mut stats = PlayStats::default();
    let mut events = xr::EventDataBuffer::new();
    let mut running = false;
    let mut exit_requested = false;
    let mut active_hand = 1usize;
    let mut hovered: Option<browser::Hit> = None;
    let mut screenshot = options.play.screenshot.clone();
    let mut placement = Placement::new(&options.view);
    let mut volume = DEFAULT_VOLUME;
    let mut controls_panel_at: Option<Panel> = None;
    let mut controls_drawn: Option<(controls::State, controls::Hit)> = None;
    let mut drag: Option<Drag> = None;

    'main: loop {
        if options.quit.load(Ordering::Relaxed) && !exit_requested {
            exit_requested = true;
            if running {
                ctx.session.request_exit()?;
            } else {
                break;
            }
        }
        while let Some(event) = ctx.xr.poll_event(&mut events)? {
            match event {
                xr::Event::SessionStateChanged(e) => {
                    eprintln!("OpenXR session: {:?}", e.state());
                    match e.state() {
                        xr::SessionState::READY => {
                            ctx.session.begin(VIEW_TYPE)?;
                            running = true;
                        }
                        xr::SessionState::STOPPING => {
                            ctx.session.end()?;
                            running = false;
                        }
                        xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => break 'main,
                        _ => {}
                    }
                }
                xr::Event::InstanceLossPending(_) => break 'main,
                _ => {}
            }
        }
        if !running {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }

        let state = ctx.frame_waiter.wait()?;
        ctx.frame_stream.begin()?;
        stats.xr_frames += 1;
        if !state.should_render {
            ctx.frame_stream.end(state.predicted_display_time, ctx.blend_mode, &[])?;
            continue;
        }
        stats.rendered_xr_frames += 1;
        let now = state.predicted_display_time.as_nanos();
        let dt = state.predicted_display_period.as_nanos() as f32 / 1e9;
        let buttons = input.poll(&ctx, &space, state.predicted_display_time).unwrap_or_else(|e| {
            eprintln!("Input: {e:#}");
            InputState::default()
        });
        for (hand, pressed) in buttons.select.iter().enumerate() {
            if *pressed {
                active_hand = hand;
            }
        }
        let ray = buttons.rays[active_hand].as_ref().or(buttons.rays[1 - active_hand].as_ref());
        let (_, views) = ctx
            .session
            .locate_views(VIEW_TYPE, state.predicted_display_time, &space)?;

        // Leaving playback: B, end of video, or --duration reached.
        let mut stop_playback = false;
        if let Mode::Playing(playback) = &mut mode {
            let reached_end = options
                .play
                .duration
                .is_some_and(|d| playback.media_time(now).is_some_and(|t| t >= d));
            stop_playback = buttons.back || playback.finished(now) || reached_end;
        }
        if stop_playback {
            if let Mode::Playing(playback) = std::mem::replace(&mut mode, Mode::Browser) {
                stats.displayed_frames += playback.stats.displayed_frames;
                stats.uploaded_frames += playback.stats.uploaded_frames;
                stats.skipped_frames += playback.stats.skipped_frames;
                stats.media_seconds = playback.stats.media_seconds;
            }
            controls_panel_at = None;
            drag = None;
            match navigator.as_mut() {
                Some(nav) => nav.redraw(),
                None => options.quit.store(true, Ordering::Relaxed),
            }
        }

        let mut cursor_at: Option<(Panel, f32, f32)> = None;
        let mut projection_views: Vec<xr::CompositionLayerProjectionView<xr::Vulkan>> = Vec::new();
        match &mut mode {
            Mode::Browser => {
                let (Some(nav), Some(target)) = (navigator.as_mut(), browser_target.as_mut()) else {
                    ctx.frame_stream.end(state.predicted_display_time, ctx.blend_mode, &[])?;
                    continue;
                };
                if let Some(opened) = nav.poll() {
                    eprintln!("Playing {} as {:?} / {:?}", opened.name, opened.layout.projection, opened.layout.stereo);
                    mode = Mode::Playing(Box::new(Playback::start(
                        opened.decoder,
                        opened.layout,
                        0.0,
                        volume as f32 / controls::VOLUME_STEPS as f32,
                    )));
                    ctx.frame_stream.end(state.predicted_display_time, ctx.blend_mode, &[])?;
                    continue;
                }
                let point = ray.and_then(|r| BROWSER_PANEL.hit(r));
                let hit = point.map_or(browser::Hit::Nothing, |(x, y)| browser::hit(nav.view(), x, y));
                if buttons.select[active_hand] {
                    match hit {
                        browser::Hit::DialogButton => nav.close_dialog(),
                        browser::Hit::Row(i) if !nav.dialog_open() => nav.select(i),
                        _ => {}
                    }
                }
                if buttons.back {
                    nav.back();
                }
                if buttons.scroll.abs() > 0.2 {
                    nav.scroll_by(-buttons.scroll * 12.0 * dt);
                }
                if nav.take_dirty() || hovered != Some(hit) {
                    hovered = Some(hit);
                    let hover_point = match hit {
                        browser::Hit::Nothing => None,
                        _ => point,
                    };
                    let canvas = browser::render(nav.view(), &mut fonts, hover_point, false);
                    renderer.upload_quad(target, &canvas.pixels)?;
                }
                if let Some((x, y)) = point {
                    cursor_at = Some((BROWSER_PANEL, x, y));
                }
            }
            Mode::Playing(playback) => {
                // A shows or hides the control bar in front of the head.
                if buttons.pause {
                    controls_panel_at = match controls_panel_at {
                        Some(_) => None,
                        None => views.first().map(|v| controls_panel(&v.pose)),
                    };
                    controls_drawn = None;
                }
                let curved_applies = playback.layout.projection == Projection::Flat;
                let ui_state = controls::State {
                    paused: playback.paused(),
                    position: playback.position().floor(),
                    duration: playback.duration,
                    curved: curved_applies.then_some(placement.curved),
                    volume: playback.has_audio().then_some(volume),
                };
                let control_point = controls_panel_at.and_then(|p| ray.and_then(|r| p.hit(r)).map(|pt| (p, pt)));
                let control_hit =
                    control_point.map_or(controls::Hit::Nothing, |(_, (x, y))| controls::hit(&ui_state, x, y));

                if buttons.select[active_hand] && drag.is_none() {
                    if control_point.is_some() {
                        match control_hit {
                            controls::Hit::PlayPause => playback.toggle_pause(now),
                            controls::Hit::Seek(f) => playback.seek(f as f64 * playback.duration),
                            controls::Hit::Curved => placement.curved = !placement.curved,
                            controls::Hit::Reset => {
                                placement = Placement { curved: placement.curved, ..Placement::new(&options.view) };
                            }
                            controls::Hit::Volume(level) => {
                                volume = level;
                                playback.set_volume(level as f32 / controls::VOLUME_STEPS as f32);
                            }
                            controls::Hit::Nothing => {}
                        }
                        controls_drawn = None;
                    } else if let Some(r) = buttons.rays[active_hand].as_ref() {
                        // Trigger held away from the controls: drag the screen.
                        let (yaw, pitch) = aim_angles(r);
                        drag = Some(Drag { hand: active_hand, yaw, pitch, start: placement });
                    }
                }
                if let Some(d) = &drag {
                    match (buttons.select_held[d.hand], buttons.rays[d.hand].as_ref()) {
                        (true, Some(r)) => {
                            let (yaw, pitch) = aim_angles(r);
                            placement.yaw = d.start.yaw + wrap_angle(yaw - d.yaw);
                            placement.pitch = (d.start.pitch + (pitch - d.pitch)).clamp(-1.3, 1.3);
                            // The stick pushes the screen away or pulls it closer.
                            if buttons.scroll.abs() > 0.2 {
                                placement.distance =
                                    (placement.distance + buttons.scroll * 2.0 * dt).clamp(0.8, 12.0);
                            }
                        }
                        _ => drag = None,
                    }
                }

                if controls_panel_at.is_some() {
                    if controls_drawn.as_ref() != Some(&(ui_state.clone(), control_hit)) {
                        let canvas = controls::render(&ui_state, &mut fonts, control_hit);
                        renderer.upload_quad(&mut controls_target, &canvas.pixels)?;
                        controls_drawn = Some((ui_state, control_hit));
                    }
                    if let Some((p, (x, y))) = control_point {
                        cursor_at = Some((p, x, y));
                    }
                }

                let upload = playback.advance(now);
                renderer.begin_frame(if upload { playback.current() } else { None })?;
                if upload {
                    playback.stats.uploaded_frames += 1;
                }
                let show = playback.current().is_some();
                if show {
                    playback.stats.displayed_frames += 1;
                }
                let tex = renderer.video_size();
                let mut indices = [0u32; 2];
                for (eye, view) in views.iter().enumerate() {
                    let target = &mut renderer.eyes[eye];
                    let index = target.swapchain.acquire_image()?;
                    target.swapchain.wait_image(xr::Duration::INFINITE)?;
                    indices[eye] = index;
                    let params = eye_params(view, eye, &playback.layout, tex, &options.view, &placement);
                    renderer.draw_eye(eye, index, &params, show);
                }
                let media_time = playback.media_time(now);
                let capture = match (&screenshot, media_time) {
                    (Some((_, at)), Some(t)) if t >= *at && show => screenshot.take(),
                    _ => None,
                };
                if capture.is_some() {
                    renderer.record_readback(0, indices[0])?;
                }
                renderer.end_frame()?;
                if let Some((path, _)) = capture {
                    renderer.take_screenshot(0, &path)?;
                    stats.screenshot = Some(path.display().to_string());
                }
                for eye in renderer.eyes.iter_mut() {
                    eye.swapchain.release_image()?;
                }
                projection_views = views
                    .iter()
                    .zip(&renderer.eyes)
                    .map(|(view, target)| {
                        xr::CompositionLayerProjectionView::new().pose(view.pose).fov(view.fov).sub_image(
                            xr::SwapchainSubImage::new().swapchain(&target.swapchain).image_array_index(0).image_rect(
                                xr::Rect2Di {
                                    offset: xr::Offset2Di { x: 0, y: 0 },
                                    extent: xr::Extent2Di { width: target.width as i32, height: target.height as i32 },
                                },
                            ),
                        )
                    })
                    .collect();
            }
        }

        // Quad layers to show this frame: (target, pose, size, blend).
        let mut quads: Vec<(&QuadTarget, xr::Posef, [f32; 2], bool)> = Vec::new();
        match &mode {
            Mode::Browser => {
                if let Some(target) = browser_target.as_ref().filter(|t| t.ready) {
                    quads.push((target, BROWSER_PANEL.pose(), BROWSER_PANEL.size, false));
                }
            }
            Mode::Playing(_) => {
                if let Some(panel) = controls_panel_at.filter(|_| controls_target.ready) {
                    quads.push((&controls_target, panel.pose(), panel.size, false));
                }
            }
        }
        if let Some((panel, x, y)) = cursor_at {
            // Lifted a few millimetres so the cursor sits in front of the panel.
            let p = panel.point(x, y, 0.005);
            let pose = xr::Posef {
                orientation: panel.orientation(),
                position: xr::Vector3f { x: p[0], y: p[1], z: p[2] },
            };
            quads.push((&cursor, pose, [CURSOR_SIZE, CURSOR_SIZE], true));
        }

        let projection = xr::CompositionLayerProjection::new().space(&space).views(&projection_views);
        let quad_layers: Vec<xr::CompositionLayerQuad<xr::Vulkan>> = quads
            .iter()
            .map(|(target, pose, size, blend)| quad_layer(&space, target, *pose, *size, *blend))
            .collect();
        let mut layers: Vec<&xr::CompositionLayerBase<xr::Vulkan>> = Vec::new();
        if !projection_views.is_empty() {
            layers.push(&projection);
        }
        for quad in &quad_layers {
            layers.push(&**quad);
        }
        ctx.frame_stream.end(state.predicted_display_time, ctx.blend_mode, &layers)?;
    }
    options.quit.store(true, Ordering::Relaxed);
    if let Mode::Playing(playback) = mode {
        stats.displayed_frames += playback.stats.displayed_frames;
        stats.uploaded_frames += playback.stats.uploaded_frames;
        stats.skipped_frames += playback.stats.skipped_frames;
        stats.media_seconds = playback.stats.media_seconds;
    }
    drop(renderer);
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_hit_round_trips_through_point() {
        let panel = Panel { center: [0.3, -0.4, -1.0], yaw: 0.5, tilt: -0.45, size: [1.2, 0.26], pixels: [1200, 260] };
        for (x, y) in [(600.0, 130.0), (100.0, 40.0), (1150.0, 250.0)] {
            let target = panel.point(x, y, 0.0);
            let len = dot(target, target).sqrt();
            let ray = Ray { origin: [0.0; 3], direction: scale(target, 1.0 / len) };
            let (hx, hy) = panel.hit(&ray).expect("hit");
            assert!((hx - x).abs() < 2.0 && (hy - y).abs() < 2.0, "({x},{y}) -> ({hx},{hy})");
        }
    }

    #[test]
    fn panel_orientation_matches_basis() {
        let panel = Panel { center: [0.0; 3], yaw: 0.7, tilt: -0.4, size: [1.0, 1.0], pixels: [1, 1] };
        let q = panel.orientation();
        // Rotate +Z (the quad's normal) by q and compare with the basis normal.
        let v = [0.0f32, 0.0, 1.0];
        let t = [2.0 * (q.y * v[2] - q.z * v[1]), 2.0 * (q.z * v[0] - q.x * v[2]), 2.0 * (q.x * v[1] - q.y * v[0])];
        let rotated = [
            v[0] + q.w * t[0] + (q.y * t[2] - q.z * t[1]),
            v[1] + q.w * t[1] + (q.z * t[0] - q.x * t[2]),
            v[2] + q.w * t[2] + (q.x * t[1] - q.y * t[0]),
        ];
        let normal = panel.basis()[2];
        for i in 0..3 {
            assert!((rotated[i] - normal[i]).abs() < 1e-5, "{rotated:?} vs {normal:?}");
        }
    }
}
