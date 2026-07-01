//! Glyph set with an ASCII fallback. Unicode terminals get rounded boxes, paws,
//! and check/cross marks; legacy or `dumb` terminals get clean ASCII so output is
//! never mojibake. Chosen once from `Caps::unicode` and threaded into renderers.

#[derive(Debug, Clone, Copy)]
pub struct Glyphs {
    pub ok: &'static str,
    pub err: &'static str,
    pub warn: &'static str,
    pub info: &'static str,
    pub arrow: &'static str,
    pub bullet: &'static str,
    pub paw: &'static str,
    pub purr_face: &'static str,
    pub hiss_face: &'static str,
    pub pounce_face: &'static str,
    pub top_left: &'static str,
    pub top_right: &'static str,
    pub bottom_left: &'static str,
    pub bottom_right: &'static str,
    pub horizontal: &'static str,
    pub vertical: &'static str,
    pub tee_right: &'static str,
    pub tee_left: &'static str,
    pub bar_full: &'static str,
    pub bar_empty: &'static str,
    pub gutter_down: &'static str,
    pub gutter_up: &'static str,
    pub caret: &'static str,
    pub spinner: &'static [&'static str],
}

const UNICODE_SPINNER: [&str; 10] = [
    "\u{280B}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283C}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280F}",
];
const ASCII_SPINNER: [&str; 4] = ["|", "/", "-", "\\"];

impl Glyphs {
    pub const UNICODE: Glyphs = Glyphs {
        ok: "\u{2713}",
        err: "\u{2717}",
        warn: "\u{25B2}",
        info: "\u{2022}",
        arrow: "\u{2192}",
        bullet: "\u{00B7}",
        paw: "\u{1F43E}",
        purr_face: "\u{1F638}",
        hiss_face: "\u{1F640}",
        pounce_face: "\u{1F43E}",
        top_left: "\u{256D}",
        top_right: "\u{256E}",
        bottom_left: "\u{2570}",
        bottom_right: "\u{256F}",
        horizontal: "\u{2500}",
        vertical: "\u{2502}",
        tee_right: "\u{251C}",
        tee_left: "\u{2524}",
        bar_full: "\u{2588}",
        bar_empty: "\u{2591}",
        gutter_down: "\u{256D}",
        gutter_up: "\u{2570}",
        caret: "^",
        spinner: &UNICODE_SPINNER,
    };

    pub const ASCII: Glyphs = Glyphs {
        ok: "+",
        err: "x",
        warn: "!",
        info: "*",
        arrow: "->",
        bullet: "-",
        paw: ":3",
        purr_face: ":3",
        hiss_face: ">:(",
        pounce_face: ":3",
        top_left: "+",
        top_right: "+",
        bottom_left: "+",
        bottom_right: "+",
        horizontal: "-",
        vertical: "|",
        tee_right: "+",
        tee_left: "+",
        bar_full: "#",
        bar_empty: "-",
        gutter_down: "/",
        gutter_up: "\\",
        caret: "^",
        spinner: &ASCII_SPINNER,
    };

    pub const fn select(unicode: bool) -> Glyphs {
        if unicode {
            Glyphs::UNICODE
        } else {
            Glyphs::ASCII
        }
    }
}
