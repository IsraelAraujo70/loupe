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
    Pinch,
    Ounce,
    Piece,
}

/// Grams in one ounce.
pub const GRAMS_PER_OUNCE: f64 = 28.349_523;
/// Millilitres in one metric cup.
pub const MILLILITRES_PER_CUP: f64 = 250.0;

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
            Unit::Pinch => "pinch",
            Unit::Ounce => "oz",
            Unit::Piece => "pcs",
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
        Unit::Kilogram => Some(amount * 1_000.0),
        Unit::Ounce => Some(amount * GRAMS_PER_OUNCE),
        _ => None,
    }
}

/// Converts `amount` from `from` to millilitres, when `from` is a volume.
pub fn to_millilitres(amount: f64, from: Unit) -> Option<f64> {
    match from {
        Unit::Millilitre => Some(amount),
        Unit::Litre => Some(amount * 1_000.0),
        Unit::Cup => Some(amount * MILLILITRES_PER_CUP),
        Unit::Tablespoon => Some(amount * 15.0),
        Unit::Teaspoon => Some(amount * 5.0),
        _ => None,
    }
}
