//! Converting a recipe's amounts to metric units.

use crate::recipe::{Amount, Recipe};
use crate::units::{Unit, to_grams, to_millilitres};

/// `amount` in grams or millilitres; unchanged when it has no metric form.
pub fn to_metric(amount: &Amount) -> Amount {
    if let Some(grams) = to_grams(amount.value, amount.unit) {
        return Amount { value: grams, unit: Unit::Gram, ..amount.clone() };
    }
    if let Some(millilitres) = to_millilitres(amount.value, amount.unit) {
        return Amount { value: millilitres, unit: Unit::Millilitre, ..amount.clone() };
    }
    amount.clone()
}

/// `recipe` with every amount converted by [`to_metric`].
pub fn metric(recipe: &Recipe) -> Recipe {
    Recipe { amounts: recipe.amounts.iter().map(to_metric).collect(), ..recipe.clone() }
}

#[cfg(test)]
mod tests {
    use super::to_metric;
    use crate::recipe::Amount;
    use crate::units::Unit;

    #[test]
    fn cups_become_millilitres() {
        let cup = Amount { ingredient: "milk".into(), value: 2.0, unit: Unit::Cup };
        assert_eq!(to_metric(&cup).value, 500.0);
        assert_eq!(to_metric(&cup).unit, Unit::Millilitre);
    }

    #[test]
    fn pieces_stay_pieces() {
        let eggs = Amount { ingredient: "eggs".into(), value: 3.0, unit: Unit::Piece };
        assert_eq!(to_metric(&eggs), eggs);
    }
}
