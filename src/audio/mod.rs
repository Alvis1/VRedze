//! Audio output: interleaved float samples at [`RATE`] Hz, [`CHANNELS`]
//! channels, written from the audio thread, which paces itself by the writes.
//! PulseAudio on Linux (Steam Frame), AAudio on Android (Meta Quest).

pub const RATE: u32 = 48_000;
pub const CHANNELS: u32 = 2;

#[cfg(target_os = "android")]
mod aaudio;
#[cfg(not(target_os = "android"))]
mod pulse;

#[cfg(target_os = "android")]
pub use aaudio::Output;
#[cfg(not(target_os = "android"))]
pub use pulse::Output;
