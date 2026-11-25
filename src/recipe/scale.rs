//! Scaling a recipe to a different number of servings.

use crate::recipe::{Recipe, convert};

/// Scales every amount of `recipe` from its servings to `servings`.
pub fn scale(recipe: &Recipe, servings: u32) -> Recipe {
    let factor = f64::from(servings) / f64::from(recipe.servings.max(1));
    let mut scaled = recipe.clone();
    for amount in &mut scaled.amounts {
        amount.value *= factor;
    }
    scaled.servings = servings;
    scaled
}

/// [`scale`], then every amount in metric units.
pub fn scale_metric(recipe: &Recipe, servings: u32) -> Recipe {
    convert::metric(&scale(recipe, servings))
}

/// How many whole batches of `batch` servings feed `guests`; `None` when a
/// batch feeds nobody.
pub fn batches(guests: u32, batch: u32) -> Option<u32> {
    guests.checked_add(batch.checked_sub(1)?)?.checked_div(batch)
}

#[cfg(test)]
mod tests {
    use super::batches;

    #[test]
    fn rounds_batches_up() {
        assert_eq!(batches(7, 3), Some(3));
        assert_eq!(batches(6, 3), Some(2));
        assert_eq!(batches(6, 0), None);
    }
}
