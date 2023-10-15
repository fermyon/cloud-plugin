pub mod client;
mod client_interface;
mod cloud_client_extensions;

pub use client_interface::CloudClientInterface;
#[cfg(feature = "mocks")]
pub use client_interface::MockCloudClientInterface;
pub use cloud_client_extensions::CloudClientExt;

pub const DEFAULT_APPLIST_PAGE_SIZE: i32 = 50;
