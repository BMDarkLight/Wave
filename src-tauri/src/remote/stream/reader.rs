// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! A blocking reader over a cache file that is still being filled.

use super::fill::{Fill, WaitError};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::time::Duration;
use symphonia::core::io::MediaSource;

/// Each read waits up to `timeout` for the fill to reach it. While a reader
/// exists its fill counts as active, which keeps the file from eviction.
pub struct ProgressiveReader {
    fill: Arc<Fill>,
    file: File,
    pos: u64,
    timeout: Duration,
}

impl ProgressiveReader {
    pub fn new(fill: Arc<Fill>, timeout: Duration) -> io::Result<Self> {
        let file = File::open(&fill.path)?;
        fill.add_reader();
        Ok(Self {
            fill,
            file,
            pos: 0,
            timeout,
        })
    }
}

impl Drop for ProgressiveReader {
    fn drop(&mut self) {
        self.fill.remove_reader();
    }
}

impl Read for ProgressiveReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.pos >= self.fill.len {
            return Ok(0);
        }
        let available = self
            .fill
            .wait(self.pos, 1, self.timeout)
            .map_err(|error| match error {
                WaitError::TimedOut => io::Error::new(io::ErrorKind::TimedOut, String::from(error)),
                WaitError::Failed(message) => io::Error::other(message),
            })?;
        let n = buf
            .len()
            .min(usize::try_from(available).unwrap_or(usize::MAX));
        self.file.seek(SeekFrom::Start(self.pos))?;
        let read = self.file.read(&mut buf[..n])?;
        self.pos += read as u64;
        Ok(read)
    }
}

impl Seek for ProgressiveReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(delta) => self.fill.len.checked_add_signed(delta),
            SeekFrom::Current(delta) => self.pos.checked_add_signed(delta),
        };
        let Some(target) = target else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before the start of the stream",
            ));
        };
        self.pos = target;
        Ok(target)
    }
}

impl MediaSource for ProgressiveReader {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.fill.len)
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::*;
    use super::*;
    use crate::remote::RemoteEvent;
    use std::time::Instant;

    const LONG: Duration = Duration::from_secs(20);
    /// About 1 MiB/s.
    const MEDIUM: Duration = Duration::from_millis(16);

    #[test]
    fn reader_blocks_then_returns_bytes_in_order() {
        let h = Harness::new(3 * MIB as usize);
        let fill = h.spawn(Arc::new(FakeBackend::new(h.data.clone(), MEDIUM)));
        let mut reader = ProgressiveReader::new(fill.clone(), LONG).unwrap();
        assert!(reader.is_seekable());
        assert_eq!(reader.byte_len(), Some(3 * MIB));
        assert_eq!(fill.readers(), 1);

        let mut out = Vec::new();
        reader.read_to_end(&mut out).unwrap();
        assert!(out == *h.data, "bytes differ from the source");
        assert_eq!(reader.read(&mut [0u8; 8]).unwrap(), 0);

        drop(reader);
        assert_eq!(fill.readers(), 0);
    }

    #[test]
    fn seek_far_ahead_triggers_ranged_fetch() {
        let h = Harness::new(3 * MIB as usize);
        let backend = Arc::new(FakeBackend::new(h.data.clone(), SLOW));
        let fill = h.spawn(backend.clone());
        let mut reader = ProgressiveReader::new(fill, LONG).unwrap();

        let target = 5 * MIB / 2;
        assert_eq!(reader.seek(SeekFrom::Start(target)).unwrap(), target);
        let started = Instant::now();
        let mut buf = [0u8; 4096];
        let n = reader.read(&mut buf).unwrap();
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_millis(1500), "took {elapsed:?}");
        assert!(n > 0);
        let at = target as usize;
        assert_eq!(&buf[..n], &h.data[at..at + n]);
        assert!(backend.opens().iter().any(|&offset| offset >= target));

        assert_eq!(
            reader.seek(SeekFrom::Current(-10)).unwrap(),
            target + n as u64 - 10
        );
        assert_eq!(reader.seek(SeekFrom::End(-1)).unwrap(), 3 * MIB - 1);
        assert!(reader.seek(SeekFrom::Current(-(4 * MIB as i64))).is_err());
    }

    #[test]
    fn reader_times_out_with_timed_out_kind() {
        let h = Harness::new(MIB as usize);
        let fill = h.spawn(Arc::new(FakeBackend::new(h.data.clone(), FAST).silent()));
        let mut reader = ProgressiveReader::new(fill, Duration::from_millis(200)).unwrap();
        let started = Instant::now();
        let err = reader.read(&mut [0u8; 16]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(200), "took {elapsed:?}");
        assert!(elapsed < Duration::from_secs(3), "took {elapsed:?}");
    }

    #[test]
    fn a_failed_fill_errors_the_reader_and_reports_it() {
        let h = Harness::new(MIB as usize);
        let fill = h.spawn(Arc::new(
            FakeBackend::new(h.data.clone(), FAST).failing_at(64 * 1024),
        ));
        let mut reader = ProgressiveReader::new(fill, LONG).unwrap();
        reader.seek(SeekFrom::Start(128 * 1024)).unwrap();
        let err = reader.read(&mut [0u8; 16]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Other);
        assert_eq!(err.to_string(), "Lost the connection to the server");
        assert!(wait_until(LONG, || h.events().contains(
            &RemoteEvent::StreamFailed {
                path: h.uri.to_path(),
                message: "Lost the connection to the server".into(),
            }
        )));
    }

    #[test]
    fn decodes_flac_fixture_while_downloading() {
        use crate::audio::symphonia_source::SymphoniaSource;

        let frames = 3 * 44_100;
        let wav = wav_bytes(frames);
        let h = Harness::new(wav.len());
        let data = Arc::new(wav);
        let fill = h.spawn(Arc::new(FakeBackend::new(data.clone(), SLOW)));
        let reader = ProgressiveReader::new(fill.clone(), LONG).unwrap();

        let source =
            SymphoniaSource::from_media_source(Box::new(reader), Some("wav"), &h.uri.to_path())
                .unwrap();
        assert!(
            !fill.is_complete(),
            "probing should not need the whole file"
        );
        assert_eq!(source.count(), frames * 2);
    }
}
