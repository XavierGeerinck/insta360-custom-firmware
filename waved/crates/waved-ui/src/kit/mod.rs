//! UI kit for the revision-2 design (`docs/mockups`). New screens build on
//! these pieces instead of raw LVGL calls; see `src/kit/README.md`.

pub mod components;
pub mod direction;
pub mod dispatch;
pub mod event;
pub mod fonts;
pub mod icons;
pub mod obj;
pub mod route;
pub mod router;
pub mod screen;
pub mod snapshot;
#[cfg(test)]
pub mod testkit;
pub mod time;
pub mod tokens;
