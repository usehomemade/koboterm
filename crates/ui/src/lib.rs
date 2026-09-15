//! Touch UI drawn in the same cell grid as the terminal: the on-screen
//! keyboard and the home screen. Everything goes through `Panel`, so the
//! layouts are tested against `FakePanel` on the host.

pub mod draw;
pub mod home;
pub mod osk;
pub mod popup;
pub mod topbar;

pub use home::{AddForm, FormAction, Home, HomeAction, HostEntry, TextSize};
pub use osk::{osk_rows, Osk, OskAction, OSK_ROWS};
pub use topbar::{Status, TopAction, TopBar, TOPBAR_ROWS};
pub use popup::{Popup, PopupHit};
