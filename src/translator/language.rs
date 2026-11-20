use crate::translator::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// Explicit source/target language. Automatic detection is intentionally not
/// mixed with translation: mixed-language articles need a caller-owned policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Language {
    English,
    ChineseSimplified,
    ChineseTraditional,
    Japanese,
    Korean,
    French,
    German,
    Spanish,
    Portuguese,
    Italian,
    Russian,
    Arabic,
    Indonesian,
    Thai,
    Vietnamese,
    Turkish,
    Hindi,
    Malay,
    Dutch,
    Swedish,
    Danish,
    Finnish,
    Polish,
    Czech,
    Filipino,
    Persian,
    Greek,
    Hungarian,
    Romanian,
    Cantonese,
    Macedonian,
}

impl Language {
    pub fn code(self) -> &'static str {
        use Language::*;
        match self {
            English => "en",
            ChineseSimplified => "zh-Hans",
            ChineseTraditional => "zh-Hant",
            Japanese => "ja",
            Korean => "ko",
            French => "fr",
            German => "de",
            Spanish => "es",
            Portuguese => "pt",
            Italian => "it",
            Russian => "ru",
            Arabic => "ar",
            Indonesian => "id",
            Thai => "th",
            Vietnamese => "vi",
            Turkish => "tr",
            Hindi => "hi",
            Malay => "ms",
            Dutch => "nl",
            Swedish => "sv",
            Danish => "da",
            Finnish => "fi",
            Polish => "pl",
            Czech => "cs",
            Filipino => "fil",
            Persian => "fa",
            Greek => "el",
            Hungarian => "hu",
            Romanian => "ro",
            Cantonese => "yue",
            Macedonian => "mk",
        }
    }

    pub fn name(self) -> &'static str {
        use Language::*;
        match self {
            English => "English",
            ChineseSimplified => "Chinese",
            ChineseTraditional => "Chinese",
            Japanese => "Japanese",
            Korean => "Korean",
            French => "French",
            German => "German",
            Spanish => "Spanish",
            Portuguese => "Portuguese",
            Italian => "Italian",
            Russian => "Russian",
            Arabic => "Arabic",
            Indonesian => "Indonesian",
            Thai => "Thai",
            Vietnamese => "Vietnamese",
            Turkish => "Turkish",
            Hindi => "Hindi",
            Malay => "Malay",
            Dutch => "Dutch",
            Swedish => "Swedish",
            Danish => "Danish",
            Finnish => "Finnish",
            Polish => "Polish",
            Czech => "Czech",
            Filipino => "Tagalog",
            Persian => "Persian",
            Greek => "Modern Greek",
            Hungarian => "Hungarian",
            Romanian => "Romanian",
            Cantonese => "Yue Chinese",
            Macedonian => "Macedonian",
        }
    }

    pub fn is_chinese(self) -> bool {
        matches!(self, Self::ChineseSimplified | Self::ChineseTraditional)
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::ChineseSimplified => "Simplified Chinese",
            Self::ChineseTraditional => "Traditional Chinese",
            _ => self.name(),
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl FromStr for Language {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        use Language::*;
        let normalized = value.trim().replace('_', "-").to_ascii_lowercase();
        let base = normalized.split('-').next().unwrap_or("");
        let language = match base {
            "zh" if normalized.contains("hant")
                || normalized.ends_with("-tw")
                || normalized.ends_with("-hk")
                || normalized.ends_with("-mo") =>
            {
                ChineseTraditional
            }
            "zh" => ChineseSimplified,
            "en" => English,
            "ja" => Japanese,
            "ko" => Korean,
            "fr" => French,
            "de" => German,
            "es" => Spanish,
            "pt" => Portuguese,
            "it" => Italian,
            "ru" => Russian,
            "ar" => Arabic,
            "id" => Indonesian,
            "th" => Thai,
            "vi" => Vietnamese,
            "tr" => Turkish,
            "hi" => Hindi,
            "ms" => Malay,
            "nl" => Dutch,
            "sv" => Swedish,
            "da" => Danish,
            "fi" => Finnish,
            "pl" => Polish,
            "cs" => Czech,
            "fil" | "tl" => Filipino,
            "fa" => Persian,
            "el" => Greek,
            "hu" => Hungarian,
            "ro" => Romanian,
            "yue" => Cantonese,
            "mk" => Macedonian,
            _ => {
                return Err(Error::InvalidConfig(format!(
                    "unknown language tag: {value}"
                )));
            }
        };
        Ok(language)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_script_region_and_case() {
        assert_eq!(
            "ZH_tw".parse::<Language>().unwrap(),
            Language::ChineseTraditional
        );
        assert_eq!("en-US".parse::<Language>().unwrap(), Language::English);
        assert!("auto".parse::<Language>().is_err());
    }
}
