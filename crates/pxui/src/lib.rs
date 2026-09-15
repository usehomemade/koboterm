//! Reader-style chrome drawn at pixel level: the top bar and the popups,
//! matching Nickel's reading view (Rakuten Sans UI for labels, Rakuten Serif
//! for popup titles, thin line icons, double-ring sliders).

pub mod icons;
pub mod popup;
pub mod topbar;

use px::Face;

/// Faces the chrome is drawn with. Loaded from the device at start-up.
pub struct Fonts {
    pub sans: Face,
    pub sans_bold: Face,
    pub serif: Face,
}

impl Fonts {
    /// Nickel's own UI fonts, present on every Kobo.
    pub const DIR: &'static str = "/usr/local/Trolltech/QtEmbedded-4.6.2-arm/lib/fonts";

    pub fn load_device() -> Option<Fonts> {
        let d = Self::DIR;
        let sans = Face::load(&format!("{d}/RakutenSansUIApp-Regular.ttf")).or_else(|| Face::load(&format!("{d}/NotoSans-Regular.ttf")))?;
        let sans_bold = Face::load(&format!("{d}/RakutenSansUIApp-Bold.ttf")).or_else(|| Face::load(&format!("{d}/NotoSans-Bold.ttf")))?;
        let serif = Face::load(&format!("{d}/RakutenSerifApp-Regular.ttf")).or_else(|| Face::load(&format!("{d}/Vollkorn-Regular.ttf")))?;
        Some(Fonts { sans, sans_bold, serif })
    }
}

pub const BLACK: u8 = 0x00;
pub const WHITE: u8 = 0xFF;
pub const GRAY: u8 = 0x99;
pub const LIGHT: u8 = 0xCC;
