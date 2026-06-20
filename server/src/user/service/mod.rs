mod account;
mod avatar;
mod device;
mod invitation;

pub use account::{AccountInfo, AccountService};
pub use avatar::AvatarService;
pub use device::DeviceService;
pub use invitation::{InvitationInfo, InvitationService};

// Re-export avatar utility functions used by other modules
pub use avatar::{avatar_storage_dir, default_avatar_bytes, primary_avatar_url, resolve_size};
