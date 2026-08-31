#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringConstant {
    pub tension: f64,
    pub friction: f64,
    pub inertia: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DampedSpring {
    pub position: f64,
    pub velocity: f64,
}

impl DampedSpring {
    pub fn new(position: f64) -> Self {
        Self {
            position,
            velocity: 0.0,
        }
    }

    pub fn snap(&mut self, value: f64) {
        self.position = value;
        self.velocity = 0.0;
    }

    pub fn step(&mut self, target: f64, constant: SpringConstant, dt: f64) {
        if constant.inertia <= 0.0 {
            self.snap(target);
            return;
        }
        let acceleration = (constant.tension * (target - self.position)
            - constant.friction * self.velocity)
            / constant.inertia;
        self.velocity += acceleration * dt;
        self.position += self.velocity * dt;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semi_implicit_step_matches_swift_equation() {
        let mut spring = DampedSpring::new(0.0);
        spring.step(
            1.0,
            SpringConstant {
                tension: 300.0,
                friction: 30.0,
                inertia: 3.0,
            },
            0.1,
        );
        assert!((spring.velocity - 10.0).abs() < 1e-12);
        assert!((spring.position - 1.0).abs() < 1e-12);
    }
}
