//! Several stock reports side by side, one column per pantry.

use crate::pantry::stock::Stock;

/// A table with one row per ingredient and one column per pantry.
pub fn table(pantries: &[(&str, &Stock)], names: &[&str]) -> String {
    let mut out = String::from("ingredient");
    for (pantry, _) in pantries {
        out.push_str(&format!("\t{pantry}"));
    }
    out.push('\n');
    for name in names {
        out.push_str(name);
        for (_, stock) in pantries {
            out.push_str(&format!("\t{:.1}", stock.amount(name)));
        }
        out.push('\n');
    }
    out
}
