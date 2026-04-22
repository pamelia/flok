use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rattles::presets;

const WAITING_SUFFIX: &str = "streaming";

pub(crate) struct WaitingSpinner {
    choice: SpinnerChoice,
    elapsed: Duration,
}

impl WaitingSpinner {
    pub(crate) fn random() -> Self {
        Self::from_seed(random_seed())
    }

    pub(crate) fn from_seed(seed: u64) -> Self {
        Self { choice: SpinnerChoice::from_seed(seed), elapsed: Duration::ZERO }
    }

    pub(crate) fn advance(&mut self, delta: Duration) {
        self.elapsed = self.elapsed.saturating_add(delta);
    }

    pub(crate) fn text(&self) -> String {
        format!("{} {WAITING_SUFFIX}", self.choice.frame_at(self.elapsed))
    }
}

#[derive(Clone, Copy)]
enum SpinnerChoice {
    Dots,
    Orbit,
    Pulse,
    Sparkle,
    WaveRows,
    Arc,
    RollingLine,
    Toggle,
    Moon,
    Weather,
}

impl SpinnerChoice {
    fn from_seed(seed: u64) -> Self {
        match seed % 10 {
            0 => Self::Dots,
            1 => Self::Orbit,
            2 => Self::Pulse,
            3 => Self::Sparkle,
            4 => Self::WaveRows,
            5 => Self::Arc,
            6 => Self::RollingLine,
            7 => Self::Toggle,
            8 => Self::Moon,
            _ => Self::Weather,
        }
    }

    fn frame_at(self, elapsed: Duration) -> &'static str {
        match self {
            Self::Dots => presets::braille::dots().frame_at(elapsed),
            Self::Orbit => presets::braille::orbit().frame_at(elapsed),
            Self::Pulse => presets::braille::pulse().frame_at(elapsed),
            Self::Sparkle => presets::braille::sparkle().frame_at(elapsed),
            Self::WaveRows => presets::braille::waverows().frame_at(elapsed),
            Self::Arc => presets::ascii::arc().frame_at(elapsed),
            Self::RollingLine => presets::ascii::rolling_line().frame_at(elapsed),
            Self::Toggle => presets::ascii::toggle().frame_at(elapsed),
            Self::Moon => presets::emoji::moon().frame_at(elapsed),
            Self::Weather => presets::emoji::weather().frame_at(elapsed),
        }
    }
}

fn random_seed() -> u64 {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    (nanos as u64) ^ ((nanos >> 64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_spinner_text_includes_status_label() {
        let spinner = WaitingSpinner::from_seed(0);
        let text = spinner.text();
        assert!(text.contains("streaming"), "text: {text:?}");
        assert!(text.len() > "streaming".len(), "text: {text:?}");
    }

    #[test]
    fn waiting_spinner_advance_keeps_status_label() {
        let mut spinner = WaitingSpinner::from_seed(4);
        spinner.advance(Duration::from_millis(250));
        let text = spinner.text();
        assert!(text.contains("streaming"), "text: {text:?}");
    }
}
