mod build;
pub mod json;
mod layout;
mod logic;
mod read;

#[cfg(test)]
mod tests;

pub use build::{escape, has_glyph, movie_path, movie_url};
pub use layout::{
    Align, CellDef, Child, ChildKind, ImportDef, LAYERS, Layout, RectDef, SymbolDef, TextDef, valid_component_url,
};
pub use logic::ImageBinding;
pub use logic::{
    Base, BillboardDef, CalloutsDef, DropPanelDef, ICONS, InputDef, InputDo, KEYS, Logic, MenuDef, MenuKind, MenuOp,
    OnEnter, PRIORITIES, PagingDef, PanelDef, PanelHeight, SOUNDS, ScrollBarDef, SdDef, TextBinding, TextFit,
    UI_EVENTS, VisibleBinding,
};

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutError {
    pub file: String,
    pub pos: json::Pos,
    pub message: String,
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.file, self.pos, self.message)
    }
}

impl std::error::Error for LayoutError {}

pub mod limits {
    use super::json::Limits;

    // PORT(choice): the layout limits below are set far above the port's own layouts (the keyboard: 4 imports, 2
    // symbols, 8 children, 16 inputs, 4.3 KiB) and below what a frame can draw; tuning: the layouts phase 7's resources
    // bring, measured; the original has no such layouts.

    pub const JSON: Limits = Limits { max_bytes: 256 * 1024, max_depth: 16, max_values: 20_000, max_string: 4096 };
    pub const NAME: usize = 64;
    pub const URL: usize = 128;
    pub const PATH: usize = 256;
    pub const TEXT: usize = 1024;
    pub const SEPARATOR: usize = 8;
    pub const IMPORTS: usize = 32;
    pub const SYMBOLS: usize = 64;
    pub const CHILDREN: usize = 256;
    pub const INSTANCES: u64 = 4096;
    pub const DATA: usize = 16;
    pub const BINDINGS: usize = 64;
    pub const MENUS: usize = 8;
    pub const CALLOUTS: usize = 8;
    pub const INPUTS: usize = 32;
    pub const GRID: i64 = 32;
    pub const STAGE: i64 = 4096;
    pub const COORD: f64 = 8192.0;
    pub const SCALE: f64 = 64.0;
    pub const EXPORT_PREFIX: &str = "Ava";
}
