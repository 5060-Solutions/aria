//! Audio codec re-exports from rtp-engine.

// No `negotiate_codec` here: its pick ignores what the softphone carries.
// Calls negotiate with `builder::negotiate_audio`, the list the SDP answer is
// built from.

// Re-export additional types that may be needed
#[allow(unused_imports)]
pub use rtp_engine::codec::{
    create_decoder, create_encoder, AudioDecoder, AudioEncoder, CodecType,
};
