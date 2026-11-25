//! Recipes: a name, the servings it feeds, and its amounts.

pub mod convert;
mod parser;
pub mod scale;

pub use parser::parse_line;

use crate::units::Unit;

/// One measured ingredient of a recipe.
#[derive(Clone, Debug, PartialEq)]
pub struct Amount {
    pub ingredient: String,
    pub value: f64,
    pub unit: Unit,
}

/// A recipe as the planner keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct Recipe {
    pub name: String,
    pub servings: u32,
    pub amounts: Vec<Amount>,
}
