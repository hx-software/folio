//! Embedded Fluent localization, following cosmic-utils/tasks.
use std::sync::LazyLock;

use i18n_embed::{
    DefaultLocalizer, LanguageLoader, Localizer,
    fluent::{FluentLanguageLoader, fluent_language_loader},
    unic_langid::LanguageIdentifier,
};
use rust_embed::RustEmbed;

pub fn init(requested_languages: &[LanguageIdentifier]) {
    if let Err(error) = localizer().select(requested_languages) {
        crate::browser::log_warn(&format!("Error loading Fluent localizations: {error}"));
    }
}

pub fn localizer() -> Box<dyn Localizer> {
    Box::new(DefaultLocalizer::new(&*LANGUAGE_LOADER, &Localizations))
}

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

pub static LANGUAGE_LOADER: LazyLock<FluentLanguageLoader> = LazyLock::new(|| {
    let loader: FluentLanguageLoader = fluent_language_loader!();
    loader
        .load_fallback_language(&Localizations)
        .expect("Could not load the fallback Fluent catalog");
    loader
});

/// Request a localized message, with compile-time ID and argument validation.
#[macro_export]
macro_rules! fl {
    ($message_id:literal) => {{
        i18n_embed_fl::fl!($crate::i18n::LANGUAGE_LOADER, $message_id)
    }};
    ($message_id:literal, $($args:tt)*) => {{
        i18n_embed_fl::fl!($crate::i18n::LANGUAGE_LOADER, $message_id, $($args)*)
    }};
}

pub fn message(text: &str) -> String {
    match text {
        "Unsupported project version." => fl!("unsupported-project-version"),
        "Project name is too long." => fl!("project-name-is-too-long"),
        "Page dimensions must be between 10 and 2000 mm." => {
            fl!("page-dimensions-must-be-between-10-and")
        }
        "Quantity must be 1–100,000." => fl!("quantity-must-be-1-100-000"),
        "Increment must be positive." => fl!("increment-must-be-positive"),
        "Sequence exceeds the integer limit." => fl!("sequence-exceeds-the-integer-limit"),
        "Padding cannot exceed 20 digits." => fl!("padding-cannot-exceed-20-digits"),
        "Prefixes and suffixes cannot exceed 100 bytes each." => {
            fl!("prefixes-and-suffixes-cannot-exceed-100-bytes")
        }
        "Prefixes and suffixes cannot contain control characters." => {
            fl!("prefixes-and-suffixes-cannot-contain-control-characters")
        }
        "Prefix and suffix spacing must be between 0 and 50 mm." => {
            fl!("prefix-and-suffix-spacing-must-be-between")
        }
        "Grid dimensions must be between 1 and 20." => fl!("grid-dimensions-must-be-between-1-and"),
        "Margins and gaps must be finite and non-negative." => {
            fl!("margins-and-gaps-must-be-finite-and")
        }
        "Grid cells must be at least 5 mm across." => fl!("grid-cells-must-be-at-least-5"),
        "A project must have between 1 and 256 number fields." => {
            fl!("a-project-must-have-between-1-and")
        }
        "Reduce the grid or field count: at most 2,000 number positions per sheet." => {
            fl!("reduce-the-grid-or-field-count-at")
        }
        "Number field IDs must be unique." => fl!("number-field-ids-must-be-unique"),
        "Number field anchors must be inside their placement area (cell or page)." => {
            fl!("number-field-anchors-must-be-inside-their")
        }
        "Font size must be between 1 and 300 points." => fl!("font-size-must-be-between-1-and"),
        "Invalid field colour; use #RRGGBB." => fl!("invalid-field-colour-use-rrggbb"),
        "A field's start number is too large for this quantity and increment." => {
            fl!("a-field-s-start-number-is-too")
        }
        "Artwork scale must be between 10% and 500%." => {
            fl!("artwork-scale-must-be-between-10-and")
        }
        "Artwork position must be within 2000 mm of the centre." => {
            fl!("artwork-position-must-be-within-2000-mm")
        }
        "Artwork must be non-empty and no larger than 20 MB." => {
            fl!("artwork-must-be-non-empty-and-no")
        }
        "Artwork filename is too long." => fl!("artwork-filename-is-too-long"),
        "Font must be non-empty and no larger than 10 MB." => fl!("font-must-be-non-empty-and-no"),
        "Font filename is too long." => fl!("font-filename-is-too-long"),
        "Invalid font identifier." => fl!("invalid-font-identifier"),
        "Project exceeds 100 MB." => fl!("project-exceeds-100-mb"),
        "Sheet is out of range." => fl!("sheet-is-out-of-range"),
        "A number exceeds the supported range." => fl!("a-number-exceeds-the-supported-range"),
        "The browser could not read this font file." => fl!("the-browser-could-not-read-this-font"),
        "The browser could not load this font file." => fl!("the-browser-could-not-load-this-font"),
        "The browser rejected this font. It may be damaged or unsupported." => {
            fl!("the-browser-rejected-this-font-it-may")
        }
        "The browser could not register this font." => {
            fl!("the-browser-could-not-register-this-font")
        }
        "The file picker was interrupted." => fl!("the-file-picker-was-interrupted"),
        "The PDF worker stopped unexpectedly. Cancel or retry to restart it; try smaller artwork if this repeats." =>
        {
            fl!("the-pdf-worker-stopped-unexpectedly-cancel-or")
        }
        "The PDF worker is unavailable." => fl!("the-pdf-worker-is-unavailable"),
        "IndexedDB request failed." => fl!("indexeddb-request-failed"),
        "Storage request was interrupted." => fl!("storage-request-was-interrupted"),
        "IndexedDB is unavailable; use downloadable project backups." => {
            fl!("indexeddb-is-unavailable-use-downloadable-project-backups")
        }
        "Storage write failed (storage may be full). Export a project backup." => {
            fl!("storage-write-failed-storage-may-be-full")
        }
        "Storage delete failed." => fl!("storage-delete-failed"),
        _ => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashMap};

    fn loader(languages: &[&str]) -> FluentLanguageLoader {
        let loader = FluentLanguageLoader::new("folio", "en".parse().unwrap());
        let requested = languages
            .iter()
            .map(|language| language.parse().unwrap())
            .collect::<Vec<_>>();
        DefaultLocalizer::new(&loader, &Localizations)
            .select(&requested)
            .unwrap();
        loader
    }

    #[test]
    fn language_negotiation_selects_spanish_and_falls_back_to_english() {
        assert_eq!(
            loader(&["es-MX", "en-US"]).get("new-project"),
            "Nuevo proyecto"
        );
        assert_eq!(
            loader(&["en-US", "es-MX"]).get("new-project"),
            "New project"
        );
        assert_eq!(loader(&["fr"]).get("new-project"), "New project");
    }

    #[test]
    fn embedded_catalogs_have_matching_message_ids() {
        let loader = loader(&["es-MX"]);
        let ids = |language: &str| {
            loader.with_message_iter(&language.parse().unwrap(), |messages| {
                messages
                    .map(|message| message.id.name.to_string())
                    .collect::<BTreeSet<_>>()
            })
        };
        let english = ids("en");
        assert!(!english.is_empty());
        assert_eq!(english, ids("es-MX"));
    }

    #[test]
    fn fluent_selects_plurals_and_preserves_project_names() {
        let loader = loader(&["es-MX"]);
        let singular = loader.get_args("selected", HashMap::from([("selection_count", 1)]));
        let plural = loader.get_args("selected", HashMap::from([("selection_count", 2)]));
        assert!(singular.ends_with(" seleccionado"));
        assert!(plural.ends_with(" seleccionados"));
        let name = "Boletos { $name }";
        let export = loader.get_args("export-to-a-file", HashMap::from([("name", name)]));
        assert!(export.starts_with("Exportar "));
        assert!(export.contains(name));
        assert!(export.ends_with(" a un archivo"));
    }
}
