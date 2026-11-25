//! The accessible names the diff component announces, in Loupe's words.
//!
//! gpui-component names a diff's sides "Original" and "Modified" and its
//! line button "Add annotation" in its own translations, so a screen reader
//! heard "Modified line 12, added" on a file that was only added. Loupe
//! extends those translations through rust-i18n, which the component looks up
//! before its own: every line and button it draws then reads as the product
//! says it, in every locale, while the keys left out keep the component's text.

use std::borrow::Cow;
use std::sync::Once;

use rust_i18n::Backend;

/// The crate whose translations this extends; rust-i18n prefixes its keys with it.
const NAMESPACE: &str = "gpui_component";

const LABELS: &[(&str, &str)] = &[
    ("Diff.Original", "old"),
    ("Diff.Modified", "new"),
    ("Diff.SourceLine", "Line %{line} (%{side})"),
    ("Diff.AddedLine", "Line %{line}, added"),
    ("Diff.RemovedLine", "Line %{line}, removed"),
    ("Diff.SelectLine", "Select line %{line} (%{side})"),
    ("Diff.AddAnnotation", "Comment on line %{line}"),
];

struct DiffLabels;

impl Backend for DiffLabels {
    fn available_locales(&self) -> Vec<Cow<'_, str>> {
        vec!["en".into()]
    }

    fn translate(&self, _locale: &str, key: &str) -> Option<Cow<'_, str>> {
        let key = key.strip_prefix(NAMESPACE)?.strip_prefix('.')?;
        LABELS
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, label)| Cow::Borrowed(*label))
    }

    fn messages_for_locale(&self, _locale: &str) -> Option<Vec<(Cow<'_, str>, Cow<'_, str>)>> {
        Some(
            LABELS
                .iter()
                .map(|(key, label)| (format!("{NAMESPACE}.{key}").into(), Cow::Borrowed(*label)))
                .collect(),
        )
    }
}

static DIFF_LABELS: DiffLabels = DiffLabels;

/// Makes the diff component announce Loupe's labels. Call it at startup;
/// later calls do nothing.
pub fn install_diff_labels() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| gpui_kit::component::_rust_i18n_extend(&DIFF_LABELS, NAMESPACE));
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::_rust_i18n_translate as translate;

    use super::install_diff_labels;

    #[test]
    fn the_component_reads_loupes_labels_first() {
        install_diff_labels();
        install_diff_labels();
        assert_eq!(
            translate("en", "Diff.AddAnnotation"),
            "Comment on line %{line}"
        );
        assert_eq!(translate("en", "Diff.AddedLine"), "Line %{line}, added");
        assert_eq!(translate("en", "Diff.Modified"), "new");
        assert_eq!(translate("pt-BR", "Diff.Original"), "old");
    }

    #[test]
    fn keys_loupe_leaves_out_keep_the_components_text() {
        install_diff_labels();
        assert_eq!(
            translate("en", "Diff.NoNewline"),
            "No newline at end of file"
        );
    }
}
