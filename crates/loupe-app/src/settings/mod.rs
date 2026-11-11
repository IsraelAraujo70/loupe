//! Accounts (GitHub, Linear, T3) and preferences. Owned by the shell front.
//!
//! - [`auth`]: blocking credential checks and the GitHub OAuth device flow.
//! - [`session`]: moves `Services` and `Accounts` on sign-in and sign-out.
//! - [`sign_in`]: the full-window sign-in screen shown while GitHub is not connected.
//! - [`view`]: the `Route::Settings` screen; [`catalog`] lists its sections and
//!   settings, [`accounts`] draws its account cards, [`workspace_map`] the
//!   Linear workspace of each GitHub org or repository, [`appearance`] its
//!   Appearance section (color scheme, themes, interface, typography), and
//!   [`shortcuts`] its Keyboard shortcuts section.

mod accounts;
mod appearance;
pub mod auth;
pub mod catalog;
pub mod session;
pub mod shortcuts;
pub mod sign_in;
pub mod view;
mod workspace_map;

pub use sign_in::sign_in_view;
pub use view::settings_view;
