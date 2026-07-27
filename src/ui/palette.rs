use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteKind {
    Default,
    Colorblind,
    Mono,
}

impl PaletteKind {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "colorblind" | "cb" => PaletteKind::Colorblind,
            "mono" | "monochrome" => PaletteKind::Mono,
            _ => PaletteKind::Default,
        }
    }

    fn colors(self) -> &'static [Color] {
        match self {
            PaletteKind::Default => DEFAULT,
            PaletteKind::Colorblind => COLORBLIND,
            PaletteKind::Mono => MONO,
        }
    }
}

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

/// Wong / Okabe-Ito inspired: safe for the common CVD types.
const COLORBLIND: &[Color] = &[
    Color::Rgb(0, 114, 178),   // blue
    Color::Rgb(230, 159, 0),   // orange
    Color::Rgb(0, 158, 115),   // bluish-green
    Color::Rgb(240, 228, 66),  // yellow
    Color::Rgb(86, 180, 233),  // sky blue
    Color::Rgb(213, 94, 0),    // vermillion
    Color::Rgb(204, 121, 167), // reddish-purple
    Color::Rgb(0, 0, 0),       // black
];

const MONO: &[Color] = &[
    Color::White,
    Color::Gray,
    Color::DarkGray,
    Color::LightYellow,
    Color::Yellow,
];

pub fn color_for(group: &str, kind: PaletteKind) -> Color {
    let colors = kind.colors();
    let hash = fxhash(group);
    colors[(hash as usize) % colors.len()]
}

fn fxhash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
