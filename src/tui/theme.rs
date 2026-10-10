//! Colors. 24-bit when the terminal says it supports it, the 16 named colors otherwise,
//! and none at all with `NO_COLOR`.

use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy)]
pub struct Theme {
    pub accent: Color,
    pub safe: Color,
    pub review: Color,
    pub danger: Color,
    pub dim: Color,
    pub text: Color,
    pub border: Color,
    pub highlight_bg: Color,
    pub gauge_bg: Color,
    pub plain: bool,
}

impl Theme {
    pub fn detect() -> Theme {
        if std::env::var_os("NO_COLOR").is_some() {
            return Theme {
                accent: Color::Reset,
                safe: Color::Reset,
                review: Color::Reset,
                danger: Color::Reset,
                dim: Color::Reset,
                text: Color::Reset,
                border: Color::Reset,
                highlight_bg: Color::Reset,
                gauge_bg: Color::Reset,
                plain: true,
            };
        }
        let truecolor = std::env::var("COLORTERM")
            .is_ok_and(|v| v.contains("truecolor") || v.contains("24bit"));
        if truecolor {
            Theme {
                accent: Color::Rgb(125, 211, 252),
                safe: Color::Rgb(134, 239, 172),
                review: Color::Rgb(252, 211, 77),
                danger: Color::Rgb(248, 113, 113),
                dim: Color::Rgb(115, 125, 140),
                text: Color::Rgb(226, 232, 240),
                border: Color::Rgb(71, 85, 105),
                highlight_bg: Color::Rgb(30, 41, 59),
                gauge_bg: Color::Rgb(40, 48, 62),
                plain: false,
            }
        } else {
            Theme {
                accent: Color::Cyan,
                safe: Color::Green,
                review: Color::Yellow,
                danger: Color::Red,
                dim: Color::DarkGray,
                text: Color::Reset,
                border: Color::DarkGray,
                highlight_bg: Color::DarkGray,
                gauge_bg: Color::Black,
                plain: false,
            }
        }
    }

    pub fn fg(&self, c: Color) -> Style {
        if self.plain {
            Style::new()
        } else {
            Style::new().fg(c)
        }
    }

    pub fn bold(&self, c: Color) -> Style {
        self.fg(c).add_modifier(Modifier::BOLD)
    }

    pub fn selected(&self) -> Style {
        if self.plain {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new()
                .bg(self.highlight_bg)
                .add_modifier(Modifier::BOLD)
        }
    }
}
