//! Reading one ingredient line such as `200 g flour`.

use crate::recipe::Amount;
use crate::units::Unit;

/// Parses `<value> <unit> <ingredient>`; `None` when the line has another shape.
pub fn parse_line(line: &str) -> Option<Amount> {
    let mut words = line.split_whitespace();
    let value: f64 = words.next()?.parse().ok()?;
    let unit = match words.next()? {
        "g" => Unit::Gram,
        "kg" => Unit::Kilogram,
        "ml" => Unit::Millilitre,
        "l" => Unit::Litre,
        "cup" | "cups" => Unit::Cup,
        "tbsp" => Unit::Tablespoon,
        "tsp" => Unit::Teaspoon,
        "pinch" | "pinches" => Unit::Pinch,
        "oz" => Unit::Ounce,
        _ => return None,
    };
    let ingredient = words.collect::<Vec<_>>().join(" ");
    if ingredient.is_empty() {
        return None;
    }
    Some(Amount { ingredient, value, unit })
}
