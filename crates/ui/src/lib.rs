//! Touch UI drawn in the same cell grid as the terminal: the on-screen
//! keyboard and the home screen. Everything goes through `Panel`, so the
//! layouts are tested against `FakePanel` on the host.

pub mod draw;
pub mod home;
pub mod osk;

pub use home::{AddForm, FormAction, Home, HomeAction, HostEntry};
pub use osk::{Osk, OskAction, OSK_ROWS};
