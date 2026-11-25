//! Items in stock and their amounts.

use std::collections::BTreeMap;

/// What the pantry holds, by ingredient name.
#[derive(Default, Debug)]
pub struct Stock {
    items: BTreeMap<String, f64>,
}

impl Stock {
    /// Adds `amount` of `ingredient`.
    pub fn add(&mut self, ingredient: &str, amount: f64) {
        *self.items.entry(ingredient.to_owned()).or_default() += amount;
    }

    /// Takes `amount` of `ingredient`; false when there is not enough.
    pub fn take(&mut self, ingredient: &str, amount: f64) -> bool {
        match self.items.get_mut(ingredient) {
            Some(left) if *left >= amount => {
                *left -= amount;
                true
            }
            _ => false,
        }
    }

    /// The amount of `ingredient` in stock.
    pub fn amount(&self, ingredient: &str) -> f64 {
        self.items.get(ingredient).copied().unwrap_or(0.0)
    }
}
