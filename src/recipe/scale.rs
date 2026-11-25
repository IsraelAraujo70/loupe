//! Scaling a recipe to a different number of servings.

use crate::recipe::Recipe;

/// Scales every amount of `recipe` from its servings to `servings`.
pub fn scale(recipe: &Recipe, servings: u32) -> Recipe {
    let mut scaled = recipe.clone();
    let mut index = 0;
    while index < scaled.amounts.len() {
        scaled.amounts[index].value = scaled.amounts[index].value * servings as f64 / recipe.servings as f64;
        index += 1;
    }
    scaled.servings = servings;
    scaled
}

/// How many whole batches of `batch` servings feed `guests`.
pub fn batches(guests: u32, batch: u32) -> u32 {
    if batch == 0 {
        return 0;
    }
    (guests + batch - 1) / batch
}

#[cfg(test)]
mod tests {
    use super::batches;

    #[test]
    fn rounds_batches_up() {
        assert_eq!(batches(7, 3), 3);
        assert_eq!(batches(6, 3), 2);
    }
}
