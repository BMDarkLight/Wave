use rodio::Source;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;
use symphonia::core::audio::{SampleBuffer, SignalSpec};
use symphonia::core::codecs::{CodecRegistry, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::error::AudioError;

/// A codec registry that combines all of Symphonia's built-in decoders with the
/// libopus-backed `OpusDecoder`, since Symphonia has no first-party Opus codec.
fn codec_registry() -> &'static CodecRegistry {
    static REGISTRY: OnceLock<CodecRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut registry);
        registry.register_all::<symphonia_adapter_libopus::OpusDecoder>();
        registry
    })
}

pub struct SymphoniaSource {
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    format: Box<dyn symphonia::core::formats::FormatReader>,
    buffer: SampleBuffer<i16>,
    current_frame_offset: usize,
    total_duration: Option<Duration>,
    spec: SignalSpec,
}

/// Byte length of a leading ID3v2 tag, including its header and footer.
///
/// Returns `None` when the bytes aren't an ID3v2 header at all.
///
/// This exists because some encoders emit a *degenerate* tag: a valid ID3v2.4
/// header declaring a body size of zero. Deezer's preview MP3s all do. Symphonia
/// walks off the end of such a tag during probing and fails the whole file with
/// `UnexpectedEof("out of bounds")`, even though the audio after it is perfectly
/// good. Knowing the tag's exact length lets us start decoding just past it.
fn id3v2_tag_len(header: &[u8]) -> Option<u64> {
    if header.len() < 10 || &header[0..3] != b"ID3" {
        return None;
    }
    // Version bytes are (major, revision); 0xFF is invalid in both.
    if header[3] == 0xFF || header[4] == 0xFF {
        return None;
    }
    // The 4 size bytes are synchsafe: 7 significant bits each, high bit always 0.
    if header[6..10].iter().any(|b| b & 0x80 != 0) {
        return None;
    }
    let body = ((header[6] as u64) << 21)
        | ((header[7] as u64) << 14)
        | ((header[8] as u64) << 7)
        | (header[9] as u64);
    // Bit 4 of the flags marks a 10-byte footer (ID3v2.4 only).
    let footer = if header[5] & 0x10 != 0 { 10 } else { 0 };
    Some(10 + body + footer)
}

/// A file presented as if it began at `offset`.
///
/// Used to hand Symphonia the audio *after* a tag it can't parse, without
/// copying the file. Absolute seeks are rebased so the format reader can never
/// wander back into the bad bytes.
struct OffsetMediaSource {
    file: File,
    offset: u64,
    file_len: Option<u64>,
}

impl OffsetMediaSource {
    fn new(path: &str, offset: u64) -> std::io::Result<Self> {
        let mut file = File::open(path)?;
        let file_len = file.metadata().ok().map(|m| m.len());
        file.seek(SeekFrom::Start(offset))?;
        Ok(Self {
            file,
            offset,
            file_len,
        })
    }
}

impl Read for OffsetMediaSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buf)
    }
}

impl Seek for OffsetMediaSource {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let absolute = match pos {
            SeekFrom::Start(n) => SeekFrom::Start(self.offset.saturating_add(n)),
            // End and Current are already relative to real positions.
            other => other,
        };
        let landed = self.file.seek(absolute)?;
        // Never report a position before the start of the audio.
        Ok(landed.saturating_sub(self.offset))
    }
}

impl MediaSource for OffsetMediaSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        self.file_len.map(|len| len.saturating_sub(self.offset))
    }
}

impl SymphoniaSource {
    pub fn new(path: &str) -> Result<Self, AudioError> {
        crate::path_validation::validate_audio_path(path)
            .map_err(|e| AudioError::FileOpen(format!("{e}: {path}")))?;
        let file = File::open(path).map_err(|error| {
            AudioError::FileOpen(format!("Cannot open audio file \"{path}\": {error}"))
        })?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = Path::new(path).extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }

        let format_opts = FormatOptions::default();
        let metadata_opts = MetadataOptions::default();
        let decoder_opts = DecoderOptions::default();

        let mut probed = match symphonia::default::get_probe().format(
            &hint,
            mss,
            &format_opts,
            &metadata_opts,
        ) {
            Ok(probed) => probed,
            Err(first_error) => {
                // A leading ID3v2 tag Symphonia can't walk (notably the empty
                // ID3v2.4 header every Deezer preview carries) fails the probe
                // even though the audio behind it decodes fine. Retry from just
                // past the tag before giving up.
                let retried =
                    Self::probe_past_leading_tag(path, &hint, &format_opts, &metadata_opts);
                match retried {
                    Some(probed) => probed,
                    None => {
                        return Err(AudioError::Decode(format!(
                            "Unrecognised or corrupted audio format in \"{path}\": {first_error}"
                        )))
                    }
                }
            }
        };

        let track_id = probed
            .format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| {
                AudioError::UnsupportedFormat(format!(
                    "No playable audio stream found in \"{path}\""
                ))
            })?
            .id;

        let track = probed
            .format
            .tracks()
            .iter()
            .find(|t| t.id == track_id)
            .unwrap();

        let codec_params = track.codec_params.clone();

        let mut decoder = codec_registry()
            .make(&codec_params, &decoder_opts)
            .map_err(|error| AudioError::Decode(format!("Failed to create decoder: {error}")))?;

        let total_duration =
            codec_params
                .time_base
                .zip(codec_params.n_frames)
                .map(|(base, frames)| {
                    let time = base.calc_time(frames);
                    Duration::from_secs(time.seconds) + Duration::from_secs_f64(time.frac)
                });

        let (buffer, spec) = Self::decode_first_packet(&mut probed.format, &mut decoder, track_id)?;

        Ok(Self {
            decoder,
            track_id,
            format: probed.format,
            buffer,
            current_frame_offset: 0,
            total_duration,
            spec,
        })
    }

    /// Re-probe `path` starting after a leading ID3v2 tag. Returns `None` when
    /// there is no such tag or the retry fails too, leaving the caller to
    /// report the original error rather than a misleading second one.
    fn probe_past_leading_tag(
        path: &str,
        hint: &Hint,
        format_opts: &FormatOptions,
        metadata_opts: &MetadataOptions,
    ) -> Option<symphonia::core::probe::ProbeResult> {
        let mut header = [0u8; 10];
        {
            let mut file = File::open(path).ok()?;
            file.read_exact(&mut header).ok()?;
        }
        let tag_len = id3v2_tag_len(&header)?;

        let source = OffsetMediaSource::new(path, tag_len).ok()?;
        let mss = MediaSourceStream::new(Box::new(source), Default::default());
        match symphonia::default::get_probe().format(hint, mss, format_opts, metadata_opts) {
            Ok(probed) => {
                tracing::debug!("Recovered \"{path}\" by skipping {tag_len} bytes of ID3v2 tag");
                Some(probed)
            }
            Err(_) => None,
        }
    }

    fn decode_first_packet(
        format: &mut Box<dyn symphonia::core::formats::FormatReader>,
        decoder: &mut Box<dyn symphonia::core::codecs::Decoder>,
        track_id: u32,
    ) -> Result<(SampleBuffer<i16>, SignalSpec), AudioError> {
        loop {
            match format.next_packet() {
                Ok(packet) => {
                    if packet.track_id() != track_id {
                        continue;
                    }
                    match decoder.decode(&packet) {
                        Ok(decoded) => {
                            let spec = *decoded.spec();
                            let duration =
                                symphonia::core::units::Duration::from(decoded.capacity() as u64);
                            let mut buf = SampleBuffer::<i16>::new(duration, spec);
                            buf.copy_interleaved_ref(decoded);
                            return Ok((buf, spec));
                        }
                        Err(_) => continue,
                    }
                }
                Err(symphonia::core::errors::Error::IoError(ref e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    return Err(AudioError::Decode("Unexpected end of file".into()));
                }
                Err(symphonia::core::errors::Error::SeekError(_)) => {
                    return Err(AudioError::Decode(
                        "Seek error during initialization".into(),
                    ));
                }
                Err(error) => {
                    return Err(AudioError::Decode(format!(
                        "Failed to read packet: {error}"
                    )));
                }
            }
        }
    }
}

impl Iterator for SymphoniaSource {
    type Item = i16;

    fn next(&mut self) -> Option<i16> {
        if self.current_frame_offset >= self.buffer.len() {
            loop {
                let packet = self.format.next_packet().ok()?;
                if packet.track_id() != self.track_id {
                    continue;
                }
                let decoded = self.decoder.decode(&packet).ok()?;
                decoded.spec().clone_into(&mut self.spec);
                let duration = symphonia::core::units::Duration::from(decoded.capacity() as u64);
                let mut buf = SampleBuffer::<i16>::new(duration, self.spec);
                buf.copy_interleaved_ref(decoded);
                self.buffer = buf;
                self.current_frame_offset = 0;
                break;
            }
        }

        let sample = self.buffer.samples().get(self.current_frame_offset)?;
        self.current_frame_offset += 1;
        Some(*sample)
    }
}

impl Source for SymphoniaSource {
    fn current_frame_len(&self) -> Option<usize> {
        Some(self.buffer.len())
    }

    fn channels(&self) -> u16 {
        self.spec.channels.count() as u16
    }

    fn sample_rate(&self) -> u32 {
        self.spec.rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        use symphonia::core::formats::{SeekMode, SeekTo};

        let seek_beyond_end = self
            .total_duration()
            .is_some_and(|dur| dur.saturating_sub(pos).as_millis() < 1);

        let time: symphonia::core::units::Time = if seek_beyond_end {
            let dur = self.total_duration.expect("checked above");
            (dur.as_secs_f64() - 0.0001).max(0.0).into()
        } else {
            pos.as_secs_f64().into()
        };

        let to_skip = self.current_frame_offset % self.channels() as usize;

        let seek_res = self
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: None,
                },
            )
            .map_err(|e| rodio::source::SeekError::Other(Box::new(e)))?;

        let mut samples_to_pass = seek_res.required_ts - seek_res.actual_ts;
        let packet = loop {
            match self.format.next_packet() {
                Ok(candidate) => {
                    if candidate.dur() > samples_to_pass {
                        break candidate;
                    }
                    samples_to_pass -= candidate.dur();
                }
                Err(e) => return Err(rodio::source::SeekError::Other(Box::new(e))),
            }
        };

        let decoded = self
            .decoder
            .decode(&packet)
            .map_err(|e| rodio::source::SeekError::Other(Box::new(e)))?;

        decoded.spec().clone_into(&mut self.spec);
        let duration = symphonia::core::units::Duration::from(decoded.capacity() as u64);
        let mut buf = SampleBuffer::<i16>::new(duration, self.spec);
        buf.copy_interleaved_ref(decoded);
        self.buffer = buf;
        self.current_frame_offset = samples_to_pass as usize * self.channels() as usize + to_skip;

        Ok(())
    }
}

#[cfg(test)]
mod id3_tests {
    use super::*;
    use std::io::Write;

    fn header(version: u8, flags: u8, size: [u8; 4]) -> [u8; 10] {
        let mut h = [0u8; 10];
        h[..3].copy_from_slice(b"ID3");
        h[3] = version;
        h[4] = 0;
        h[5] = flags;
        h[6..10].copy_from_slice(&size);
        h
    }

    #[test]
    fn empty_id3v2_4_tag_is_ten_bytes() {
        // The exact shape every Deezer preview ships, and the reason the probe
        // failed: a valid header declaring no body at all.
        let h = header(4, 0x00, [0, 0, 0, 0]);
        assert_eq!(id3v2_tag_len(&h), Some(10));
    }

    #[test]
    fn synchsafe_size_is_decoded() {
        // 0x00 0x02 0x01 0x0A -> (2<<14) | (1<<7) | 10 = 32906 body bytes.
        let h = header(3, 0x00, [0x00, 0x02, 0x01, 0x0A]);
        assert_eq!(id3v2_tag_len(&h), Some(10 + 32906));
    }

    #[test]
    fn footer_flag_adds_ten_bytes() {
        let without = id3v2_tag_len(&header(4, 0x00, [0, 0, 0, 100])).unwrap();
        let with = id3v2_tag_len(&header(4, 0x10, [0, 0, 0, 100])).unwrap();
        assert_eq!(with - without, 10);
    }

    #[test]
    fn non_id3_input_is_rejected() {
        // A bare MPEG frame sync must not be mistaken for a tag.
        let mut mpeg = [0u8; 10];
        mpeg[0] = 0xFF;
        mpeg[1] = 0xFB;
        assert_eq!(id3v2_tag_len(&mpeg), None);
        assert_eq!(id3v2_tag_len(b"RIFFxxxxWA"), None);
        assert_eq!(id3v2_tag_len(b"ID3"), None, "truncated header");
    }

    #[test]
    fn malformed_size_bytes_are_rejected() {
        // Synchsafe integers never set the high bit; if one is set the header
        // is not trustworthy and we must not seek by a bogus length.
        assert_eq!(id3v2_tag_len(&header(4, 0x00, [0x80, 0, 0, 0])), None);
        assert_eq!(id3v2_tag_len(&header(0xFF, 0x00, [0, 0, 0, 1])), None);
    }

    /// Diagnostic for "why won't this file play": reports the tag shape and
    /// whether the engine can open it.
    /// `WAVE_PROBE_FILE=<path> cargo test --lib probe_file -- --ignored --nocapture`
    #[test]
    #[ignore = "needs WAVE_PROBE_FILE"]
    fn probe_file_from_env() {
        // A diagnostic, not a gate: skip quietly when run as part of the whole
        // ignored suite rather than failing it.
        let Ok(path) = std::env::var("WAVE_PROBE_FILE") else {
            println!("skipped — set WAVE_PROBE_FILE to probe a file");
            return;
        };
        let mut header = [0u8; 10];
        {
            let mut f = File::open(&path).expect("open failed");
            let _ = f.read_exact(&mut header);
        }
        println!("path      {path}");
        println!("id3 tag   {:?} bytes", id3v2_tag_len(&header));
        match SymphoniaSource::new(&path) {
            Ok(_) => println!("decode    OK"),
            Err(e) => panic!("decode    FAILED: {e}"),
        }
    }

    #[test]
    fn offset_source_hides_the_leading_bytes() {
        let path = std::env::temp_dir().join(format!("wave-offset-{}.bin", std::process::id()));
        let mut f = File::create(&path).unwrap();
        f.write_all(b"HEADERabcdefghij").unwrap();
        drop(f);
        let path_str = path.to_string_lossy().into_owned();

        let mut src = OffsetMediaSource::new(&path_str, 6).unwrap();
        assert_eq!(src.byte_len(), Some(10), "length excludes the skipped tag");

        let mut buf = [0u8; 4];
        src.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"abcd", "reads begin after the offset");

        // An absolute seek to 0 must land on the audio, not the tag.
        assert_eq!(src.seek(SeekFrom::Start(0)).unwrap(), 0);
        src.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"abcd");

        assert_eq!(src.seek(SeekFrom::Start(4)).unwrap(), 4);
        src.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"efgh");

        let _ = std::fs::remove_file(&path);
    }
}
