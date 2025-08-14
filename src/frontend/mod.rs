pub mod frontend;
pub mod state;
pub mod buffer;
pub mod dispatch;
pub mod gtk_overlay;
pub mod client;

// Legacy compatibility
pub mod sync_client {
    pub use super::client::SyncFrontendClient;
}

pub use frontend::*;
