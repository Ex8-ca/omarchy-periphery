//! Per-window state we track.
//!
//! A window can be in one of three regimes:
//!   - Normal: not being touched by the periphery interaction.
//!   - Dragging: the user is holding the mouse button and the window's outer
//!     edge has crossed a monitor boundary. We animate its size toward an
//!     attractor.
//!   - Peripheried: the user released past the periphery threshold. We
//!     remember the pre-periphery geometry so we can restore it.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Geometry {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Regime {
    Normal,
    Dragging,
    Peripheried,
}

#[derive(Debug, Clone)]
pub struct WindowState {
    pub class: String,
    pub title: String,
    pub current: Geometry,
    /// Geometry captured just before the user dragged it past the edge.
    /// Used to restore the window to its pre-peripheried position.
    pub pre_peeph: Option<Geometry>,
    /// Last computed fractional size (1.0 = full screen, 0.03 = icon).
    pub last_size_frac: f32,
    pub regime: Regime,
}

impl WindowState {
    pub fn new(class: String, title: String, current: Geometry) -> Self {
        Self {
            class,
            title,
            current,
            pre_peeph: None,
            last_size_frac: 1.0,
            regime: Regime::Normal,
        }
    }

    pub fn is_peripheried(&self) -> bool {
        self.regime == Regime::Peripheried
    }

    pub fn last_size_frac(&self) -> f32 {
        self.last_size_frac
    }

    pub fn pre_peeph_geometry(&self) -> Option<Geometry> {
        self.pre_peeph
    }

    pub fn mark_restored(&mut self) {
        self.regime = Regime::Normal;
        self.pre_peeph = None;
        self.last_size_frac = 1.0;
    }
}