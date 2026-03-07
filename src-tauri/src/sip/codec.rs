//! Audio codec re-exports from rtp-engine.

pub use rtp_engine::codec::negotiate_codec;

// Re-export additional types that may be needed
#[allow(unused_imports)]
pub use rtp_engine::codec::{create_decoder, create_encoder, AudioDecoder, AudioEncoder, CodecType};
