use console::{Alignment, Term};
use indicatif::{HumanBytes, ProgressStyle};
use std::sync::OnceLock;

/// Width of the right hand column when the terminal is wide enough.
const MSG_WIDTH: usize = 52;
/// Narrowest message column we render.
const MIN_MSG_WIDTH: usize = 11;
/// Narrowest bar we are willing to draw.
const MIN_BAR_WIDTH: usize = 10;
/// Widest bar we are willing to draw.
const MAX_BAR_WIDTH: usize = 60;
/// Columns taken by the spinner and the spaces surrounding the bar.
const CHROME_WIDTH: usize = 3;
/// Column holding the transferred amount.
const BYTES_WIDTH: usize = 11;
/// Column holding the total amount.
const TOTAL_WIDTH: usize = 11;
/// Column holding the transfer rate.
const SPEED_WIDTH: usize = 12;
/// Column holding the completion percentage.
const PERCENT_WIDTH: usize = 4;
/// Column holding the estimated time left.
const ETA_WIDTH: usize = 6;

fn term_width() -> usize {
    Term::stderr().size().1 as usize
}

/// Width of the right hand column, adapted to the terminal.
pub fn msg_width() -> usize {
    static WIDTH: OnceLock<usize> = OnceLock::new();
    *WIDTH.get_or_init(|| {
        let term = term_width();
        MSG_WIDTH
            .min(term.saturating_sub(CHROME_WIDTH + MIN_BAR_WIDTH))
            .max(MIN_MSG_WIDTH)
    })
}

/// Width of the bar. Resolved once so that every bar of a run has the same length.
pub fn bar_width() -> usize {
    static WIDTH: OnceLock<usize> = OnceLock::new();
    *WIDTH.get_or_init(|| {
        let term = term_width();
        let avail = term.saturating_sub(CHROME_WIDTH + msg_width());
        avail.clamp(MIN_BAR_WIDTH, MAX_BAR_WIDTH)
    })
}

/// Grow a text to exactly `width` columns, never cutting it.
fn fit(s: &str, width: usize) -> String {
    console::pad_str(s, width, Alignment::Left, None).into_owned()
}

/// Human readable byte count that never grows past `max` columns.
fn human_bytes(value: u64, max: usize) -> String {
    let text = HumanBytes(value).to_string();
    if console::measure_text_width(&text) <= max {
        return text;
    }
    if let Some((int, rest)) = text.split_once('.') {
        let unit = rest.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
        let shorter = format!("{int}{unit}");
        if console::measure_text_width(&shorter) <= max {
            return shorter;
        }
    }
    text
}

/// Pad or shorten a status text so the column keeps a constant width.
pub fn pad_msg(msg: &str) -> String {
    let width = msg_width();
    if console::measure_text_width(msg) > width {
        console::pad_str(msg, width, Alignment::Left, Some("…")).into_owned()
    } else {
        fit(msg, width)
    }
}

/// Style shared by every progress bar: fixed width bar, fixed width message.
pub fn bar_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template(&format!(
            "{{spinner:.green}} {{bar:<{w}.green}} {{msg}}",
            w = bar_width()
        ))
        .expect("valid progress template")
        .progress_chars("##-")
}

/// Remaining time in a compact form, or `?` when it cannot be estimated.
fn format_eta(pos: u64, total: u64, per_sec: f64) -> String {
    if !per_sec.is_finite() || per_sec <= 0.0 || pos == 0 || pos >= total {
        return "?".to_string();
    }
    let secs = ((total - pos) as f64 / per_sec).round() as u64;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Transfer statistics in human readable form, laid out in fixed width columns.
///
/// Unknown values are shown as `?` so the columns never move.
pub fn stats_line(pos: u64, total: Option<u64>, per_sec: f64) -> String {
    let width = msg_width();
    let per_sec = if per_sec.is_finite() { per_sec } else { 0.0 };

    let bytes = console::pad_str(
        &human_bytes(pos, BYTES_WIDTH),
        BYTES_WIDTH,
        Alignment::Right,
        None,
    )
    .into_owned();
    let speed = console::pad_str(
        &format!("{}/s", human_bytes(per_sec as u64, SPEED_WIDTH - 3)),
        SPEED_WIDTH,
        Alignment::Right,
        None,
    )
    .into_owned();

    if width < 24 {
        return fit(&bytes, width);
    }
    if width < 41 {
        return fit(&format!("{bytes} {speed}"), width);
    }

    let (total_s, percent) = match total {
        Some(t) if t > 0 => (
            console::pad_str(
                &human_bytes(t, TOTAL_WIDTH),
                TOTAL_WIDTH,
                Alignment::Right,
                None,
            )
            .into_owned(),
            console::pad_str(
                &format!("{:.0}%", pos as f64 / t as f64 * 100.0),
                PERCENT_WIDTH,
                Alignment::Right,
                None,
            )
            .into_owned(),
        ),
        _ => (fit("?", TOTAL_WIDTH), fit("?", PERCENT_WIDTH)),
    };

    if width < MSG_WIDTH {
        return fit(&format!("{bytes}/{total_s} {percent} {speed}"), width);
    }

    let eta = match total {
        Some(t) if t > 0 => console::pad_str(
            &format_eta(pos, t, per_sec),
            ETA_WIDTH,
            Alignment::Right,
            None,
        )
        .into_owned(),
        _ => fit("?", ETA_WIDTH),
    };
    fit(
        &format!("{bytes}/{total_s} {percent} {speed} ETA {eta}"),
        width,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatif::{ProgressBar, ProgressDrawTarget, TermLike};
    use std::io;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone, Default)]
    struct Capture {
        chunks: Arc<Mutex<Vec<String>>>,
    }

    impl Capture {
        /// Last chunk holding visible content, ignoring the padding spaces
        /// indicatif appends to fill the terminal width.
        fn last_line(&self) -> Option<String> {
            self.chunks
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find(|c| c.chars().any(|ch| !ch.is_whitespace()))
                .cloned()
        }
    }

    impl TermLike for Capture {
        fn width(&self) -> u16 {
            80
        }
        fn height(&self) -> u16 {
            20
        }
        fn move_cursor_up(&self, _: usize) -> io::Result<()> {
            Ok(())
        }
        fn move_cursor_down(&self, _: usize) -> io::Result<()> {
            Ok(())
        }
        fn move_cursor_right(&self, _: usize) -> io::Result<()> {
            Ok(())
        }
        fn move_cursor_left(&self, _: usize) -> io::Result<()> {
            Ok(())
        }
        fn write_line(&self, s: &str) -> io::Result<()> {
            self.chunks.lock().unwrap().push(s.to_string());
            Ok(())
        }
        fn write_str(&self, s: &str) -> io::Result<()> {
            self.chunks.lock().unwrap().push(s.to_string());
            Ok(())
        }
        fn clear_line(&self) -> io::Result<()> {
            Ok(())
        }
        fn flush(&self) -> io::Result<()> {
            Ok(())
        }
    }

    fn render(msg: &str, len: Option<u64>, pos: u64) -> String {
        let cap = Capture::default();
        let pb = ProgressBar::with_draw_target(
            len,
            ProgressDrawTarget::term_like(Box::new(cap.clone())),
        );
        pb.set_style(bar_style());
        pb.set_message(pad_msg(msg));
        pb.set_position(pos);
        pb.tick();
        cap.last_line().expect("rendered line")
    }

    #[test]
    fn bars_keep_a_constant_width() {
        let short = render("apt main: deb.debian.org", Some(100), 10);
        let long = render(
            "apt main: https://deb.debian.org/debian bookworm",
            Some(100),
            90,
        );
        let width = console::measure_text_width(&short);
        assert_eq!(width, console::measure_text_width(&long));
        assert_eq!(width, msg_width() + bar_width() + 3);
        assert!(width <= 80);
    }

    #[test]
    fn stats_line_has_a_constant_width() {
        let known = stats_line(1234, Some(10000), 4096.0);
        let unknown = stats_line(1234, None, 4096.0);
        assert_eq!(
            console::measure_text_width(&known),
            console::measure_text_width(&unknown)
        );
        assert_eq!(console::measure_text_width(&known), msg_width());
    }

    #[test]
    fn pad_msg_never_grows_past_the_column() {
        let exact = "x".repeat(msg_width());
        assert_eq!(console::measure_text_width(&pad_msg(&exact)), msg_width());
        let long = "y".repeat(msg_width() + 20);
        assert_eq!(console::measure_text_width(&pad_msg(&long)), msg_width());
    }

    #[test]
    fn huge_values_stay_inside_their_column() {
        let line = stats_line(u64::MAX / 4, Some(u64::MAX), f64::MAX);
        assert_eq!(console::measure_text_width(&line), msg_width());
    }
}
