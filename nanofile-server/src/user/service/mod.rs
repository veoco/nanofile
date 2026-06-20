mod account_service;
mod avatar_service;
mod device_service;
mod invitation_service;

pub use account_service::{AccountInfo, AccountService};
pub use avatar_service::AvatarService;
pub use device_service::DeviceService;
pub use invitation_service::{InvitationInfo, InvitationService};

// Re-export avatar utility functions used by other modules
pub use avatar_service::{
    avatar_storage_dir, default_avatar_bytes, primary_avatar_url, resolve_size,
};
