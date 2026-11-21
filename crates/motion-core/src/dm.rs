//! Maths that gives the same bits on every machine: `libm`'s, where the
//! standard library's go to the C library (which picks its code by CPU).
//! Everything a frame or a bake is worked out from goes through these.

pub fn sin(x: f64) -> f64 {
    libm::sin(x)
}
pub fn cos(x: f64) -> f64 {
    libm::cos(x)
}
pub fn sin_cos(x: f64) -> (f64, f64) {
    (libm::sin(x), libm::cos(x))
}
pub fn tan(x: f64) -> f64 {
    libm::tan(x)
}
pub fn exp(x: f64) -> f64 {
    libm::exp(x)
}
pub fn exp2(x: f64) -> f64 {
    libm::exp2(x)
}
pub fn ln(x: f64) -> f64 {
    libm::log(x)
}
pub fn log2(x: f64) -> f64 {
    libm::log2(x)
}
pub fn acos(x: f64) -> f64 {
    libm::acos(x)
}
pub fn asin(x: f64) -> f64 {
    libm::asin(x)
}
pub fn powf(x: f64, y: f64) -> f64 {
    libm::pow(x, y)
}
pub fn atan2(y: f64, x: f64) -> f64 {
    libm::atan2(y, x)
}
/// (Multiplies, an add and a square root are the same bits everywhere.)
pub fn hypot(x: f64, y: f64) -> f64 {
    (x * x + y * y).sqrt()
}
