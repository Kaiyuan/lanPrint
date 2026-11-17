//! Language metadata and a small runtime translation table.
//!
//! The desktop UI uses a flat key/value table so switching language updates the
//! UI immediately without restarting. The same keys are reused by the web site
//! and by the bridge server where useful.

use serde::{Deserialize, Serialize};

use crate::model::Language;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageInfo {
    pub code: String,
    pub native_name: String,
    pub english_name: String,
}

pub fn language_list() -> Vec<LanguageInfo> {
    Language::ALL
        .iter()
        .map(|l| LanguageInfo {
            code: l.code().to_string(),
            native_name: l.native_name().to_string(),
            english_name: english_name(*l).to_string(),
        })
        .collect()
}

pub fn english_name(lang: Language) -> &'static str {
    match lang {
        Language::ZhCn => "Simplified Chinese",
        Language::ZhTw => "Traditional Chinese",
        Language::En => "English",
        Language::Fr => "French",
        Language::De => "German",
        Language::It => "Italian",
        Language::Es => "Spanish",
        Language::Ja => "Japanese",
        Language::Ko => "Korean",
    }
}

/// A translation table. Keys missing from a locale fall back to English.
pub struct Translations {
    lang: Language,
    map: serde_json::Map<String, serde_json::Value>,
    fallback: serde_json::Map<String, serde_json::Value>,
}

macro_rules! locale {
    ($lang:literal) => {
        include_str!(concat!("../../../locales/", $lang, ".json"))
    };
}

impl Translations {
    pub fn load(lang: Language) -> Self {
        let fallback: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(locale!("en-US")).unwrap_or_default();
        let text = match lang {
            Language::ZhCn => locale!("zh-CN"),
            Language::ZhTw => locale!("zh-TW"),
            Language::En => locale!("en-US"),
            Language::Fr => locale!("fr-FR"),
            Language::De => locale!("de-DE"),
            Language::It => locale!("it-IT"),
            Language::Es => locale!("es-ES"),
            Language::Ja => locale!("ja-JP"),
            Language::Ko => locale!("ko-KR"),
        };
        let map = serde_json::from_str(text).unwrap_or_default();
        Self {
            lang,
            map,
            fallback,
        }
    }

    pub fn language(&self) -> Language {
        self.lang
    }

    pub fn get(&self, key: &str) -> String {
        self.map
            .get(key)
            .or_else(|| self.fallback.get(key))
            .and_then(|v| v.as_str())
            .unwrap_or(key)
            .to_string()
    }

    pub fn fmt(&self, key: &str, args: &[(&str, &str)]) -> String {
        let mut text = self.get(key);
        for (name, value) in args {
            text = text.replace(&format!("{{{name}}}"), value);
        }
        text
    }
}
