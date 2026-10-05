//! Files list row (D1). Owned by workstream D.

use uuid::Uuid;

/// What one row of the Files list shows. `screens_v2::files` builds these from
/// the snapshot; the row component draws them.
#[derive(Clone, Debug, PartialEq)]
pub struct RowLook {
    pub id: Uuid,
    /// Upload-state glyph from `kit::icons` and its colour.
    pub icon: &'static str,
    pub icon_color: u32,
    /// "Today, 14:02".
    pub title: String,
    /// "42 min, upload failed".
    pub line: String,
}
