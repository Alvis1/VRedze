//! Audio output through AAudio (Android 8+), the Quest's native audio API.

use super::{CHANNELS, RATE};
use anyhow::Context;
use ndk::audio::{
    AudioContentType, AudioDirection, AudioFormat, AudioPerformanceMode, AudioSharingMode,
    AudioStream, AudioStreamBuilder, AudioStreamState, AudioUsage, Clockid,
};

/// How long a write may wait for room before giving up for this round.
const WRITE_TIMEOUT_NS: i64 = 200_000_000;
/// About 100 ms of buffer: room for a busy frame, little delay.
const BUFFER_FRAMES: i32 = (RATE / 10) as i32;

pub struct Output {
    stream: AudioStream,
    started: bool,
}

// SAFETY: the stream is used from one thread at a time (the audio thread).
unsafe impl Send for Output {}

fn open_stream() -> anyhow::Result<AudioStream> {
    let stream = AudioStreamBuilder::new()?
        .direction(AudioDirection::Output)
        .sample_rate(RATE as i32)
        .channel_count(CHANNELS as i32)
        .format(AudioFormat::PCM_Float)
        .sharing_mode(AudioSharingMode::Shared)
        .performance_mode(AudioPerformanceMode::None)
        .usage(AudioUsage::Media)
        .content_type(AudioContentType::Movie)
        .open_stream()
        .context("Open the AAudio output")?;
    let _ = stream.set_buffer_size_in_frames(BUFFER_FRAMES);
    Ok(stream)
}

impl Output {
    pub fn open(_name: &str) -> anyhow::Result<Self> {
        Ok(Self {
            stream: open_stream()?,
            started: false,
        })
    }

    /// Blocks until the samples are queued (this paces the audio thread).
    pub fn write(&mut self, samples: &[f32]) -> anyhow::Result<()> {
        let channels = CHANNELS as usize;
        let mut done = 0;
        while done < samples.len() / channels {
            let rest = &samples[done * channels..];
            // SAFETY: `rest` holds at least the frames passed.
            let written = unsafe {
                self.stream.write(
                    rest.as_ptr().cast(),
                    (rest.len() / channels) as i32,
                    if self.started { WRITE_TIMEOUT_NS } else { 0 },
                )
            };
            match written {
                Ok(n) => done += n as usize,
                Err(e) => {
                    // The output device changed (headphones, Bluetooth): reopen.
                    eprintln!("Audio: {e}; reopening the output");
                    self.stream = open_stream()?;
                    self.started = false;
                    continue;
                }
            }
            // Fill the buffer before starting, so playback starts without a gap.
            if !self.started {
                self.stream.request_start().context("Start the AAudio output")?;
                self.started = true;
            }
        }
        Ok(())
    }

    /// Seconds between queued audio and what is heard now.
    pub fn latency(&mut self) -> f64 {
        let written = self.stream.frames_written();
        // The presentation timestamp says which frame was heard when.
        let heard = match self.stream.timestamp(Clockid::Monotonic) {
            Ok(t) => {
                let now = monotonic_ns();
                t.frame_position as f64 + (now - t.time_nanoseconds) as f64 * RATE as f64 / 1e9
            }
            Err(_) => self.stream.frames_read() as f64,
        };
        ((written as f64 - heard) / RATE as f64).max(0.0)
    }

    /// Drops queued audio (after a seek).
    pub fn flush(&mut self) {
        if !self.started {
            return;
        }
        // AAudio flushes only a paused stream.
        let paused = self.stream.request_pause().is_ok()
            && wait_until_not(&self.stream, AudioStreamState::Pausing);
        if paused && self.stream.request_flush().is_ok() {
            wait_until_not(&self.stream, AudioStreamState::Flushing);
        }
        // The next write starts it again once it holds new audio.
        self.started = false;
    }
}

fn wait_until_not(stream: &AudioStream, state: AudioStreamState) -> bool {
    let mut current = stream.state();
    for _ in 0..20 {
        if current != state {
            return true;
        }
        match stream.wait_for_state_change(state, 50_000_000) {
            Ok(next) => current = next,
            Err(_) => return false,
        }
    }
    false
}

fn monotonic_ns() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime writes the timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec * 1_000_000_000 + ts.tv_nsec
}
