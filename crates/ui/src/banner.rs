//! Signature surfaces: the gradient wordmark, the no-args landing screen, the
//! branded version block, and the dev launch banner. These are the moments meant
//! to be screenshotted; each degrades to clean text off a styled terminal.

use std::time::Duration;

use crate::caps::Caps;
use crate::palette::Rgb;
use crate::{fmt, width};

pub const TAGLINE: &str = "Looks like a kitten. Runs like Rust.";
pub const HOMEPAGE: &str = "https://meow.style";

/// `paw meow`: paw in brand pink, wordmark in the brand gradient (bold).
pub fn wordmark(caps: &Caps) -> String {
    format!(
        "{} {}",
        caps.bold(Rgb::FLOSS, caps.glyphs.paw),
        caps.brand("meow")
    )
}

/// One command group on the landing screen.
pub struct CommandGroup<'a> {
    pub title: &'a str,
    pub commands: &'a [(&'a str, &'a str)],
}

/// The no-args landing screen: wordmark, tagline, grouped commands, footer.
pub fn landing(caps: &Caps, version: &str, groups: &[CommandGroup<'_>]) -> String {
    let mut out = String::new();
    out.push_str(&format!("  {}  {}\n", wordmark(caps), caps.dim(version)));
    out.push_str(&format!("  {}\n\n", caps.muted(TAGLINE)));
    out.push_str(&format!("  {}\n", caps.bold(Rgb::WHISKER, "USAGE")));
    out.push_str(&format!(
        "    {} {}\n",
        caps.bold(Rgb::FLOSS, "meow"),
        caps.muted("<command> [options]")
    ));
    out.push_str(&format!(
        "    {} {}\n",
        caps.bold(Rgb::FLOSS, "meow <file>"),
        caps.muted("run a file or URL directly")
    ));

    let name_w = groups
        .iter()
        .flat_map(|g| g.commands.iter())
        .map(|(n, _)| n.chars().count())
        .max()
        .unwrap_or(0);
    for group in groups {
        out.push_str(&format!("\n  {}\n", caps.bold(Rgb::WHISKER, group.title)));
        for (name, desc) in group.commands {
            out.push_str(&format!(
                "    {} {}\n",
                caps.bold(Rgb::FLOSS, &width::pad_end(name, name_w)),
                caps.muted(desc)
            ));
        }
    }
    out.push_str(&format!(
        "\n  {} {}\n",
        caps.muted("Learn more"),
        caps.link(HOMEPAGE, &caps.paint(Rgb::SKY, HOMEPAGE))
    ));
    out
}

/// The `--version` block: branded + multiline on a styled terminal, plain and
/// greppable (`meow <semver>` first) otherwise.
pub fn version_block(caps: &Caps, version: &str, extras: &[(&str, &str)]) -> String {
    if caps.styled() {
        let mut out = format!("{}  {}", wordmark(caps), caps.bold(Rgb::MILK, version));
        for (k, v) in extras {
            out.push_str(&format!("\n  {}  {}", caps.muted(k), caps.dim(v)));
        }
        out
    } else {
        let mut out = format!("meow {version}");
        for (k, v) in extras {
            out.push_str(&format!("\n{k} {v}"));
        }
        out
    }
}

/// The dev launch banner (stderr): wordmark + mode, then the target + meow's own
/// cold-start time. Honest: this is meow handing off, not the user's server
/// reaching ready.
pub fn dev_banner(
    caps: &Caps,
    version: &str,
    mode: &str,
    target: &str,
    elapsed: Duration,
) -> String {
    let g = caps.glyphs;
    let l1 = format!(
        "  {}  {}  {}",
        wordmark(caps),
        caps.dim(version),
        caps.muted(mode)
    );
    let boot = format!(
        "{} {}",
        caps.dim("\u{00B7} booted in"),
        caps.bold(Rgb::FLOSS, &fmt::duration(elapsed))
    );
    let l2 = format!(
        "  {} {} {} {}",
        caps.bold(Rgb::FLOSS, g.arrow),
        caps.muted("running"),
        caps.bold(Rgb::MILK, target),
        boot
    );
    format!("\n{l1}\n{l2}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wordmark_contains_the_name() {
        assert!(wordmark(&Caps::plain()).contains("meow"));
    }

    #[test]
    fn plain_version_is_greppable_first_line() {
        let v = version_block(&Caps::plain(), "0.0.0", &[("v8", "13.0")]);
        assert!(v.starts_with("meow 0.0.0"));
    }

    #[test]
    fn landing_lists_groups_and_commands() {
        let groups = [CommandGroup {
            title: "RUN",
            commands: &[("run", "execute a file")],
        }];
        let out = landing(&Caps::plain(), "0.0.0", &groups);
        assert!(out.contains("USAGE"));
        assert!(out.contains("RUN"));
        assert!(out.contains("run"));
        assert!(out.contains("meow.style"));
    }
}
