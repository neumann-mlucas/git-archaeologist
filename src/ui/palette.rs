use ratatui::style::Color;

const DEFAULT: &[Color] = &[
    Color::Cyan,
    Color::Yellow,
    Color::Green,
    Color::Magenta,
    Color::Blue,
    Color::Red,
    Color::LightCyan,
    Color::LightYellow,
    Color::LightGreen,
    Color::LightMagenta,
    Color::LightBlue,
    Color::LightRed,
];

/// Deterministic color for a group label — same label always gets same color
/// within a run, so chart series and breakdown rows stay in sync.
pub fn color_for(group: &str) -> Color {
    let hash = fxhash(group);
    DEFAULT[(hash as usize) % DEFAULT.len()]
}

fn fxhash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
