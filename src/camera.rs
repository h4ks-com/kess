use katui::game::{P2, V3, v3};

/// How far the player can swing the board around, so it stays on their side.
const MOST_YAW: f32 = 0.7;
/// The sprites look right within this range of heights around the render camera's.
const LOWEST_PITCH: f32 = 0.6;
const HIGHEST_PITCH: f32 = 0.85;
const NEAREST: f32 = 0.85;
const FARTHEST: f32 = 1.2;

/// Where the camera sits, as an orbit around a point on the board.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orbit {
    /// The side we look from: 0 behind white, pi behind black.
    pub home: f32,
    /// How far the player swung away from home.
    pub yaw: f32,
    /// The angle above the board.
    pub pitch: f32,
    /// How far we stand, as a factor of the render camera's distance.
    pub zoom: f32,
}

/// The render camera's numbers, from assets/layout.json.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lens {
    pub distance: f32,
    pub pitch: f32,
    pub target_y: f32,
    pub fov_x: f32,
    pub aspect: f32,
}

impl Lens {
    #[must_use]
    pub fn from_render(position: [f32; 3], target: [f32; 3], fov_x: f32, aspect: f32) -> Self {
        let (back, up) = (target[1] - position[1], position[2] - target[2]);
        Self {
            distance: back.hypot(up),
            pitch: up.atan2(back),
            target_y: target[1],
            fov_x,
            aspect,
        }
    }

    #[must_use]
    pub fn home(&self, white: bool) -> Orbit {
        Orbit {
            home: if white { 0.0 } else { std::f32::consts::PI },
            yaw: 0.0,
            pitch: self.pitch,
            zoom: 1.0,
        }
    }
}

impl Orbit {
    /// Swing by a mouse drag, kept within the range the sprites hold up in.
    #[must_use]
    pub fn swung(self, yaw: f32, pitch: f32) -> Self {
        Self {
            yaw: (self.yaw + yaw).clamp(-MOST_YAW, MOST_YAW),
            pitch: (self.pitch + pitch).clamp(LOWEST_PITCH, HIGHEST_PITCH),
            ..self
        }
    }

    #[must_use]
    pub fn zoomed(self, factor: f32) -> Self {
        Self {
            zoom: (self.zoom * factor).clamp(NEAREST, FARTHEST),
            ..self
        }
    }
}

/// A camera placed for one screen, which projects board points to pixels.
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub eye: V3,
    right: V3,
    up: V3,
    forward: V3,
    focal: f32,
    center: P2,
    /// One game pixel's width over its height.
    aspect: f32,
}

impl View {
    /// The camera `orbit` on a screen of `width` by `height` game pixels.
    #[must_use]
    pub fn new(lens: &Lens, orbit: Orbit, width: f32, height: f32, aspect: f32) -> Self {
        let angle = orbit.home + orbit.yaw;
        // the render looked a little behind the board's center, toward the near side
        let target = v3(
            -angle.sin() * lens.target_y,
            angle.cos() * lens.target_y,
            0.0,
        );
        let reach = lens.distance * orbit.zoom;
        let flat = reach * orbit.pitch.cos();
        let eye = target
            + v3(
                angle.sin() * flat,
                -angle.cos() * flat,
                reach * orbit.pitch.sin(),
            );
        let forward = (target - eye).normalized();
        let right = forward.cross(v3(0.0, 0.0, 1.0)).normalized();
        let up = right.cross(forward);
        // we fit the sprite render's frame inside the screen, so pieces and board share one scale
        let frame_height = (width * aspect / lens.aspect).min(height);
        let focal = frame_height / 2.0 / ((lens.fov_x / 2.0).tan() / lens.aspect);
        Self {
            eye,
            right,
            up,
            forward,
            focal,
            center: P2::new(width / 2.0, height / 2.0),
            aspect,
        }
    }

    /// `point` on screen, with its distance from the camera.
    #[must_use]
    pub fn project(&self, point: V3) -> Option<(P2, f32)> {
        let local = point - self.eye;
        let depth = local.dot(self.forward);
        if depth <= 0.01 {
            return None;
        }
        let x = local.dot(self.right) * self.focal / depth / self.aspect;
        let y = local.dot(self.up) * self.focal / depth;
        Some((P2::new(self.center.x + x, self.center.y - y), depth))
    }

    #[must_use]
    pub fn place(&self, point: V3) -> Option<P2> {
        self.project(point).map(|(spot, _)| spot)
    }

    /// The ray from the eye through screen pixel `spot`.
    #[must_use]
    pub fn ray(&self, spot: P2) -> V3 {
        let x = (spot.x - self.center.x) * self.aspect / self.focal;
        let y = (self.center.y - spot.y) / self.focal;
        (self.forward + self.right * x + self.up * y).normalized()
    }

    /// Where the ray through `spot` meets the plane `z = height`.
    #[must_use]
    pub fn on_plane(&self, spot: P2, height: f32) -> Option<V3> {
        let ray = self.ray(spot);
        if ray.z.abs() < 1e-5 {
            return None;
        }
        let along = (height - self.eye.z) / ray.z;
        (along > 0.0).then(|| self.eye + ray * along)
    }

    /// Screen pixels one board unit of height spans at `point`.
    #[must_use]
    pub fn unit(&self, point: V3) -> f32 {
        match (self.place(point), self.place(point + v3(0.0, 0.0, 1.0))) {
            (Some(foot), Some(head)) => foot.y - head.y,
            _ => 0.0,
        }
    }

    /// Screen pixels one board unit facing the camera spans at `point`,
    /// which only depends on how far away it is.
    #[must_use]
    pub fn breadth(&self, point: V3) -> f32 {
        let depth = (point - self.eye).dot(self.forward);
        if depth <= 0.01 {
            0.0
        } else {
            self.focal / depth
        }
    }
}

impl Lens {
    /// How a sprite rendered at the board's center measures across
    /// against up: what turns a sprite's rendered unit of height into its
    /// size at any distance.
    #[must_use]
    pub fn breadth_per_unit(&self) -> f32 {
        let view = View::new(self, self.home(true), 1600.0, 1000.0, 1.0);
        let center = v3(0.0, 0.0, 0.0);
        view.breadth(center) / view.unit(center).max(1e-3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lens() -> Lens {
        Lens::from_render(
            [0.0, -11.6, 9.6],
            [0.0, -0.5, 0.0],
            2.0 * (18.0_f32 / 40.0).atan(),
            1.6,
        )
    }

    #[test]
    fn white_sees_rank_one_near_and_black_sees_it_far() {
        let lens = lens();
        let white = View::new(&lens, lens.home(true), 1600.0, 1000.0, 1.0);
        let black = View::new(&lens, lens.home(false), 1600.0, 1000.0, 1.0);
        let near = |view: &View, y: f32| view.place(v3(0.0, y, 0.0)).unwrap().y;
        assert!(near(&white, -3.5) > near(&white, 3.5));
        assert!(near(&black, 3.5) > near(&black, -3.5));
        let a1 = white.place(v3(-3.5, -3.5, 0.0)).unwrap();
        assert!(a1.x < 800.0, "a1 sits on white's left");
    }

    #[test]
    fn a_ray_through_a_projected_point_lands_back_on_it() {
        let lens = lens();
        let view = View::new(&lens, lens.home(true).swung(0.3, 0.1), 1200.0, 700.0, 0.5);
        let point = v3(1.25, -2.5, 0.0);
        let back = view.on_plane(view.place(point).unwrap(), 0.0).unwrap();
        assert!((back - point).dot(back - point) < 1e-4);
    }

    #[test]
    fn a_far_piece_shrinks_by_distance_and_a_center_piece_matches_its_render() {
        let lens = lens();
        let view = View::new(&lens, lens.home(true), 1600.0, 1000.0, 1.0);
        let center = v3(0.0, 0.0, 0.0);
        let render_unit = 120.0;
        let size = |point: V3| view.breadth(point) / (render_unit * lens.breadth_per_unit());
        assert!((size(center) * render_unit - view.unit(center)).abs() < 0.01);
        let (near, far) = (v3(0.0, -3.5, 0.0), v3(0.0, 3.5, 0.0));
        let ratio = size(far) / size(near);
        let squares = view.breadth(far) / view.breadth(near);
        assert!(
            (ratio - squares).abs() < 1e-4,
            "pieces shrink like the squares across"
        );
        assert!(
            view.unit(far) / view.unit(near) > ratio,
            "the old height scale grew far pieces"
        );
    }

    #[test]
    fn swinging_stays_in_range() {
        let lens = lens();
        let orbit = lens.home(true).swung(9.0, 9.0).zoomed(9.0);
        assert_eq!(
            (orbit.yaw, orbit.pitch, orbit.zoom),
            (MOST_YAW, HIGHEST_PITCH, FARTHEST)
        );
    }
}
