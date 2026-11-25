//! Imports the old comma-separated stock export. Kept until every pantry
//! moved to the new format.

use crate::pantry::stock::Stock;

/// Reads `name,amount` lines into `stock`; returns the lines it skipped.
pub fn import(stock: &mut Stock, text: &str) -> usize {
    let mut skipped = 0;
    for line in text.lines() {
        let Some((name, amount)) = line.split_once(',') else {
            skipped += 1;
            continue;
        };
        match amount.trim().parse::<f64>() {
            Ok(amount) => stock.add(name.trim(), amount),
            Err(_) => skipped += 1,
        }
    }
    skipped
}
