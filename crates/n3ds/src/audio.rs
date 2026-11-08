//! 3DS sound: the DSP plays the `.adp` clips itself (DSP-ADPCM is its own
//! format), so mixing costs the CPU nothing. Effects load into linear
//! memory the first time they play and stay cached up to `BUDGET` (least
//! recently used go first); the soundtrack streams from the SD card half a
//! second at a time on two channels (left and right).
//!
//! Needs `sdmc:/3ds/dspfirm.cdc` (made by DSP1 on any custom firmware
//! setup); without it the game runs silent.

use std::fs::File;
use std::io::Read;

use ctru_sys as ctru;
use game::data::{AudioClipId, AudioDef};
use scene::adpcm::{self, Adp};
use scene::sound::{Mixer, Voice};
use scene::stream;

const CHANNELS: usize = 24;
/// The soundtrack's two channels.
const MUSIC: [usize; 2] = [22, 23];
/// Samples per streamed buffer (a whole number of frames, ~0.5 s).
const CHUNK: usize = adpcm::FRAME_SAMPLES * 800;
const CHUNK_BYTES: usize = CHUNK / adpcm::FRAME_SAMPLES * adpcm::FRAME_BYTES;
const BUFFERS: usize = 3;
/// Linear memory for cached effects (textures already take ~14 MB of it).
const BUDGET: usize = 5 << 20;
/// Frames before a clip that failed to load is tried again.
const RETRY: u32 = 600;

/// An effect clip in linear memory.
struct Clip {
    data: *mut u8,
    bytes: usize,
    samples: u32,
    rate: f32,
    coefs: [u16; 16],
    /// Back to the source level (clips are stored normalized).
    gain: f32,
    /// Decoder state at the start (for looping).
    start: Box<ctru::ndspAdpcmData>,
}

struct Stream {
    source: stream::Source<File>,
    bufs: [[Box<ctru::ndspWaveBuf>; BUFFERS]; 2],
    data: [[*mut u8; BUFFERS]; 2],
    starts: [Box<ctru::ndspAdpcmData>; 2],
    /// Generation of this stream (its voice id).
    id: Voice,
    gain: f32,
}

pub struct Audio {
    ok: bool,
    clips: Vec<Option<Clip>>,
    names: Vec<String>,
    /// Each clip's file.
    paths: Vec<String>,
    /// Soundtrack clips (streamed, never cached).
    streamed: Vec<bool>,
    /// Per clip: the frame it last played, and when it last failed to load.
    used: Vec<u32>,
    failed: Vec<Option<u32>>,
    cached: usize,
    frame: u32,
    /// The clip on each channel.
    on: [Option<AudioClipId>; CHANNELS],
    dir: String,
    /// Per channel: its wave buffer and the voice playing on it.
    bufs: Vec<Box<ctru::ndspWaveBuf>>,
    voice: [Voice; CHANNELS],
    /// The playing clip's gain on each channel.
    level: [f32; CHANNELS],
    next: Voice,
    stream: Option<Stream>,
    music_mix: (f32, f32),
    /// Loads that failed: file missing, unreadable (older format), no memory.
    missing: u32,
    outdated: u32,
    no_memory: u32,
}

/// Sets a channel's levels. The DSP can't amplify a voice, so the gain stops
/// at 1 (clips are stored normalized to make up for it).
fn mix(ch: usize, gain: f32, pan: f32) {
    let gain = gain.clamp(0.0, 1.0);
    let mut m = [0.0f32; 12];
    m[0] = gain * (1.0 - pan).min(1.0);
    m[1] = gain * (1.0 + pan).min(1.0);
    unsafe { ctru::ndspChnSetMix(ch as i32, m.as_mut_ptr()) };
}

fn setup(ch: usize, rate: f32, coefs: &[u16; 16]) {
    unsafe {
        ctru::ndspChnReset(ch as i32);
        ctru::ndspChnSetInterp(ch as i32, ctru::NDSP_INTERP_LINEAR);
        ctru::ndspChnSetRate(ch as i32, rate);
        ctru::ndspChnSetFormat(ch as i32, ctru::NDSP_FORMAT_MONO_ADPCM as u16);
        let mut c = *coefs;
        ctru::ndspChnSetAdpcmCoefs(ch as i32, c.as_mut_ptr());
    }
}

fn coefs(c: &adpcm::Channel) -> [u16; 16] {
    c.coefs.map(|v| v as u16)
}

impl Audio {
    /// Starts NDSP for the clips in `dir/audio`.
    pub fn new(dir: &str, def: &AudioDef) -> Audio {
        let ok = unsafe { ctru::ndspInit() } >= 0;
        if ok {
            unsafe {
                ctru::ndspSetOutputMode(ctru::NDSP_OUTPUT_STEREO);
                // Boosted levels (see `Player::boost`) round off instead of cracking.
                ctru::ndspSetClippingMode(ctru::NDSP_CLIP_SOFT);
            }
        }
        let n = def.clips.len();
        Audio {
            ok,
            clips: (0..n).map(|_| None).collect(),
            names: def.clips.iter().map(|c| c.name.clone()).collect(),
            paths: def
                .clips
                .iter()
                .map(|c| format!("{dir}/audio/{}.adp", c.name))
                .collect(),
            streamed: def.clips.iter().map(|c| c.stream).collect(),
            used: vec![0; n],
            failed: vec![None; n],
            cached: 0,
            frame: 0,
            on: [None; CHANNELS],
            dir: format!("{dir}/audio"),
            bufs: (0..CHANNELS)
                .map(|_| Box::new(unsafe { std::mem::zeroed() }))
                .collect(),
            voice: [0; CHANNELS],
            level: [1.0; CHANNELS],
            next: 1,
            stream: None,
            music_mix: (0.0, 0.0),
            missing: 0,
            outdated: 0,
            no_memory: 0,
        }
    }

    /// What's wrong with the sound files, for the touch screen: checks the
    /// first clip's header.
    pub fn files_problem(&self) -> Option<String> {
        if !self.ok {
            return Some("No sound: sdmc:/3ds/dspfirm.cdc is missing\n(run DSP1 once)".into());
        }
        let name = self.names.first()?;
        let mut head = vec![0u8; adpcm::header_len(2)];
        let Ok(mut f) = File::open(format!("{}/{name}.adp", self.dir)) else {
            return Some(format!("No sound: copy the audio folder to\n{}", self.dir));
        };
        let n = f.read(&mut head).unwrap_or(0);
        if Adp::header(&head[..n]).is_none() {
            return Some(format!(
                "No sound: the audio folder is out of date,\nrecopy it to {}",
                self.dir
            ));
        }
        None
    }

    /// One line for the debug overlay.
    pub fn status(&self) -> String {
        if !self.ok {
            return "sound: no DSP (dspfirm.cdc?)".into();
        }
        let loaded = self.clips.iter().filter(|c| c.is_some()).count();
        let busy = (0..MUSIC[0])
            .filter(|&ch| unsafe { ctru::ndspChnIsPlaying(ch as i32) })
            .count();
        format!(
            "sound: {loaded} cached {}k  {busy} playing  fail {}/{}/{} (missing/old/mem)  music {} x{:.2}",
            self.cached >> 10,
            self.missing,
            self.outdated,
            self.no_memory,
            if self.stream_playing() { "on" } else { "off" },
            self.music_mix.0,
        )
    }

    /// Frees the least recently used cached clip that isn't playing.
    fn evict(&mut self) -> bool {
        let busy = |c: usize, a: &Audio| {
            (0..MUSIC[0]).any(|ch| {
                a.on[ch] == Some(c as AudioClipId)
                    && (a.bufs[ch].status == ctru::NDSP_WBUF_QUEUED as u8
                        || a.bufs[ch].status == ctru::NDSP_WBUF_PLAYING as u8
                        || unsafe { ctru::ndspChnIsPlaying(ch as i32) })
            })
        };
        let victim = (0..self.clips.len())
            .filter(|&c| self.clips[c].is_some() && !busy(c, self))
            .min_by_key(|&c| self.used[c]);
        let Some(c) = victim else { return false };
        if let Some(clip) = self.clips[c].take() {
            unsafe { ctru::linearFree(clip.data as *mut _) };
            self.cached -= clip.bytes;
        }
        true
    }

    /// Reads clip `c` into linear memory if it isn't there.
    fn load(&mut self, c: usize) -> bool {
        if self.clips[c].is_some() {
            return true;
        }
        if self.failed[c].is_some_and(|f| self.frame.wrapping_sub(f) < RETRY) {
            return false;
        }
        let Ok(b) = std::fs::read(&self.paths[c]) else {
            self.missing += 1;
            self.failed[c] = Some(self.frame);
            return false;
        };
        let a = Adp::from_bytes(&b);
        let Some(ch) = a.as_ref().and_then(|a| a.channels.first()) else {
            self.outdated += 1;
            self.failed[c] = Some(self.frame);
            return false;
        };
        let a = a.as_ref().unwrap();
        let bytes = ch.data.len();
        while self.cached + bytes > BUDGET && self.evict() {}
        let mut data = unsafe { ctru::linearAlloc(bytes) } as *mut u8;
        while data.is_null() && self.evict() {
            data = unsafe { ctru::linearAlloc(bytes) } as *mut u8;
        }
        if data.is_null() {
            self.no_memory += 1;
            self.failed[c] = Some(self.frame);
            return false;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(ch.data.as_ptr(), data, bytes);
            let _ = ctru::DSP_FlushDataCache(data as *const _, bytes as u32);
        }
        self.cached += bytes;
        self.failed[c] = None;
        self.clips[c] = Some(Clip {
            data,
            bytes,
            samples: a.samples,
            rate: a.rate as f32,
            coefs: coefs(ch),
            gain: a.gain,
            start: Box::new(ctru::ndspAdpcmData {
                index: ch.data[0] as u16,
                history0: 0,
                history1: 0,
            }),
        });
        true
    }

    fn channel_of(&self, v: Voice) -> Option<usize> {
        self.voice.iter().position(|&x| x == v && v != 0)
    }

    fn start_stream(&mut self, clip: AudioClipId, gain: f32) -> Option<Voice> {
        self.stop_stream();
        let file = File::open(self.paths.get(clip as usize)?).ok()?;
        // Validate headers and the complete payload before allocating DSP memory.
        let source = stream::Source::open(file).ok()?;
        let a = &source.header;
        let starts = source.starts;
        let alloc = || unsafe { ctru::linearAlloc(CHUNK_BYTES) } as *mut u8;
        let data = [[alloc(), alloc(), alloc()], [alloc(), alloc(), alloc()]];
        if data.iter().flatten().any(|p| p.is_null()) {
            for p in data.iter().flatten().filter(|p| !p.is_null()) {
                unsafe { ctru::linearFree(*p as *mut _) };
            }
            return None;
        }
        for (c, &ch) in MUSIC.iter().enumerate() {
            let src = &a.channels[c.min(a.channels.len() - 1)];
            setup(ch, a.rate as f32, &coefs(src));
        }
        let id = self.next;
        self.next += 1;
        let zero = || Box::new(unsafe { std::mem::zeroed::<ctru::ndspWaveBuf>() });
        let mut s = Stream {
            gain: a.gain,
            source,
            bufs: [[zero(), zero(), zero()], [zero(), zero(), zero()]],
            data,
            starts: starts.map(|index| {
                Box::new(ctru::ndspAdpcmData {
                    index,
                    history0: 0,
                    history1: 0,
                })
            }),
            id,
        };
        // Queue every buffer on both channels before either plays much.
        for b in 0..BUFFERS {
            match Self::fill(&mut s, b) {
                Ok(true) => {}
                Ok(false) => break,
                Err(_) => {
                    Self::clear_stream(s);
                    return None;
                }
            }
        }
        let gain = gain * s.gain;
        self.stream = Some(s);
        self.music_mix = (gain, 0.0);
        mix(MUSIC[0], gain, -1.0);
        mix(MUSIC[1], gain, 1.0);
        Some(id)
    }

    /// Stage and flush both channels before either buffer becomes DSP-visible.
    fn fill(s: &mut Stream, b: usize) -> std::io::Result<bool> {
        let [left, right] = s
            .data
            .map(|channel| unsafe { std::slice::from_raw_parts_mut(channel[b], CHUNK_BYTES) });
        let bufs = &mut s.bufs;
        let starts = &mut s.starts;
        s.source.fill_pair([left, right], CHUNK, |chunk, channels| {
            for dst in channels {
                let result =
                    unsafe { ctru::DSP_FlushDataCache(dst.as_ptr() as *const _, dst.len() as u32) };
                if result < 0 {
                    return Err(std::io::Error::other("DSP stream cache flush failed"));
                }
            }
            for c in 0..2 {
                let wb = &mut bufs[c][b];
                **wb = unsafe { std::mem::zeroed() };
                wb.__bindgen_anon_1.data_adpcm = channels[c].as_ptr() as *mut u8;
                wb.nsamples = chunk.samples as u32;
                wb.adpcm_data = if chunk.first {
                    starts[c].as_mut() as *mut _
                } else {
                    std::ptr::null_mut()
                };
            }
            for c in 0..2 {
                unsafe { ctru::ndspChnWaveBufAdd(MUSIC[c] as i32, bufs[c][b].as_mut() as *mut _) };
            }
            Ok(())
        })
    }

    fn clear_stream(s: Stream) {
        for &ch in &MUSIC {
            unsafe { ctru::ndspChnWaveBufClear(ch as i32) };
        }
        for p in s.data.iter().flatten() {
            unsafe { ctru::linearFree(*p as *mut _) };
        }
    }

    fn stop_stream(&mut self) {
        if let Some(s) = self.stream.take() {
            Self::clear_stream(s);
        }
    }

    /// Refills the soundtrack's finished buffers. Call once a frame.
    pub fn update(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        let Some(s) = &mut self.stream else { return };
        let mut failed = false;
        for b in 0..BUFFERS {
            let done = s.bufs[0][b].status == ctru::NDSP_WBUF_DONE as u8
                && s.bufs[1][b].status == ctru::NDSP_WBUF_DONE as u8;
            if done {
                match Self::fill(s, b) {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(_) => {
                        failed = true;
                        break;
                    }
                }
            }
        }
        if failed {
            self.stop_stream();
        }
    }

    fn stream_playing(&self) -> bool {
        self.stream.as_ref().is_some_and(|s| {
            s.source.playing(s.bufs.iter().flatten().any(|b| {
                b.status == ctru::NDSP_WBUF_QUEUED as u8
                    || b.status == ctru::NDSP_WBUF_PLAYING as u8
            }))
        })
    }
}

impl Mixer for Audio {
    fn play(&mut self, clip: AudioClipId, gain: f32, pan: f32, looping: bool) -> Option<Voice> {
        if !self.ok {
            return None;
        }
        if *self.streamed.get(clip as usize)? {
            return self.start_stream(clip, gain);
        }
        if !self.load(clip as usize) {
            return None;
        }
        self.used[clip as usize] = self.frame;
        // A free effect channel (else the quietest-to-lose: the oldest).
        // WaveBufAdd publishes QUEUED synchronously; service playing state lags.
        let free = (0..MUSIC[0]).find(|&ch| {
            let st = self.bufs[ch].status;
            st != ctru::NDSP_WBUF_QUEUED as u8
                && st != ctru::NDSP_WBUF_PLAYING as u8
                && unsafe { !ctru::ndspChnIsPlaying(ch as i32) }
        });
        let ch = match free {
            Some(ch) => ch,
            None => (0..MUSIC[0]).min_by_key(|&ch| self.voice[ch])?,
        };
        let c = self.clips[clip as usize].as_mut()?;
        setup(ch, c.rate, &c.coefs);
        self.level[ch] = c.gain;
        mix(ch, gain * c.gain, pan);
        let wb = &mut self.bufs[ch];
        **wb = unsafe { std::mem::zeroed() };
        wb.__bindgen_anon_1.data_adpcm = c.data;
        wb.nsamples = c.samples;
        wb.adpcm_data = c.start.as_mut() as *mut _;
        wb.looping = looping;
        unsafe { ctru::ndspChnWaveBufAdd(ch as i32, wb.as_mut() as *mut _) };
        let id = self.next;
        self.next += 1;
        self.voice[ch] = id;
        self.on[ch] = Some(clip);
        Some(id)
    }

    fn set(&mut self, voice: Voice, gain: f32, pan: f32) {
        if let Some(s) = self.stream.as_ref().filter(|s| s.id == voice) {
            let gain = gain * s.gain;
            self.music_mix = (gain, pan);
            mix(MUSIC[0], gain, -1.0);
            mix(MUSIC[1], gain, 1.0);
        } else if let Some(ch) = self.channel_of(voice) {
            mix(ch, gain * self.level[ch], pan);
        }
    }

    fn stop(&mut self, voice: Voice) {
        if self.stream.as_ref().is_some_and(|s| s.id == voice) {
            self.stop_stream();
        } else if let Some(ch) = self.channel_of(voice) {
            unsafe { ctru::ndspChnWaveBufClear(ch as i32) };
            self.voice[ch] = 0;
        }
    }

    fn playing(&self, voice: Voice) -> bool {
        if self.stream.as_ref().is_some_and(|s| s.id == voice) {
            return self.stream_playing();
        }
        self.channel_of(voice)
            .is_some_and(|ch| self.bufs[ch].status != ctru::NDSP_WBUF_DONE as u8)
    }

    fn pause(&mut self, paused: bool) {
        for ch in 0..CHANNELS {
            unsafe { ctru::ndspChnSetPaused(ch as i32, paused) };
        }
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        if self.ok {
            self.stop_stream();
            for ch in 0..CHANNELS {
                unsafe { ctru::ndspChnWaveBufClear(ch as i32) };
            }
            for c in self.clips.iter_mut().filter_map(Option::take) {
                unsafe { ctru::linearFree(c.data as *mut _) };
            }
            unsafe { ctru::ndspExit() };
        }
    }
}
