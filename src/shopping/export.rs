//! Exporting a shopping list as Markdown or CSV.

use std::collections::BTreeMap;

/// A Markdown checklist, one item per line.
pub fn markdown(list: &BTreeMap<String, f64>) -> String {
    list.iter().map(|(name, value)| format!("- [ ] {name} ({value:.0})\n")).collect()
}

/// `ingredient,amount` lines with a header.
pub fn csv(list: &BTreeMap<String, f64>) -> String {
    let mut out = String::from("ingredient,amount\n");
    for (name, value) in list {
        out.push_str(&format!("{name},{value:.1}\n"));
    }
    out
}
