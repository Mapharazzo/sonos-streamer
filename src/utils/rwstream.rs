/*
///
/// rwstream.rs
///
/// ChannelStream: the interface between the audio capture and the HTTP streaming clients
///
/// The write method sends the captured audio samples (f32) on the CrossBeam channel of the Channelstream
/// and the HTTP Response then uses the Read trait implementation of the Channelstream to read
/// them back for streaming to the HTTP client
///
*/
use crate::{
    enums::streaming::{
        BitDepth::{self, *},
        Endian::{self, *},
        StreamingFormat,
    },
    globals::statics::get_config,
    latency::pulse_trigger_path,
    utils::samples_conv::{
        f32_chunk_to_i32, i32_to_i16be, i32_to_i16le, i32_to_i24be, i32_to_i24le,
    },
};
use crossbeam_channel::{Receiver, Sender};
use ecow::EcoString;
#[cfg(debug_assertions)]
use fastrand::Rng;
use itertools::Itertools;
use log::{debug, info};
use std::{
    collections::VecDeque,
    io::{Error, Read, Result as IoResult},
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

static FILL_LPCM_READ_CALLS: AtomicU64 = AtomicU64::new(0);
static FILL_LPCM_READ_BYTES: AtomicU64 = AtomicU64::new(0);

/// Shared audio sample buffer passed between capture and streaming threads.
pub type AudioSamples = Arc<Vec<f32>>;

/// If `trigger_pulse.txt` exists, overwrite the start of `samples` (stereo f32) with the Barker pulse
/// and arm the latency stopwatch. Returns `true` if a trigger file was consumed.
fn apply_latency_pulse_if_triggered(samples: &mut Vec<f32>) -> bool {
    let trigger_path = pulse_trigger_path();
    if std::fs::metadata(&trigger_path).is_err() {
        return false;
    }
    let _ = std::fs::remove_file(&trigger_path);
    info!("LATENCY PULSE: Injecting Barker sequence into audio buffer!");

    let barker_mono = crate::latency::barker_mono();
    let mut pulse = Vec::with_capacity(barker_mono.len() * 2);
    for bit in barker_mono.iter() {
        pulse.push(*bit);
        pulse.push(*bit);
    }

    if samples.len() < pulse.len() {
        samples.resize(pulse.len(), 0.0);
    }
    let len_to_overwrite = std::cmp::min(pulse.len(), samples.len());
    for i in 0..len_to_overwrite {
        samples[i] = pulse[i];
    }

    crate::latency::PULSE_INJECTED_AT.store(
        crate::latency::now_ms(),
        std::sync::atomic::Ordering::SeqCst,
    );
    true
}

/// Some DLNA renderers (notably Sonos) drop long-lived HTTP streams that are **perfect** digital
/// silence for many seconds — common with WASAPI loopback before any app outputs to the device.
/// Add imperceptible triangular dither in normalized float (~±1) so PCM is not all-zero.
fn maybe_dither_digital_silence(bits_per_sample: u16, samples: &mut [f32]) {
    let peak = samples.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
    if peak > 1.0e-8 {
        return;
    }
    let scale = 2_f32.powi(i32::from(bits_per_sample.clamp(1, 32)));
    let amp = (1.0 / scale) * 0.35;
    for (i, s) in samples.iter_mut().enumerate() {
        let tri = match i % 4 {
            0 => -1.0f32,
            1 => 1.0,
            2 => 1.0,
            _ => -1.0,
        };
        *s = tri * amp;
    }
}

/// Channelstream - used to transport the f32 samples from the `wave_reader`
/// to the http output stream in LPCM/WAV/FLAC format
/// implements `Read` for the HTTP streaming
#[derive(Clone)]
pub struct ChannelStream {
    pub s: Sender<AudioSamples>,
    pub r: Receiver<AudioSamples>,
    pub remote_ip: EcoString,
    pub streaming_format: StreamingFormat,
    fifo: VecDeque<f32>,
    silence: Vec<f32>,
    capture_timeout: Duration,
    sending_silence: bool,
    wav_hdr: Vec<u8>,
    use_wave_format: bool,
    bits_per_sample: u16,
}

impl ChannelStream {
    pub fn new(
        tx: Sender<AudioSamples>,
        rx: Receiver<AudioSamples>,
        remote_ip_addr: EcoString,
        use_wave_format: bool,
        sample_rate: u32,
        bits_per_sample: u16,
        streaming_format: StreamingFormat,
    ) -> ChannelStream {
        let capture_timeout = u64::from(get_config().capture_timeout.unwrap_or(5));
        ChannelStream {
            s: tx,
            r: rx,
            fifo: VecDeque::with_capacity(16384),
            silence: get_silence_buffer(sample_rate, capture_timeout / 4),
            capture_timeout: Duration::from_millis(capture_timeout), // silence kicks in after CAPTURE_TIMEOUT seconds
            sending_silence: false,
            remote_ip: remote_ip_addr,
            wav_hdr: if streaming_format == StreamingFormat::Wav {
                create_wav_hdr(sample_rate, bits_per_sample)
            } else if streaming_format == StreamingFormat::Rf64 {
                create_rf64_hdr(sample_rate, bits_per_sample)
            } else {
                Vec::new()
            },
            use_wave_format,
            bits_per_sample,
            streaming_format,
        }
    }

    /// called by the `wave_reader`s to write the f32 samples to our input channel
    pub fn write(&self, samples: AudioSamples) {
        // don't blow up memory if streaming stalls for some reason
        // 10_000 messages (capture buffers, not samples) is a quite a lot
        if self.s.len() < 10_000 {
            let _ = self.s.send(samples);
        } else {
            #[cfg(debug_assertions)]
            {
                debug!("Samples buffer overflow, dropping chunks!");
            }
        }
    }

    /// fill the LPCM/WAV/RF64 fifo buffer with f32 samples
    /// (or with f32 silence if no samples are coming)
    fn get_samples(&mut self) {
        let time_out = self.capture_timeout;
        match self.r.recv_timeout(time_out) {
            Ok(chunk) => {
                let mut samples = chunk.as_ref().clone();
                if !apply_latency_pulse_if_triggered(&mut samples) {
                    maybe_dither_digital_silence(self.bits_per_sample, &mut samples);
                }
                self.fifo.append(&mut VecDeque::from(samples));
                self.sending_silence = false;
            }
            Err(_) => {
                // When capture has not produced a chunk yet (or stalls), we still must honour
                // `trigger_pulse.txt` or latency calibration and HTTP `/latency/trigger` never fire.
                let mut samples = self.silence.clone();
                if apply_latency_pulse_if_triggered(&mut samples) {
                    self.fifo.append(&mut VecDeque::from(samples));
                    self.sending_silence = false;
                } else {
                    maybe_dither_digital_silence(self.bits_per_sample, &mut samples);
                    self.fifo.append(&mut VecDeque::from(samples));
                    self.sending_silence = true;
                }
            }
        }
    }

    /// Fill the HTTP read buffer with LPCM/WAV/RF64 data from the f32 samples `VecDeque` fifo.
    ///
    /// The f32 samples are read from the f32 input channel and buffered in the`VecDeque` fifo.
    /// The VecDeque is then read for conversion to LPCM/WAV/RF64 data and
    /// stored in the HTTP transmission buffer as needed
    fn fill_lpcm_buffer(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        /// the f32 samples are converted in chunks of 4 f32 values (SSE2 f32x4)
        const CHUNK_SIZE: usize = 4;
        if self.use_wave_format && !self.wav_hdr.is_empty() {
            let i = self.wav_hdr.len();
            debug_assert!(
                buf.len() >= i,
                "HTTP read buffer smaller than WAV/RF64 header!"
            );
            buf[..i].copy_from_slice(&self.wav_hdr);
            self.wav_hdr.clear();
            return Ok(i);
        }
        // make sure we have enough samples ready to fill the read buffer
        let bytes_per_sample = (self.bits_per_sample / 8) as usize;
        let buf_chunksize = bytes_per_sample * 4;
        if buf.is_empty() {
            // #region agent log
            crate::debug_agent::agent_log(
                "H2",
                "rwstream.rs:fill_lpcm_buffer",
                "empty_read_buf_eof",
                "{}",
            );
            // #endregion
            return Ok(0);
        }
        let chunks_needed = buf.len() / buf_chunksize;
        if chunks_needed == 0 {
            // #region agent log
            crate::debug_agent::agent_log(
                "H2",
                "rwstream.rs:fill_lpcm_buffer",
                "partial_buf_chunks_zero_eof",
                &format!(
                    "{{\"buf_len\":{},\"buf_chunksize\":{}}}",
                    buf.len(),
                    buf_chunksize
                ),
            );
            // #endregion
            return Ok(0);
        }
        let samples_needed: usize = chunks_needed * 4;
        while self.fifo.len() < samples_needed {
            self.get_samples();
        }
        // drain the fifo of the samples needed to fill the buffer in 4 samples chunks
        // this way we don't need the expensive pop_front()
        // the drain contains the exact number of samples needed to fill the streaming buffer
        let drain_iter = self.fifo.drain(0..samples_needed).chunks(CHUNK_SIZE);
        // fill the buffer with sample chunks (1 chunk = 4 samples)
        // so we can zip the buf in chunks of 4 samples (chunksize) with the drain
        let chunks_iter = buf.chunks_exact_mut(buf_chunksize).zip(&drain_iter);
        // setup sample conversion parameters
        let endianness: Endian = if self.use_wave_format { Little } else { Big };
        let bd = BitDepth::from(self.bits_per_sample);
        // convert the f32 samples to i16 or i24 little/big endian to fille the buffer
        match (endianness, bd) {
            (Little, Bits16) => chunks_iter.for_each(|(chunk, sample_chunk)| {
                i32_to_i16le(&f32_chunk_to_i32(bd, sample_chunk), chunk);
            }),
            (Little, Bits24) => chunks_iter.for_each(|(chunk, sample_chunk)| {
                i32_to_i24le(&f32_chunk_to_i32(bd, sample_chunk), chunk);
            }),
            (Big, Bits16) => chunks_iter.for_each(|(chunk, sample_chunk)| {
                i32_to_i16be(&f32_chunk_to_i32(bd, sample_chunk), chunk);
            }),
            (Big, Bits24) => chunks_iter.for_each(|(chunk, sample_chunk)| {
                i32_to_i24be(&f32_chunk_to_i32(bd, sample_chunk), chunk);
            }),
        }
        let n = chunks_needed * buf_chunksize;
        // #region agent log
        let calls = FILL_LPCM_READ_CALLS.fetch_add(1, Ordering::Relaxed) + 1;
        let bytes = FILL_LPCM_READ_BYTES.fetch_add(n as u64, Ordering::Relaxed) + n as u64;
        if calls <= 3 || calls % 500 == 0 {
            crate::debug_agent::agent_log(
                "H4",
                "rwstream.rs:fill_lpcm_buffer",
                "pcm_read_progress",
                &format!(
                    "{{\"calls\":{calls},\"returned_bytes\":{n},\"cumulative_bytes\":{bytes}}}"
                ),
            );
        }
        // #endregion
        Ok(n)
    }
}

/// implement the Read trait for the HTTP writer
/// filling the read buffer with FLAC or LPCM/WAV/RF64 data
impl Read for ChannelStream {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        self.fill_lpcm_buffer(buf)
    }
}

/// create an "infinite size" wav hdr for the PCM Data (s16le or s24le)
/// note: this may not work when streaming to an older "libsndfile" based renderer
/// as it insists on a seekable WAV file depending on the open mode used
/*
Field	        Length	Contents
ckID	        4	    Chunk ID: 'RIFF'
cksize	        4	    Chunk size: 4 + 24 + (8 + M*Nc*Ns + (0 or 1)
WAVEID	        4	    WAVE ID: 'WAVE'
ckID	        4	    Chunk ID: 'fmt '
cksize	        4	    Chunk size: 16
wFormatTag	    2	    WAVE_FORMAT_PCM (0001)
nChannels	    2	    Nc
nSamplesPerSec	4	    F
nAvgBytesPerSec	4	    F*M*Nc
nBlockAlign	    2	    M*Nc
wBitsPerSample	2	    rounds up to 8*M
ckID	        4	    Chunk ID: 'data'
cksize	        4	    Chunk size: M*Nc*Ns
sampled data	M*Nc*Ns	Nc*Ns channel-interleaved M-byte samples
pad byte	    0 or 1	Padding byte if M*Nc*Ns is odd
*/
fn create_wav_hdr(sample_rate: u32, bits_per_sample: u16) -> Vec<u8> {
    let mut hdr = [0u8; 44];
    let channels: u16 = 2;
    let bytes_per_sample: u16 = bits_per_sample / 8;
    let block_align: u16 = channels * bytes_per_sample;
    let byte_rate: u32 = sample_rate * u32::from(block_align);
    hdr[0..4].copy_from_slice(b"RIFF"); //ChunkId, little endian WAV
    let riffchunksize: u32 = u32::MAX; // RIFF chunksize
    let datachunksize: u32 = riffchunksize - 36; // data chunksize
    hdr[4..8].copy_from_slice(&riffchunksize.to_le_bytes()); // RIFF ChunkSize
    hdr[8..12].copy_from_slice(b"WAVE"); // File Format
    hdr[12..16].copy_from_slice(b"fmt "); // SubChunk = Format
    hdr[16..20].copy_from_slice(&16u32.to_le_bytes()); // fmt chunksize for PCM
    hdr[20..22].copy_from_slice(&1u16.to_le_bytes()); // AudioFormat: uncompressed PCM
    hdr[22..24].copy_from_slice(&channels.to_le_bytes()); // numchannels 2
    hdr[24..28].copy_from_slice(&sample_rate.to_le_bytes()); // SampleRate
    hdr[28..32].copy_from_slice(&byte_rate.to_le_bytes()); // ByteRate (Bps)
    hdr[32..34].copy_from_slice(&block_align.to_le_bytes()); // BlockAlign
    hdr[34..36].copy_from_slice(&bits_per_sample.to_le_bytes()); // BitsPerSample
    hdr[36..40].copy_from_slice(b"data"); // SubChunk = "data"
    hdr[40..44].copy_from_slice(&datachunksize.to_le_bytes()); // data SubChunkSize
    debug!("WAV Header (l={}): \r\n{:02x?}", hdr.len(), hdr);
    hdr.to_vec()
}

/// create an "infinite size" RF64 header for the PCM Data (s16le or s24le)
/*
Field           Len offset   Meaning
ckID            4   0        chunk ID 'RF64'
ckSize          4   4        dummy chunksize -1 (0xffffffff)
WAVEID          4   8        compatibility 'WAVE' ID
ckID            4   12       chunk ID 'ds64'
ckSize          4   16       chunk size (28)
RIFFSize        8   20       size of RIFF chunk (data chunk size - 8)
dataSize        8   28       size of data chunk
sampleCount     8   36       number of samples
tableLength     4   44       number of valid table array entries 0
tableArray      0            not used
ckID            4   48       chunk ID 'fmt '
cksize	        4	52       Chunk size: 16
wFormatTag	    2	56       WAVE_FORMAT_PCM (0001)
nChannels	    2	58       Nc
nSamplesPerSec	4	60       F
nAvgBytesPerSec	4	64       F*M*Nc
nBlockAlign	    2	68       M*Nc
wBitsPerSample	2	70       rounds up to 8*M
ckID	        4	72       Chunk ID: 'data'
cksize	        4	76       dummy Chunk size -1 (0xffffffff)
sampled data    ... 80
*/
fn create_rf64_hdr(sample_rate: u32, bits_per_sample: u16) -> Vec<u8> {
    let mut hdr = [0u8; 80];
    let channels: u16 = 2;
    let bytes_per_sample: u16 = bits_per_sample / 8;
    let block_align: u16 = channels * bytes_per_sample;
    let byte_rate: u32 = sample_rate * u32::from(block_align);
    hdr[0..4].copy_from_slice(b"RF64"); //ChunkId, little endian WAV
    let rf64chunksize: u32 = 0xffff_ffff; // dummy RIFF chunksize
    let datachunksize: u32 = 0xffff_ffff; // dummy data chunksize
    let ds64chunksize: u32 = 28;
    let frame_size: u64 = u64::from(bytes_per_sample) * u64::from(channels);
    let ds64nsamples: u64 = ((i64::MAX / 8) as u64 - 72) / frame_size;
    let ds64datasize: u64 = ds64nsamples * frame_size; // exact multiple of frame_size
    let ds64riffsize: u64 = ds64datasize + 72; // header overhead: WAVE(4)+ds64(36)+fmt(24)+data_hdr(8)
    let ds64tablelength = 0u32;
    hdr[4..8].copy_from_slice(&rf64chunksize.to_le_bytes()); // RIFF ChunkSize
    hdr[8..12].copy_from_slice(b"WAVE"); // File Format
    hdr[12..16].copy_from_slice(b"ds64"); // SubChunk = ds64
    hdr[16..20].copy_from_slice(&ds64chunksize.to_le_bytes());
    hdr[20..28].copy_from_slice(&ds64riffsize.to_le_bytes());
    hdr[28..36].copy_from_slice(&ds64datasize.to_le_bytes());
    hdr[36..44].copy_from_slice(&ds64nsamples.to_le_bytes());
    hdr[44..48].copy_from_slice(&ds64tablelength.to_le_bytes());
    hdr[48..52].copy_from_slice(b"fmt "); // SubChunk = Format
    hdr[52..56].copy_from_slice(&16u32.to_le_bytes()); // fmt chunksize for PCM
    hdr[56..58].copy_from_slice(&1u16.to_le_bytes()); // AudioFormat: uncompressed PCM
    hdr[58..60].copy_from_slice(&channels.to_le_bytes()); // numchannels 2
    hdr[60..64].copy_from_slice(&sample_rate.to_le_bytes()); // SampleRate
    hdr[64..68].copy_from_slice(&byte_rate.to_le_bytes()); // ByteRate (Bps)
    hdr[68..70].copy_from_slice(&block_align.to_le_bytes()); // BlockAlign
    hdr[70..72].copy_from_slice(&bits_per_sample.to_le_bytes()); // BitsPerSample
    hdr[72..76].copy_from_slice(b"data"); // SubChunk = "data"
    hdr[76..80].copy_from_slice(&datachunksize.to_le_bytes()); // data SubChunkSize
    debug!("RF64 Header (l={}): \r\n{:02x?}", hdr.len(), hdr);

    hdr.to_vec()
}

fn get_silence_buffer(sample_rate: u32, silence_period: u64) -> Vec<f32> {
    // silence_period is in msecs (capture_timeout / 4), sample rate is per second, 2 channels for stereo
    let size = ((sample_rate as u64 * 2 * silence_period) / 1000) as usize;
    let mut silence = Vec::with_capacity(size);
    silence.resize(size, 0f32);
    silence
}

///
/// fill the pre-allocated noise buffer with a very faint white noise (-60db)
///
#[cfg(debug_assertions)]
#[allow(dead_code)]
fn get_noise_buffer(sample_rate: u32, silence_period: u64) -> Vec<f32> {
    // create the random generator for the white noise
    let mut rng = Rng::with_seed(79);
    let size = ((sample_rate as u64 * 2 * silence_period) / 1000) as usize;
    let mut noise = Vec::with_capacity(size);
    noise.resize(size, 0.0);
    let amplitude: f32 = 0.001;
    for sample in &mut noise {
        *sample = ((rng.f32() * 2.0) - 1.0) * amplitude;
    }
    noise
}

#[cfg(test)]
mod tests {
    use crate::utils::rwstream::*;
    #[test]

    fn test_wav_hdr() {
        let _hdr = create_wav_hdr(44100, 24);
        //eprintln!("WAV Header (l={}): \r\n{:02x?}", hdr.len(), hdr);
        let _hdr = create_wav_hdr(44100, 16);
        //eprintln!("WAV Header (l={}): \r\n{:02x?}", hdr.len(), hdr);
    }

    #[test]
    fn test_silence() {
        const SAMPLE_RATE: u32 = 44100;
        let sb = get_silence_buffer(SAMPLE_RATE, 250);
        assert_eq!(sb.len(), ((SAMPLE_RATE * 2) as u64 / (1000 / 250)) as usize);
    }

    #[test]
    #[cfg(debug_assertions)]
    fn test_noise() {
        // create the random generator for the white noise
        let mut rng = Rng::with_seed(79);
        let sample_rate = 44100;
        let silence_period = 250; //msecs
        let size = ((sample_rate as u64 * 2 * silence_period) / 1000) as usize;
        let mut noise = Vec::with_capacity(size);
        noise.resize(size, 0.0);
        let amplitude: f32 = 0.001;
        for sample in &mut noise {
            *sample = ((rng.f32() * 2.0) - 1.0) * amplitude;
        }
        eprintln!("{noise:?}");
    }

    use dasp_sample::{I24, Sample};
    // just to prove that ((i32 >> 8) & 0xffffff) is indeed I24
    #[test]
    fn test_i24() {
        let sample = i32::from_sample(0x12345678i32);
        let i24_sample = I24::from_sample(sample);
        println!("i24: {i24_sample:X?}");
        let f32_sample: f32 = 0.123456;
        let a1 = {
            let i24sample = i32::from_sample(f32_sample) >> 8;
            let b = i24sample.to_le_bytes();
            [b[0], b[1], b[2]]
        };
        let a2 = { &((i32::from_sample(f32_sample) >> 8).to_le_bytes())[..=2] };
        assert_eq!(a1, a2);
        let b1 = {
            let i24sample = i32::from_sample(f32_sample) >> 8;
            let b = i24sample.to_be_bytes();
            [b[1], b[2], b[3]]
        };
        let b2 = { &((i32::from_sample(f32_sample) >> 8).to_be_bytes())[1..] };
        assert_eq!(b1, b2);
    }
}
