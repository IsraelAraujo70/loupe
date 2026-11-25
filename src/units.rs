//! Kitchen units and conversions between them.

/// A unit an amount is measured in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Gram,
    Kilogram,
    Millilitre,
    Litre,
    Cup,
    Tablespoon,
    Teaspoon,
    Ounce,
    Piece,
}

/// Grams in one ounce.
pub const GRAMS_PER_OUNCE: f64 = 28.35;
/// Millilitres in one US cup.
pub const MILLILITRES_PER_CUP: f64 = 240.0;

impl Unit {
    /// The short label shown next to an amount.
    pub fn label(self) -> &'static str {
        match self {
            Unit::Gram => "g",
            Unit::Kilogram => "kg",
            Unit::Millilitre => "ml",
            Unit::Litre => "l",
            Unit::Cup => "cup",
            Unit::Tablespoon => "tbsp",
            Unit::Teaspoon => "tsp",
            Unit::Ounce => "oz",
            Unit::Piece => "pc",
        }
    }

    /// Whether amounts in this unit are weights.
    pub fn is_weight(self) -> bool {
        matches!(self, Unit::Gram | Unit::Kilogram | Unit::Ounce)
    }
}

/// Converts `amount` from `from` to grams, when `from` is a weight.
pub fn to_grams(amount: f64, from: Unit) -> Option<f64> {
    match from {
        Unit::Gram => Some(amount),
        Unit::Kilogram => Some(amount * 1000.0),
        Unit::Ounce => Some(amount * GRAMS_PER_OUNCE),
        _ => None,
    }
}
