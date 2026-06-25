//! Brand palette + color downsampling.
//!
//! Truecolor hexes come straight from meow.style (hot pink, magenta, violet)
//! plus a semantic set. Every color downsamples cleanly to xterm-256 and the
//! basic 16 so no surface depends on truecolor being present; it just gets
//! richer when the terminal can show it.

/// A 24-bit color. Brand colors live as associated constants so call sites read
/// as intent (`Rgb::FLOSS`) rather than raw channel triples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    /// Primary brand accent, hot pink (`#FF4D9D`).
    pub const FLOSS: Rgb = Rgb::new(0xFF, 0x4D, 0x9D);
    /// Gradient mid, magenta (`#F92C8B`).
    pub const MAGENTA: Rgb = Rgb::new(0xF9, 0x2C, 0x8B);
    /// Gradient end / deep accent, violet (`#A06CF5`).
    pub const VIOLET: Rgb = Rgb::new(0xA0, 0x6C, 0xF5);

    /// Success (`#98FF98`).
    pub const CATNIP: Rgb = Rgb::new(0x98, 0xFF, 0x98);
    /// Error (`#FF5C72`).
    pub const HISS: Rgb = Rgb::new(0xFF, 0x5C, 0x72);
    /// Warning (`#FFC55C`).
    pub const HONEY: Rgb = Rgb::new(0xFF, 0xC5, 0x5C);
    /// Info / action (`#7CC7FF`).
    pub const SKY: Rgb = Rgb::new(0x7C, 0xC7, 0xFF);
    /// Muted ink: frames, gutters, secondary text (`#8A8A99`).
    pub const WHISKER: Rgb = Rgb::new(0x8A, 0x8A, 0x99);
    /// Faint ink: dim separators (`#5A5A6A`).
    pub const SMOKE: Rgb = Rgb::new(0x5A, 0x5A, 0x6A);
    /// Near-white body text (`#ECECF2`).
    pub const MILK: Rgb = Rgb::new(0xEC, 0xEC, 0xF2);
}

/// The signature wordmark / banner gradient: hot-pink to magenta to violet.
pub const BRAND_GRADIENT: [Rgb; 3] = [Rgb::FLOSS, Rgb::MAGENTA, Rgb::VIOLET];

/// Linear interpolation between two colors.
pub fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| -> u8 { (x as f32 + (y as f32 - x as f32) * t).round() as u8 };
    Rgb::new(mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b))
}

/// Sample a multi-stop gradient at `t` in `[0, 1]`.
pub fn sample(stops: &[Rgb], t: f32) -> Rgb {
    match stops.len() {
        0 => Rgb::MILK,
        1 => stops[0],
        n => {
            let t = t.clamp(0.0, 1.0);
            let scaled = t * (n - 1) as f32;
            let idx = (scaled.floor() as usize).min(n - 2);
            lerp(stops[idx], stops[idx + 1], scaled - idx as f32)
        }
    }
}

/// Nearest xterm-256 index for a truecolor value.
pub fn to_ansi256(c: Rgb) -> u8 {
    let max = c.r.max(c.g).max(c.b);
    let min = c.r.min(c.g).min(c.b);
    if max.saturating_sub(min) < 8 {
        if max < 8 {
            return 16;
        }
        if max > 248 {
            return 231;
        }
        return 232 + (((max as u16 - 8) * 24) / 247) as u8;
    }
    let level = |v: u8| -> u16 {
        const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        let mut best = 0usize;
        let mut best_d = i32::MAX;
        for (i, l) in LEVELS.iter().enumerate() {
            let d = (v as i32 - *l as i32).abs();
            if d < best_d {
                best_d = d;
                best = i;
            }
        }
        best as u16
    };
    (16 + 36 * level(c.r) + 6 * level(c.g) + level(c.b)) as u8
}

/// Nearest basic-16 SGR foreground code (`30..=37`, `90..=97`).
pub fn to_ansi16(c: Rgb) -> u8 {
    const TABLE: [(u8, Rgb); 16] = [
        (30, Rgb::new(0, 0, 0)),
        (31, Rgb::new(205, 49, 49)),
        (32, Rgb::new(13, 188, 121)),
        (33, Rgb::new(229, 229, 16)),
        (34, Rgb::new(36, 114, 200)),
        (35, Rgb::new(188, 63, 188)),
        (36, Rgb::new(17, 168, 205)),
        (37, Rgb::new(229, 229, 229)),
        (90, Rgb::new(102, 102, 102)),
        (91, Rgb::new(241, 76, 76)),
        (92, Rgb::new(35, 209, 139)),
        (93, Rgb::new(245, 245, 67)),
        (94, Rgb::new(59, 142, 234)),
        (95, Rgb::new(214, 112, 214)),
        (96, Rgb::new(41, 184, 219)),
        (97, Rgb::new(255, 255, 255)),
    ];
    let mut best = TABLE[0].0;
    let mut best_d = i64::MAX;
    for (code, p) in TABLE {
        let dr = c.r as i64 - p.r as i64;
        let dg = c.g as i64 - p.g as i64;
        let db = c.b as i64 - p.b as i64;
        let d = dr * dr + dg * dg + db * db;
        if d < best_d {
            best_d = d;
            best = code;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_hits_endpoints() {
        assert_eq!(sample(&BRAND_GRADIENT, 0.0), Rgb::FLOSS);
        assert_eq!(sample(&BRAND_GRADIENT, 1.0), Rgb::VIOLET);
    }

    #[test]
    fn grey_uses_the_grey_ramp() {
        assert!((232..=255).contains(&to_ansi256(Rgb::new(128, 128, 128))));
    }

    #[test]
    fn brand_pink_is_a_cube_color() {
        assert!((16..=231).contains(&to_ansi256(Rgb::FLOSS)));
    }

    #[test]
    fn nearest16_picks_a_magenta_for_floss() {
        let code = to_ansi16(Rgb::FLOSS);
        assert!(code == 35 || code == 95);
    }
}
