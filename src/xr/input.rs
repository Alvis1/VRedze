//! Controller input: an aim ray per hand (smoothed), select (trigger or A),
//! back (B), scroll (thumbstick; grip held = faster). Bindings are suggested for the Steam
//! Frame controller, Index controllers and the generic simple profile.

use super::context::XrContext;
use openxr as xr;

pub struct Ray {
    pub origin: [f32; 3],
    pub direction: [f32; 3],
}

#[derive(Default)]
pub struct InputState {
    /// Aim rays per hand (left, right), when tracked.
    pub rays: [Option<Ray>; 2],
    /// Select pressed this frame, per hand.
    pub select: [bool; 2],
    /// Select held down, per hand (dragging).
    pub select_held: [bool; 2],
    pub back: bool,
    /// Thumbstick pressed in (reset view) this frame.
    pub reset: bool,
    /// Grip (lower trigger) held on either hand: a modifier, e.g. fast scroll.
    pub grip: bool,
    /// Thumbstick vertical deflection (-1..1, up positive), strongest hand.
    pub scroll: f32,
}

pub struct Input {
    set: xr::ActionSet,
    aim: xr::Action<xr::Posef>,
    select: xr::Action<bool>,
    back: xr::Action<bool>,
    reset: xr::Action<bool>,
    grip: xr::Action<bool>,
    scroll: xr::Action<xr::Vector2f>,
    hands: [xr::Path; 2],
    spaces: [xr::Space; 2],
    /// Per hand: origin and direction filters.
    filters: [[OneEuro; 2]; 2],
}

/// One Euro filter (Casiez et al.): heavy smoothing while still, little lag
/// when moving fast. Filters a 3-vector.
#[derive(Clone, Copy)]
struct OneEuro {
    min_cutoff: f32,
    beta: f32,
    value: Option<[f32; 3]>,
    speed: [f32; 3],
    time: i64,
}

impl OneEuro {
    const fn new(min_cutoff: f32, beta: f32) -> Self {
        Self {
            min_cutoff,
            beta,
            value: None,
            speed: [0.0; 3],
            time: 0,
        }
    }

    fn alpha(cutoff: f32, dt: f32) -> f32 {
        let tau = 1.0 / (2.0 * std::f32::consts::PI * cutoff);
        1.0 / (1.0 + tau / dt)
    }

    fn filter(&mut self, x: [f32; 3], time: i64) -> [f32; 3] {
        let Some(prev) = self.value else {
            self.value = Some(x);
            self.time = time;
            return x;
        };
        let dt = ((time - self.time) as f32 / 1e9).clamp(1e-4, 0.1);
        self.time = time;
        let a_d = Self::alpha(1.0, dt);
        let mut out = [0.0; 3];
        for i in 0..3 {
            let raw_speed = (x[i] - prev[i]) / dt;
            self.speed[i] += a_d * (raw_speed - self.speed[i]);
            let cutoff = self.min_cutoff + self.beta * self.speed[i].abs();
            out[i] = prev[i] + Self::alpha(cutoff, dt) * (x[i] - prev[i]);
        }
        self.value = Some(out);
        out
    }

    fn reset(&mut self) {
        self.value = None;
    }
}

fn rotate(q: xr::Quaternionf, v: [f32; 3]) -> [f32; 3] {
    // v' = v + 2w(q×v) + 2 q×(q×v)
    let (qx, qy, qz, w) = (q.x, q.y, q.z, q.w);
    let t = [
        2.0 * (qy * v[2] - qz * v[1]),
        2.0 * (qz * v[0] - qx * v[2]),
        2.0 * (qx * v[1] - qy * v[0]),
    ];
    [
        v[0] + w * t[0] + (qy * t[2] - qz * t[1]),
        v[1] + w * t[1] + (qz * t[0] - qx * t[2]),
        v[2] + w * t[2] + (qx * t[1] - qy * t[0]),
    ]
}

impl Input {
    pub fn new(ctx: &XrContext) -> anyhow::Result<Self> {
        let xr_ = &ctx.xr;
        let hands = [
            xr_.string_to_path("/user/hand/left")?,
            xr_.string_to_path("/user/hand/right")?,
        ];
        let set = xr_.create_action_set("player", "Player", 0)?;
        let aim = set.create_action::<xr::Posef>("aim", "Pointer", &hands)?;
        let select = set.create_action::<bool>("select", "Select", &hands)?;
        let back = set.create_action::<bool>("back", "Back", &hands)?;
        let reset = set.create_action::<bool>("reset", "Reset view", &hands)?;
        let grip = set.create_action::<bool>("grip", "Modifier", &hands)?;
        let scroll = set.create_action::<xr::Vector2f>("scroll", "Scroll", &hands)?;

        let binding = |action: &str, path: xr::Path| match action {
            "aim" => xr::Binding::new(&aim, path),
            "select" => xr::Binding::new(&select, path),
            "back" => xr::Binding::new(&back, path),
            "reset" => xr::Binding::new(&reset, path),
            "grip" => xr::Binding::new(&grip, path),
            _ => xr::Binding::new(&scroll, path),
        };
        // Suggests every binding the runtime accepts for `profile` (each is
        // checked on its own first, since one bad path rejects the whole set).
        let suggest = |profile: &str, wanted: &[(&str, &str)]| -> anyhow::Result<usize> {
            let profile_path = xr_.string_to_path(profile)?;
            let mut accepted = Vec::new();
            for hand in ["left", "right"] {
                for (action, input) in wanted {
                    let path = xr_.string_to_path(&format!("/user/hand/{hand}/input/{input}"))?;
                    if xr_
                        .suggest_interaction_profile_bindings(
                            profile_path,
                            &[binding(action, path)],
                        )
                        .is_ok()
                    {
                        accepted.push((*action, path));
                    }
                }
            }
            let list: Vec<_> = accepted.iter().map(|(a, p)| binding(a, *p)).collect();
            if !list.is_empty() {
                xr_.suggest_interaction_profile_bindings(profile_path, &list)?;
            }
            Ok(list.len())
        };
        let full = [
            ("aim", "aim/pose"),
            ("select", "trigger/click"),
            ("select", "trigger/value"),
            ("select", "a/click"),
            ("select", "x/click"),
            ("back", "b/click"),
            ("back", "y/click"),
            ("scroll", "thumbstick"),
            ("reset", "thumbstick/click"),
            ("grip", "squeeze/click"),
            ("grip", "squeeze/value"),
        ];
        for profile in [
            "/interaction_profiles/valve/frame_controller",
            "/interaction_profiles/valve/frame_controller_valve",
            "/interaction_profiles/valve/index_controller",
        ] {
            match suggest(profile, &full) {
                Ok(n) => eprintln!("Input: {profile}: {n} bindings"),
                Err(e) => eprintln!("Input: {profile} bindings not accepted: {e}"),
            }
        }
        suggest(
            "/interaction_profiles/khr/simple_controller",
            &[
                ("aim", "aim/pose"),
                ("select", "select/click"),
                ("back", "menu/click"),
            ],
        )?;
        ctx.session.attach_action_sets(&[&set])?;
        let spaces = [
            aim.create_space(&ctx.session, hands[0], xr::Posef::IDENTITY)?,
            aim.create_space(&ctx.session, hands[1], xr::Posef::IDENTITY)?,
        ];
        Ok(Self {
            set,
            aim,
            select,
            back,
            reset,
            grip,
            scroll,
            hands,
            spaces,
            // Direction: unit vector (rad/s-ish speeds); origin: metres.
            filters: [[OneEuro::new(3.0, 6.0), OneEuro::new(1.5, 0.6)]; 2],
        })
    }

    pub fn poll(
        &mut self,
        ctx: &XrContext,
        space: &xr::Space,
        time: xr::Time,
    ) -> anyhow::Result<InputState> {
        ctx.session.sync_actions(&[(&self.set).into()])?;
        let mut state = InputState::default();
        let pressed = |action: &xr::Action<bool>, hand: xr::Path| -> anyhow::Result<bool> {
            let s = action.state(&ctx.session, hand)?;
            Ok(s.is_active && s.current_state && s.changed_since_last_sync)
        };
        for (i, &hand) in self.hands.iter().enumerate() {
            state.select[i] = pressed(&self.select, hand)?;
            let held = self.select.state(&ctx.session, hand)?;
            state.select_held[i] = held.is_active && held.current_state;
            state.back |= pressed(&self.back, hand)?;
            state.reset |= pressed(&self.reset, hand)?;
            let grip = self.grip.state(&ctx.session, hand)?;
            state.grip |= grip.is_active && grip.current_state;
            let stick = self.scroll.state(&ctx.session, hand)?;
            if stick.is_active && stick.current_state.y.abs() > state.scroll.abs() {
                state.scroll = stick.current_state.y;
            }
            if self.aim.is_active(&ctx.session, hand)? {
                let location = self.spaces[i].locate(space, time)?;
                let flags = location.location_flags;
                if flags.contains(xr::SpaceLocationFlags::POSITION_VALID)
                    && flags.contains(xr::SpaceLocationFlags::ORIENTATION_VALID)
                {
                    let p = location.pose.position;
                    let t = time.as_nanos();
                    let origin = self.filters[i][0].filter([p.x, p.y, p.z], t);
                    let d = self.filters[i][1]
                        .filter(rotate(location.pose.orientation, [0.0, 0.0, -1.0]), t);
                    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
                    state.rays[i] = Some(Ray {
                        origin,
                        direction: [d[0] / len, d[1] / len, d[2] / len],
                    });
                } else {
                    self.filters[i][0].reset();
                    self.filters[i][1].reset();
                }
            }
        }
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_euro_smooths_jitter_but_follows_motion() {
        let mut f = OneEuro::new(3.0, 6.0);
        let step = 11_111_111; // 90 Hz
        // Holding still with ±0.004 noise (~0.25° on a unit vector).
        let mut max_dev = 0.0f32;
        for i in 0..200 {
            let noise = if i % 2 == 0 { 0.004 } else { -0.004 };
            let out = f.filter([noise, 0.0, -1.0], i * step);
            if i > 20 {
                max_dev = max_dev.max(out[0].abs());
            }
        }
        assert!(max_dev < 0.001, "jitter left: {max_dev}");
        // A fast turn is followed within a few frames.
        let mut out = [0.0; 3];
        for i in 200..210 {
            out = f.filter([0.5, 0.0, -0.8], i * step);
        }
        assert!((out[0] - 0.5).abs() < 0.05, "lagging: {out:?}");
    }

    #[test]
    fn rotation_turns_forward_vector() {
        // 90° yaw to the left (about +Y) maps -Z forward to -X.
        let half = std::f32::consts::FRAC_PI_4;
        let q = xr::Quaternionf {
            x: 0.0,
            y: half.sin(),
            z: 0.0,
            w: half.cos(),
        };
        let v = rotate(q, [0.0, 0.0, -1.0]);
        assert!(
            (v[0] + 1.0).abs() < 1e-5 && v[1].abs() < 1e-5 && v[2].abs() < 1e-5,
            "{v:?}"
        );
    }
}
