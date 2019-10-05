mod error;
mod keys;
mod wallet;
mod wallets;
pub mod xwallets;

pub use error::Error;
pub use wallet::Wallet;
pub use wallets::Wallets;

pub type Result<T> = std::result::Result<T, Error>;
