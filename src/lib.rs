//! VRedze: VR video player for Steam Frame and Meta Quest, reading from the
//! headset's own storage or directly from SMB shares. Forked from Just Video.

#[cfg(all(target_os = "android", feature = "decode"))]
mod android;
#[cfg(feature = "decode")]
pub mod audio;
pub mod config;
#[cfg(feature = "decode")]
pub mod decode;
pub mod inventory;
#[cfg(feature = "decode")]
pub mod library;
pub mod local;
#[cfg(feature = "decode")]
pub mod media;
pub mod platform;
#[cfg(feature = "decode")]
pub mod playability;
pub mod readahead;
pub mod smb;
pub mod srvsvc;
pub mod subtitles;
#[cfg(feature = "decode")]
pub mod ui;
#[cfg(feature = "decode")]
pub mod vr;
#[cfg(feature = "decode")]
pub mod xr;
