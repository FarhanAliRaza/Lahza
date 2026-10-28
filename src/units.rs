//! Display formatting for times, zoom, speed and percentages, so every
//! readout in the editor spells units the same way.

/// Duration in seconds with one decimal: "7.0 s".
pub(crate) fn fmt_seconds(seconds: f64) -> String {
    format!("{:.1} s", seconds.max(0.0))
}

/// Time span: "0.4 s – 4.2 s".
pub(crate) fn fmt_seconds_range(start: f64, end: f64) -> String {
    format!("{:.1} s – {:.1} s", start.max(0.0), end.max(0.0))
}

/// Zoom or playback speed multiplier: "2.0×".
pub(crate) fn fmt_zoom(factor: f64) -> String {
    format!("{factor:.1}×")
}

/// Playhead time: "0:02.00".
pub(crate) fn timecode(seconds: f64) -> String {
    let centis = if seconds.is_finite() { (seconds.max(0.0) * 100.0).round() as u64 } else { 0 };
    let whole = centis / 100;
    if whole >= 3_600 {
        format!("{}:{:02}:{:02}.{:02}", whole / 3_600, whole / 60 % 60, whole % 60, centis % 100)
    } else {
        format!("{}:{:02}.{:02}", whole / 60, whole % 60, centis % 100)
    }
}

/// Ruler tick label: "0:02", or "0:02.5" when ticks are under a second
/// apart. Every label at one step has the same shape, so widths hold still.
pub(crate) fn ruler_time(seconds: f64, fractional: bool) -> String {
    let tenths = if seconds.is_finite() { (seconds.max(0.0) * 10.0).round() as u64 } else { 0 };
    let whole = tenths / 10;
    let clock = if whole >= 3_600 {
        format!("{}:{:02}:{:02}", whole / 3_600, whole / 60 % 60, whole % 60)
    } else {
        format!("{}:{:02}", whole / 60, whole % 60)
    };
    if fractional { format!("{clock}.{}", tenths % 10) } else { clock }
}

/// Whole percent with no space: "40%".
pub(crate) fn percent(fraction: f64) -> String {
    format!("{:.0}%", (fraction * 100.0).round())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_units() {
        assert_eq!(fmt_seconds(7.0), "7.0 s");
        assert_eq!(fmt_seconds_range(0.4, 4.2), "0.4 s – 4.2 s");
        assert_eq!(fmt_zoom(2.0), "2.0×");
        assert_eq!(timecode(2.0), "0:02.00");
        assert_eq!(timecode(83.456), "1:23.46");
        assert_eq!(timecode(3_661.5), "1:01:01.50");
        assert_eq!(percent(0.4), "40%");
        assert_eq!(ruler_time(2.0, false), "0:02");
        assert_eq!(ruler_time(1.999_999, false), "0:02");
        assert_eq!(ruler_time(2.5, true), "0:02.5");
        assert_eq!(ruler_time(3_602.0, false), "1:00:02");
    }
}
