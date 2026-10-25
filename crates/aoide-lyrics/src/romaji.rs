//! Kana to romaji, modified Hepburn. No dictionary: kana in, letters out.
//!
//! What a dictionary-free conversion can't know it leaves alone: kanji pass
//! through unchanged, the particle は stays "ha", and an おう that is a long vowel
//! stays "ou". [`JapaneseReader`](crate::JapaneseReader) (the `furigana`
//! feature) reads kanji and particles first, then converts with this.

/// Whether `text` looks Japanese: it has kana. Chinese has kanji but no kana, so
/// kanji alone don't count.
pub fn is_japanese(text: &str) -> bool {
    text.chars().any(|c| is_hiragana(c) || is_katakana(c))
}

/// `text` with its hiragana and katakana written in modified Hepburn romaji:
/// し "shi", ち "chi", つ "tsu", ふ "fu", じ and ぢ "ji", ず and づ "zu", を "o",
/// ん "n" (and "n'" before a vowel or y), っ doubling the next consonant ("tch"
/// before ch), ー a macron on the vowel before it. Japanese punctuation becomes
/// its ASCII counterpart; everything else passes through.
pub fn kana_to_romaji(text: &str) -> String {
    let chars: Vec<char> = text.chars().map(to_hiragana).collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            'っ' => {
                // Doubles the consonant of what follows; nothing at the end.
                let (next, _) = syllable(&chars[i + 1..]);
                if let Some(next) = next {
                    if next.starts_with("ch") {
                        out.push('t');
                    } else if let Some(first) = next.chars().next().filter(|c| !is_vowel(*c)) {
                        out.push(first);
                    }
                }
                i += 1;
            }
            'ん' => {
                out.push('n');
                let (next, _) = syllable(&chars[i + 1..]);
                if next.is_some_and(|n| n.starts_with(|c: char| is_vowel(c) || c == 'y')) {
                    out.push('\'');
                }
                i += 1;
            }
            'ー' => {
                lengthen(&mut out);
                i += 1;
            }
            _ => match syllable(&chars[i..]) {
                (Some(romaji), used) => {
                    out.push_str(&romaji);
                    i += used;
                }
                (None, _) => {
                    match punctuation(c) {
                        Some(ascii) => out.push_str(ascii),
                        None => out.push(c),
                    }
                    i += 1;
                }
            },
        }
    }
    out
}

/// The romaji of the syllable at the start of `chars` (one kana, or a kana and
/// the small kana after it), and how many chars it took. `None` when it doesn't
/// start with kana.
fn syllable(chars: &[char]) -> (Option<String>, usize) {
    let Some(&first) = chars.first() else {
        return (None, 0);
    };
    let Some(base) = single(first) else {
        return (None, 0);
    };
    let Some(&second) = chars.get(1) else {
        return (Some(base.to_owned()), 1);
    };
    // き + ゃ: kya; し + ゃ: sha; ち + ゃ: cha; じ + ゃ: ja.
    if let Some(glide) = match second {
        'ゃ' => Some('a'),
        'ゅ' => Some('u'),
        'ょ' => Some('o'),
        _ => None,
    } && base.ends_with('i')
        && base.len() >= 2
    {
        let stem = &base[..base.len() - 1];
        let joined = match stem {
            "sh" | "ch" | "j" => format!("{stem}{glide}"),
            _ => format!("{stem}y{glide}"),
        };
        return (Some(joined), 2);
    }
    // Small vowels after a kana, mostly in loanwords: ファ fa, ティ ti, ウィ wi.
    if let Some(vowel) = small_vowel(second) {
        let joined = match (first, vowel) {
            ('ふ', v) => Some(format!("f{v}")),
            ('ゔ', v) => Some(format!("v{v}")),
            ('て', 'i') => Some("ti".to_owned()),
            ('で', 'i') => Some("di".to_owned()),
            ('と', 'u') => Some("tu".to_owned()),
            ('ど', 'u') => Some("du".to_owned()),
            ('う', v @ ('i' | 'e' | 'o')) => Some(format!("w{v}")),
            ('い', 'e') => Some("ye".to_owned()),
            ('し', 'e') => Some("she".to_owned()),
            ('ち', 'e') => Some("che".to_owned()),
            ('じ', 'e') => Some("je".to_owned()),
            ('つ', v) => Some(format!("ts{v}")),
            _ => None,
        };
        if let Some(joined) = joined {
            return (Some(joined), 2);
        }
    }
    (Some(base.to_owned()), 1)
}

/// One hiragana on its own.
#[rustfmt::skip]
fn single(c: char) -> Option<&'static str> {
    Some(match c {
        'あ' => "a", 'い' => "i", 'う' => "u", 'え' => "e", 'お' => "o",
        'か' => "ka", 'き' => "ki", 'く' => "ku", 'け' => "ke", 'こ' => "ko",
        'が' => "ga", 'ぎ' => "gi", 'ぐ' => "gu", 'げ' => "ge", 'ご' => "go",
        'さ' => "sa", 'し' => "shi", 'す' => "su", 'せ' => "se", 'そ' => "so",
        'ざ' => "za", 'じ' => "ji", 'ず' => "zu", 'ぜ' => "ze", 'ぞ' => "zo",
        'た' => "ta", 'ち' => "chi", 'つ' => "tsu", 'て' => "te", 'と' => "to",
        'だ' => "da", 'ぢ' => "ji", 'づ' => "zu", 'で' => "de", 'ど' => "do",
        'な' => "na", 'に' => "ni", 'ぬ' => "nu", 'ね' => "ne", 'の' => "no",
        'は' => "ha", 'ひ' => "hi", 'ふ' => "fu", 'へ' => "he", 'ほ' => "ho",
        'ば' => "ba", 'び' => "bi", 'ぶ' => "bu", 'べ' => "be", 'ぼ' => "bo",
        'ぱ' => "pa", 'ぴ' => "pi", 'ぷ' => "pu", 'ぺ' => "pe", 'ぽ' => "po",
        'ま' => "ma", 'み' => "mi", 'む' => "mu", 'め' => "me", 'も' => "mo",
        'や' => "ya", 'ゆ' => "yu", 'よ' => "yo",
        'ら' => "ra", 'り' => "ri", 'る' => "ru", 'れ' => "re", 'ろ' => "ro",
        'わ' => "wa", 'ゐ' => "i", 'ゑ' => "e", 'を' => "o",
        'ゔ' => "vu",
        // Small kana met on their own.
        'ぁ' => "a", 'ぃ' => "i", 'ぅ' => "u", 'ぇ' => "e", 'ぉ' => "o",
        'ゃ' => "ya", 'ゅ' => "yu", 'ょ' => "yo", 'ゎ' => "wa",
        'ゕ' => "ka", 'ゖ' => "ke",
        _ => return None,
    })
}

fn small_vowel(c: char) -> Option<char> {
    Some(match c {
        'ぁ' => 'a',
        'ぃ' => 'i',
        'ぅ' => 'u',
        'ぇ' => 'e',
        'ぉ' => 'o',
        _ => return None,
    })
}

/// ー: the vowel before it, long. "kēki", "tōkyō".
fn lengthen(out: &mut String) {
    let long = match out.chars().last() {
        Some('a') => 'ā',
        Some('i') => 'ī',
        Some('u') => 'ū',
        Some('e') => 'ē',
        Some('o') => 'ō',
        _ => return,
    };
    out.pop();
    out.push(long);
}

fn punctuation(c: char) -> Option<&'static str> {
    Some(match c {
        '、' => ",",
        '。' => ".",
        '！' => "!",
        '？' => "?",
        '「' | '」' | '『' | '』' => "\"",
        '・' => " ",
        '　' => " ",
        '～' | '〜' => "~",
        _ => return None,
    })
}

fn is_vowel(c: char) -> bool {
    matches!(c, 'a' | 'i' | 'u' | 'e' | 'o')
}

fn is_hiragana(c: char) -> bool {
    ('\u{3041}'..='\u{3096}').contains(&c)
}

fn is_katakana(c: char) -> bool {
    ('\u{30A1}'..='\u{30FA}').contains(&c) || c == 'ー'
}

/// Katakana as hiragana (ア → あ); anything else as it is. ー stays.
pub(crate) fn to_hiragana(c: char) -> char {
    match c {
        '\u{30A1}'..='\u{30F6}' => char::from_u32(c as u32 - 0x60).unwrap_or(c),
        _ => c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hiragana_and_katakana_read_as_hepburn() {
        assert_eq!(kana_to_romaji("さくら"), "sakura");
        assert_eq!(kana_to_romaji("しちつふじづ"), "shichitsufujizu");
        assert_eq!(kana_to_romaji("サクラ"), "sakura");
    }

    #[test]
    fn small_ya_yu_yo_make_one_syllable() {
        assert_eq!(kana_to_romaji("きょう"), "kyou");
        assert_eq!(kana_to_romaji("しゃしん"), "shashin");
        assert_eq!(kana_to_romaji("ちゃ"), "cha");
        assert_eq!(kana_to_romaji("じゅう"), "juu");
        assert_eq!(kana_to_romaji("りょ"), "ryo");
    }

    #[test]
    fn small_tsu_doubles_and_before_ch_is_t() {
        assert_eq!(kana_to_romaji("きって"), "kitte");
        assert_eq!(kana_to_romaji("まっちゃ"), "matcha");
        assert_eq!(kana_to_romaji("ずっと"), "zutto");
        assert_eq!(kana_to_romaji("あっ"), "a");
    }

    #[test]
    fn n_takes_an_apostrophe_before_a_vowel_or_y() {
        assert_eq!(kana_to_romaji("きんようび"), "kin'youbi");
        assert_eq!(kana_to_romaji("せんえん"), "sen'en");
        assert_eq!(kana_to_romaji("しんぶん"), "shinbun");
        assert_eq!(kana_to_romaji("ほん"), "hon");
    }

    #[test]
    fn the_long_mark_is_a_macron() {
        assert_eq!(kana_to_romaji("ケーキ"), "kēki");
        assert_eq!(kana_to_romaji("トーキョー"), "tōkyō");
        assert_eq!(kana_to_romaji("ー"), "");
    }

    #[test]
    fn loanword_sounds() {
        assert_eq!(kana_to_romaji("パーティー"), "pātī");
        assert_eq!(kana_to_romaji("ファン"), "fan");
        assert_eq!(kana_to_romaji("ウィンドウ"), "windou");
        assert_eq!(kana_to_romaji("ヴァイオリン"), "vaiorin");
        assert_eq!(kana_to_romaji("チェック"), "chekku");
        assert_eq!(kana_to_romaji("ディスコ"), "disuko");
    }

    #[test]
    fn wo_is_o_and_other_text_passes_through() {
        assert_eq!(kana_to_romaji("を"), "o");
        assert_eq!(kana_to_romaji("東京へ"), "東京he");
        assert_eq!(kana_to_romaji("Hello, 世界！"), "Hello, 世界!");
        assert_eq!(kana_to_romaji("「あ」、い。"), "\"a\",i.");
    }

    #[test]
    fn japanese_has_kana_and_chinese_does_not() {
        assert!(is_japanese("夜空に輝く星"));
        assert!(is_japanese("カタカナ"));
        assert!(!is_japanese("我爱你"));
        assert!(!is_japanese("plain English"));
    }
}
