//! A plain-text report of the stock.

use crate::pantry::stock::Stock;

/// One line per ingredient: name and amount.
pub fn report(stock: &Stock, names: &[&str]) -> String {
    let mut out = String::new();
    for name in names {
        out.push_str(&format!("{name}: {}\n", stock.amount(name)));
    }
    out
}
