//! Transactional reads for the 3DS soundtrack. A buffer pair is published only
//! after both channels have been read and prepared; a failed read is terminal.
//! The backend retains ownership of DSP-visible memory and queue lifetimes.

use std::io::{self, Read, Seek, SeekFrom};

use crate::adpcm::{self, Adp};

pub struct Source<R> {
    reader: R,
    pub header: Adp,
    pub starts: [u16; 2],
    offsets: [u64; 2],
    next: usize,
    failed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub samples: usize,
    pub first: bool,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid ADPCM stream")
}

impl<R: Read + Seek> Source<R> {
    pub fn open(mut reader: R) -> io::Result<Self> {
        let mut bytes = vec![0; 24];
        reader.read_exact(&mut bytes)?;
        let channels = u32::from_le_bytes(bytes[16..20].try_into().unwrap()) as usize;
        if !(1..=2).contains(&channels) {
            return Err(invalid());
        }
        bytes.resize(adpcm::header_len(channels), 0);
        reader.read_exact(&mut bytes[24..])?;
        let header = Adp::header(&bytes).ok_or_else(invalid)?;
        if header.samples == 0 {
            return Err(invalid());
        }
        let len = adpcm::data_len(header.samples as usize) as u64;
        let base = bytes.len() as u64;
        for c in 0..channels {
            let at = 24 + c * 36 + 32;
            let declared = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as u64;
            if declared != len {
                return Err(invalid());
            }
        }
        if reader.seek(SeekFrom::End(0))? < base + len * channels as u64 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated ADPCM payload",
            ));
        }
        let offsets = [base, if channels == 2 { base + len } else { base }];
        let mut starts = [0; 2];
        for (c, start) in starts.iter_mut().enumerate() {
            reader.seek(SeekFrom::Start(offsets[c]))?;
            let mut byte = [0];
            reader.read_exact(&mut byte)?;
            *start = byte[0] as u16;
        }
        Ok(Self {
            reader,
            header,
            starts,
            offsets,
            next: 0,
            failed: false,
        })
    }

    /// Stage both channels, then invoke a backend's pair submission exactly
    /// once. `submit` must finish preparation (including cache flushes) before
    /// publishing either channel. Failure permanently stops this source.
    pub fn fill_pair(
        &mut self,
        buffers: [&mut [u8]; 2],
        chunk_samples: usize,
        submit: impl FnOnce(Chunk, [&[u8]; 2]) -> io::Result<()>,
    ) -> io::Result<bool> {
        if self.failed {
            return Err(io::Error::other("ADPCM stream already failed"));
        }
        if self.next >= self.header.samples as usize {
            return Ok(false);
        }
        let samples = chunk_samples.min(self.header.samples as usize - self.next);
        if samples == 0 || !chunk_samples.is_multiple_of(adpcm::FRAME_SAMPLES) {
            self.failed = true;
            return Err(invalid());
        }
        let len = adpcm::data_len(samples);
        let [left, right] = buffers;
        if left.len() < len || right.len() < len {
            self.failed = true;
            return Err(invalid());
        }
        let (left, right) = (&mut left[..len], &mut right[..len]);
        let at = (self.next / adpcm::FRAME_SAMPLES * adpcm::FRAME_BYTES) as u64;
        let result = (|| {
            self.reader.seek(SeekFrom::Start(self.offsets[0] + at))?;
            self.reader.read_exact(left)?;
            if self.header.channels.len() == 2 {
                self.reader.seek(SeekFrom::Start(self.offsets[1] + at))?;
                self.reader.read_exact(right)?;
            } else {
                right.copy_from_slice(left);
            }
            submit(
                Chunk {
                    samples,
                    first: self.next == 0,
                },
                [left, right],
            )
        })();
        if let Err(e) = result {
            self.failed = true;
            return Err(e);
        }
        self.next += samples;
        Ok(true)
    }

    /// Failed streams never block the next queued track, even if the DSP has
    /// not yet observed the backend's queue clear. EOF drains admitted buffers.
    pub fn playing(&self, queued: bool) -> bool {
        !self.failed && (self.next < self.header.samples as usize || queued)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adpcm::Channel;
    use std::io::Cursor;

    fn bytes(channels: usize) -> Vec<u8> {
        Adp {
            rate: 22050,
            samples: 42,
            gain: 1.0,
            channels: (0..channels)
                .map(|i| Channel {
                    coefs: [0; 16],
                    data: vec![i as u8; 24],
                })
                .collect(),
        }
        .to_bytes()
    }

    #[test]
    fn truncated_headers_and_each_channel_payload_fail_before_submission() {
        let valid = bytes(2);
        for cut in 0..valid.len() {
            assert!(
                Source::open(Cursor::new(&valid[..cut])).is_err(),
                "cut {cut}"
            );
        }
        let mut wrong_len = valid.clone();
        wrong_len[56..60].copy_from_slice(&16u32.to_le_bytes());
        assert!(Source::open(Cursor::new(wrong_len)).is_err());
        assert!(Source::open(Cursor::new(valid)).is_ok());
    }

    struct FailingReader {
        inner: Cursor<Vec<u8>>,
        fail_at: u64,
        enabled: bool,
    }
    impl Read for FailingReader {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if self.enabled && self.inner.position() >= self.fail_at {
                Err(io::Error::other("injected SD error"))
            } else {
                self.inner.read(out)
            }
        }
    }
    impl Seek for FailingReader {
        fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
            self.inner.seek(p)
        }
    }

    #[test]
    fn failed_initial_or_refill_read_never_publishes_half_a_pair_and_is_terminal() {
        for fail_channel in 0..2 {
            for prior_chunks in 0..2 {
                let mut source = Source::open(FailingReader {
                    inner: Cursor::new(bytes(2)),
                    fail_at: (adpcm::header_len(2) + fail_channel * 24 + prior_chunks * 8) as u64,
                    enabled: false,
                })
                .unwrap();
                let (mut left, mut right) = ([0; 8], [0; 8]);
                let mut submitted = 0;
                for _ in 0..prior_chunks {
                    source
                        .fill_pair([&mut left, &mut right], 14, |_, _| {
                            submitted += 1;
                            Ok(())
                        })
                        .unwrap();
                }
                source.reader.enabled = true;
                assert!(source
                    .fill_pair([&mut left, &mut right], 14, |_, _| {
                        submitted += 1;
                        Ok(())
                    })
                    .is_err());
                assert_eq!(submitted, prior_chunks);
                assert!(!source.playing(true));
                source.reader.enabled = false;
                assert!(source
                    .fill_pair([&mut left, &mut right], 14, |_, _| panic!(
                        "terminal source retried"
                    ))
                    .is_err());
            }
        }
    }

    #[test]
    fn mono_pairs_match_eof_drains_and_submission_failure_stops() {
        let mut source = Source::open(Cursor::new(bytes(1))).unwrap();
        let (mut left, mut right) = ([0; 16], [0; 16]);
        let mut chunks = Vec::new();
        while source
            .fill_pair([&mut left, &mut right], 28, |c, pair| {
                assert_eq!(pair[0], pair[1]);
                chunks.push(c);
                Ok(())
            })
            .unwrap()
        {}
        assert_eq!(
            chunks,
            [
                Chunk {
                    samples: 28,
                    first: true
                },
                Chunk {
                    samples: 14,
                    first: false
                }
            ]
        );
        assert!(source.playing(true));
        assert!(!source.playing(false));
        let mut source = Source::open(Cursor::new(bytes(2))).unwrap();
        assert!(source
            .fill_pair([&mut left, &mut right], 28, |_, _| Err(io::Error::other(
                "cache flush"
            )))
            .is_err());
        assert!(!source.playing(true));
    }
}
