//! Display-width measurement + fitting. ANSI escapes (CSI colors and OSC-8
//! hyperlinks) count as zero width; combining marks and zero-width joiners count
//! as zero; CJK and emoji count as two. Good enough to keep box and table
//! right-edges aligned across everything the CLI renders.

/// Visible column width of `s`, ignoring ANSI escapes.
pub fn width(s: &str) -> usize {
    let mut w = 0usize;
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7E}').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        w += char_width(ch);
    }
    w
}

fn char_width(ch: char) -> usize {
    let cp = ch as u32;
    if matches!(cp,
        0x0300..=0x036F | 0x200B..=0x200F | 0xFE00..=0xFE0F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF)
    {
        return 0;
    }
    let wide = matches!(cp,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF |
        0x4E00..=0x9FFF | 0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF |
        0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 |
        0x1F300..=0x1FAFF | 0x1F000..=0x1F0FF);
    if wide {
        2
    } else {
        1
    }
}

/// Truncate `s` to at most `max` display columns, appending `\u{2026}` when cut.
/// ANSI-naive: intended for plain labels, not pre-colored strings.
pub fn truncate(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = char_width(ch);
        if w + cw > max.saturating_sub(1) {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('\u{2026}');
    out
}

/// Right-pad `s` with spaces to `cols` display columns (ANSI-aware width).
pub fn pad_end(s: &str, cols: usize) -> String {
    let w = width(s);
    if w >= cols {
        return s.to_owned();
    }
    format!("{s}{}", " ".repeat(cols - w))
}

/// Left-pad `s` with spaces to `cols` display columns (ANSI-aware width).
pub fn pad_start(s: &str, cols: usize) -> String {
    let w = width(s);
    if w >= cols {
        return s.to_owned();
    }
    format!("{}{s}", " ".repeat(cols - w))
}

/// Center `s` within `cols` columns.
pub fn center(s: &str, cols: usize) -> String {
    let w = width(s);
    if w >= cols {
        return s.to_owned();
    }
    let total = cols - w;
    let left = total / 2;
    format!("{}{s}{}", " ".repeat(left), " ".repeat(total - left))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_is_zero_width() {
        assert_eq!(width("\x1b[38;2;255;77;157mmeow\x1b[0m"), 4);
    }

    #[test]
    fn osc8_link_counts_only_the_label() {
        let link = "\x1b]8;;https://meow.style\x1b\\meow\x1b]8;;\x1b\\";
        assert_eq!(width(link), 4);
    }

    #[test]
    fn emoji_is_two_columns() {
        assert_eq!(width("\u{1F43E}"), 2);
    }

    #[test]
    fn truncate_adds_ellipsis() {
        assert_eq!(truncate("abcdef", 4), "abc\u{2026}");
        assert_eq!(truncate("abc", 4), "abc");
    }

    #[test]
    fn pad_end_accounts_for_wide_glyphs() {
        assert_eq!(width(&pad_end("\u{1F43E}", 5)), 5);
    }
}
