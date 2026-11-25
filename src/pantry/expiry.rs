//! When stock expires.

use crate::catalog;

/// Days until an item bought `days_ago` days ago expires; `None` for an
/// ingredient the catalog does not know.
pub fn days_left(ingredient: &str, days_ago: u16) -> Option<i32> {
    let item = catalog::find(ingredient)?;
    Some(i32::from(item.shelf_days) - i32::from(days_ago))
}

/// Whether an item bought `days_ago` days ago is past its shelf life.
pub fn is_expired(ingredient: &str, days_ago: u16) -> bool {
    days_left(ingredient, days_ago).is_some_and(|left| left < 0)
}
