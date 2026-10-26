//! Terminal pseudo-TTY portable.
//!
//! - Unix : vrai pty (`openpty`), redimensionnable, signaux de groupe.
//! - Windows : pipes stdin/stdout fusionnés, sans fenêtre de console ;
//!   le redimensionnement est ignoré.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::{PtyReader, PtySession, spawn_in_pty};
#[cfg(windows)]
pub use windows::{PtyReader, PtySession, spawn_in_pty};

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub struct Winsize {
    pub rows: u16,
    pub cols: u16,
}

pub fn winsize(rows: u16, cols: u16) -> Winsize {
    Winsize { rows, cols }
}
