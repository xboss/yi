//! In-process CTranslate2 and local HTTP model adapters.
#[cfg(feature = "native-ct2")]
pub mod ct2;
#[cfg(feature = "local-chat")]
pub mod local_chat;
