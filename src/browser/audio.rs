use std::{error::Error, fmt};

use crate::{
    browser::protocol::AudioSession,
    utils::{audiodevices::publish_samples, rwstream::AudioSamples},
};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AudioPacketError {
    TooLarge,
    PartialFrame,
}

impl fmt::Display for AudioPacketError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => formatter.write_str("audio packet exceeds the session limit"),
            Self::PartialFrame => formatter.write_str("audio packet contains a partial frame"),
        }
    }
}

impl Error for AudioPacketError {}

pub fn decode_packet(
    session: &AudioSession,
    bytes: &[u8],
) -> Result<AudioSamples, AudioPacketError> {
    if bytes.len() > session.max_packet_bytes {
        return Err(AudioPacketError::TooLarge);
    }

    let frame_bytes = usize::from(session.channels) * size_of::<i16>();
    if !bytes.len().is_multiple_of(frame_bytes) {
        return Err(AudioPacketError::PartialFrame);
    }

    let decoded = bytes
        .chunks_exact(size_of::<i16>())
        .map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32768.0)
        .collect::<Vec<_>>();
    Ok(AudioSamples::new(decoded))
}

#[derive(Debug, Clone, Copy)]
pub struct BrowserAudioPublisher {
    session: AudioSession,
}

impl BrowserAudioPublisher {
    #[must_use]
    pub const fn new(session: AudioSession) -> Self {
        Self { session }
    }

    pub fn publish(&self, bytes: &[u8]) -> Result<usize, AudioPacketError> {
        let samples = decode_packet(&self.session, bytes)?;
        let sample_count = samples.len();
        publish_samples(samples);
        Ok(sample_count)
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioPacketError, BrowserAudioPublisher, decode_packet};
    use crate::browser::protocol::AudioSession;

    #[test]
    fn decodes_interleaved_s16le_to_normalized_f32() {
        let session = AudioSession::v1();
        let samples = decode_packet(&session, &[0x00, 0x80, 0xff, 0x7f]).unwrap();

        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0], -1.0);
        assert!((samples[1] - 0.999_969_5).abs() < 0.000_001);
    }

    #[test]
    fn rejects_partial_stereo_frame_and_oversized_packet() {
        let session = AudioSession::v1();

        assert_eq!(
            decode_packet(&session, &[0; 2]),
            Err(AudioPacketError::PartialFrame)
        );
        assert_eq!(
            decode_packet(&session, &vec![0; session.max_packet_bytes + 4]),
            Err(AudioPacketError::TooLarge)
        );
    }

    #[test]
    fn browser_publisher_reports_published_sample_count() {
        let publisher = BrowserAudioPublisher::new(AudioSession::v1());

        assert_eq!(publisher.publish(&[0, 0, 1, 0]).unwrap(), 2);
    }
}
