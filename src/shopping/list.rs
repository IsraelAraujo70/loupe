//! What to buy: the planned amounts minus what is in stock.

use std::collections::BTreeMap;

use crate::pantry::stock::Stock;
use crate::recipe::Recipe;

/// Missing amount per ingredient for `recipes`, given `stock`.
pub fn missing(recipes: &[Recipe], stock: &Stock) -> BTreeMap<String, f64> {
    let mut needed: BTreeMap<String, f64> = BTreeMap::new();
    for recipe in recipes {
        for amount in &recipe.amounts {
            *needed.entry(amount.ingredient.clone()).or_default() += amount.value;
        }
    }
    needed
        .into_iter()
        .map(|(name, value)| {
            let left = value - stock.amount(&name);
            (name, left)
        })
        .filter(|(_, left)| *left > 0.0)
        .collect()
}
