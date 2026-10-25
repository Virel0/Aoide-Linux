//! Lyrics in the listener's own language, under the original lines.
//!
//! Two halves:
//!
//! - Always built: which language lyrics are in ([`detect_language`], by
//!   whatlang's trigram detector, nothing sent anywhere), the listener's
//!   language ([`system_language`], from the locale), and whether a
//!   translation is worth offering ([`needs_translation`]): a button that
//!   translates English into English is noise.
//! - With the `local-translation` feature: [`local::LocalTranslator`]
//!   translates the lines **on this machine** with Firefox Translations'
//!   models through the `fxtranslate` engine (pure Rust). A language pair's
//!   model (about 20 to 40 MB) is downloaded the first time it is needed,
//!   into the cache directory the app names, and verified by its hash; the
//!   lyrics themselves never leave the machine. Nothing is bundled.
//!
//! Languages are two-letter ISO 639-1 codes ("en", "fr", "ja"), as Firefox's
//! models name them.

#[cfg(feature = "local-translation")]
pub mod local;

use std::ffi::OsString;

/// A language the lyrics were found to be in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Detected {
    /// ISO 639-1.
    pub language: &'static str,
    /// whatlang's confidence, 0 to 1.
    pub confidence: f64,
}

/// whatlang's 69 languages as ISO 639-1 codes.
fn iso_639_1(lang: whatlang::Lang) -> &'static str {
    use whatlang::Lang::*;
    match lang {
        Epo => "eo",
        Eng => "en",
        Rus => "ru",
        Cmn => "zh",
        Spa => "es",
        Por => "pt",
        Ita => "it",
        Ben => "bn",
        Fra => "fr",
        Deu => "de",
        Ukr => "uk",
        Kat => "ka",
        Ara => "ar",
        Hin => "hi",
        Jpn => "ja",
        Heb => "he",
        Yid => "yi",
        Pol => "pl",
        Amh => "am",
        Jav => "jv",
        Kor => "ko",
        Nob => "nb",
        Dan => "da",
        Swe => "sv",
        Fin => "fi",
        Tur => "tr",
        Nld => "nl",
        Hun => "hu",
        Ces => "cs",
        Ell => "el",
        Bul => "bg",
        Bel => "be",
        Mar => "mr",
        Kan => "kn",
        Ron => "ro",
        Slv => "sl",
        Hrv => "hr",
        Srp => "sr",
        Mkd => "mk",
        Lit => "lt",
        Lav => "lv",
        Est => "et",
        Tam => "ta",
        Vie => "vi",
        Urd => "ur",
        Tha => "th",
        Guj => "gu",
        Uzb => "uz",
        Pan => "pa",
        Aze => "az",
        Ind => "id",
        Tel => "te",
        Pes => "fa",
        Mal => "ml",
        Ori => "or",
        Mya => "my",
        Nep => "ne",
        Sin => "si",
        Khm => "km",
        Tuk => "tk",
        Aka => "ak",
        Zul => "zu",
        Sna => "sn",
        Afr => "af",
        Lat => "la",
        Slk => "sk",
        Cat => "ca",
        Tgl => "tl",
        Hye => "hy",
        Cym => "cy",
    }
}

/// The language of `text`, if whatlang is sure of it. Short or mixed text
/// is often not; `None` then, rather than a guess.
pub fn detect_language(text: &str) -> Option<Detected> {
    let info = whatlang::detect(text)?;
    info.is_reliable().then(|| Detected {
        language: iso_639_1(info.lang()),
        confidence: info.confidence(),
    })
}

/// The language of a set of lyric lines, read as one text (a single line is
/// too short to tell).
pub fn lyrics_language(lines: &[String]) -> Option<Detected> {
    let text = lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    detect_language(&text)
}

/// One spelling per language, so "no" and "nb" compare equal.
fn normalise(code: &str) -> String {
    match code {
        "no" | "nn" => "nb".into(),
        "iw" => "he".into(),
        "in" => "id".into(),
        other => other.to_owned(),
    }
}

/// The listener's language from the locale, as glibc picks it: the first of
/// `LANGUAGE`, then `LC_ALL`, `LC_MESSAGES`, `LANG`. "pt_BR.UTF-8" is "pt";
/// "C" and "POSIX" name no language. `env` reads an environment variable.
pub fn system_language(env: impl Fn(&str) -> Option<OsString>) -> Option<String> {
    let read = |name: &str| env(name).and_then(|v| v.into_string().ok());
    let candidates = read("LANGUAGE")
        .into_iter()
        .flat_map(|list| list.split(':').map(str::to_owned).collect::<Vec<_>>())
        .chain(
            ["LC_ALL", "LC_MESSAGES", "LANG"]
                .into_iter()
                .filter_map(read),
        );
    for locale in candidates {
        let code: String = locale
            .split(['_', '.', '@', '-'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if code.len() >= 2
            && code.len() <= 3
            && code.chars().all(|c| c.is_ascii_lowercase())
            && code != "c"
            && code != "posix"
        {
            return Some(normalise(&code));
        }
    }
    None
}

/// The language to translate `lines` from, when they are reliably in a
/// language other than `target`. `None` when they are in `target` already,
/// or the language cannot be told.
pub fn needs_translation(lines: &[String], target: &str) -> Option<&'static str> {
    let source = lyrics_language(lines)?.language;
    (normalise(source) != normalise(target)).then_some(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(String::from).collect()
    }

    const FRENCH: &str = "Sous le ciel de Paris\n\
        s'envole une chanson\n\
        elle est née d'aujourd'hui\n\
        dans le cœur d'un garçon";

    const ENGLISH: &str = "I walked along the river\n\
        when the morning light was new\n\
        and every song I sang that day\n\
        was only ever meant for you";

    #[test]
    fn tells_french_lyrics_from_english() {
        assert_eq!(lyrics_language(&lines(FRENCH)).unwrap().language, "fr");
        assert_eq!(lyrics_language(&lines(ENGLISH)).unwrap().language, "en");
    }

    #[test]
    fn tells_scripts_apart() {
        let japanese =
            lines("夜空に浮かぶ星を数えて\n君の名前を呼んでいた\n明日になればまた会える");
        assert_eq!(lyrics_language(&japanese).unwrap().language, "ja");
        let russian =
            lines("Я помню чудное мгновенье\nПередо мной явилась ты\nКак мимолётное виденье");
        assert_eq!(lyrics_language(&russian).unwrap().language, "ru");
    }

    #[test]
    fn says_nothing_rather_than_guess_at_too_little() {
        assert_eq!(detect_language(""), None);
        assert_eq!(detect_language("ok"), None);
        assert_eq!(lyrics_language(&[]), None);
    }

    #[test]
    fn offers_a_translation_only_into_another_language() {
        assert_eq!(needs_translation(&lines(FRENCH), "en"), Some("fr"));
        assert_eq!(needs_translation(&lines(FRENCH), "fr"), None);
        assert_eq!(needs_translation(&lines(ENGLISH), "en"), None);
        assert_eq!(needs_translation(&["la".into()], "en"), None);
    }

    #[test]
    fn reads_the_language_from_the_locale() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| OsString::from(v))
            }
        };
        assert_eq!(
            system_language(env(&[("LANG", "pt_BR.UTF-8")])).as_deref(),
            Some("pt")
        );
        assert_eq!(
            system_language(env(&[("LANG", "en_GB.UTF-8"), ("LC_ALL", "de_DE.UTF-8")])).as_deref(),
            Some("de")
        );
        assert_eq!(
            system_language(env(&[("LANGUAGE", "fr_CA:en"), ("LANG", "en_US.UTF-8")])).as_deref(),
            Some("fr")
        );
        assert_eq!(
            system_language(env(&[("LANGUAGE", ""), ("LANG", "sr_RS@latin")])).as_deref(),
            Some("sr")
        );
        assert_eq!(system_language(env(&[("LANG", "C.UTF-8")])), None);
        assert_eq!(system_language(env(&[("LANG", "POSIX")])), None);
        assert_eq!(system_language(env(&[])), None);
        assert_eq!(
            system_language(env(&[("LANG", "nn_NO.UTF-8")])).as_deref(),
            Some("nb")
        );
    }
}
