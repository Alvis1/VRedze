//! Controller input: an aim ray per hand, select (trigger), back (B),
//! play/pause (A) and scroll (thumbstick). Bindings are suggested for the
//! Steam Frame controller, Index controllers and the generic simple profile.

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
    pub pause: bool,
    /// Thumbstick vertical deflection (-1..1, up positive), strongest hand.
    pub scroll: f32,
}

pub struct Input {
    set: xr::ActionSet,
    aim: xr::Action<xr::Posef>,
    select: xr::Action<bool>,
    back: xr::Action<bool>,
    pause: xr::Action<bool>,
    scroll: xr::Action<xr::Vector2f>,
    hands: [xr::Path; 2],
    spaces: [xr::Space; 2],
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
        let pause = set.create_action::<bool>("pause", "Play / pause", &hands)?;
        let scroll = set.create_action::<xr::Vector2f>("scroll", "Scroll", &hands)?;

        let binding = |action: &str, path: xr::Path| match action {
            "aim" => xr::Binding::new(&aim, path),
            "select" => xr::Binding::new(&select, path),
            "back" => xr::Binding::new(&back, path),
            "pause" => xr::Binding::new(&pause, path),
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
                        .suggest_interaction_profile_bindings(profile_path, &[binding(action, path)])
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
            ("back", "b/click"),
            ("back", "y/click"),
            ("pause", "a/click"),
            ("pause", "x/click"),
            ("scroll", "thumbstick"),
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
            &[("aim", "aim/pose"), ("select", "select/click"), ("back", "menu/click")],
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
            pause,
            scroll,
            hands,
            spaces,
        })
    }

    pub fn poll(
        &self,
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
            state.pause |= pressed(&self.pause, hand)?;
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
                    state.rays[i] = Some(Ray {
                        origin: [p.x, p.y, p.z],
                        direction: rotate(location.pose.orientation, [0.0, 0.0, -1.0]),
                    });
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
