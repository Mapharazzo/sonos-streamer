use crate::{
    browser::{BROWSER_CHANNELS, BROWSER_SAMPLE_RATE},
    enums::streaming::StreamingFormat,
    openhome::rendercontrol::WavData,
    utils::configuration::Configuration,
};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct BrowserMode {
    pub bridge_port: u16,
}

impl BrowserMode {
    #[must_use]
    pub fn new(bridge_port: u16) -> Self {
        Self { bridge_port }
    }

    #[must_use]
    pub fn wav_data(self) -> WavData {
        WavData {
            sample_format: cpal::SampleFormat::F32,
            sample_rate: BROWSER_SAMPLE_RATE,
            channels: BROWSER_CHANNELS,
        }
    }

    pub fn apply_session_overrides(self, config: &mut Configuration) {
        config.streaming_format = Some(StreamingFormat::Wav);
        config.bits_per_sample = Some(16);
    }

    #[cfg(feature = "cli")]
    #[must_use]
    pub fn from_args(
        args: &crate::utils::commandline::Args,
        config: &Configuration,
    ) -> Option<Self> {
        args.browser_source
            .then(|| Self::new(args.browser_port.unwrap_or(config.browser_bridge_port)))
    }
}

#[cfg(test)]
mod tests {
    use super::BrowserMode;
    use crate::{enums::streaming::StreamingFormat, utils::configuration::Configuration};

    #[test]
    fn browser_mode_is_fixed_stereo_48k_and_wav_16() {
        let mut config: Configuration = toml::from_str("").expect("in-memory configuration");
        config.streaming_format = Some(StreamingFormat::Lpcm);
        config.bits_per_sample = Some(24);
        let mode = BrowserMode::new(5902);

        mode.apply_session_overrides(&mut config);
        let wav = mode.wav_data();

        assert_eq!(wav.sample_rate, 48_000);
        assert_eq!(wav.channels, 2);
        assert_eq!(wav.sample_format, cpal::SampleFormat::F32);
        assert_eq!(config.streaming_format, Some(StreamingFormat::Wav));
        assert_eq!(config.bits_per_sample, Some(16));
    }

    #[test]
    fn browser_mode_requires_opt_in_and_prefers_argument_port() {
        let mut config: Configuration = toml::from_str("").expect("in-memory configuration");
        config.browser_bridge_port = 5902;

        let desktop_args = crate::utils::commandline::Args::default();
        assert_eq!(BrowserMode::from_args(&desktop_args, &config), None);

        let browser_args = crate::utils::commandline::Args::parse_args([
            "--browser-source",
            "--browser-port",
            "6002",
        ])
        .expect("browser args");
        assert_eq!(
            BrowserMode::from_args(&browser_args, &config),
            Some(BrowserMode::new(6002))
        );
    }
}
