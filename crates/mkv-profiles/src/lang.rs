//! Language code normalization. Matroska files carry ISO 639-2 codes (both
//! the "B" bibliographic and "T" terminology variants appear in the wild,
//! e.g. `ger` and `deu`) or BCP 47 tags (`en`, `pt-BR`); MediaInfo reports
//! ISO 639-1 where one exists. Everything is normalized to one canonical code
//! (639-1 when available, else 639-2/T) before comparing.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Language {
    /// ISO 639-1 (may be empty for languages without one).
    pub iso1: &'static str,
    /// ISO 639-2/B.
    pub iso2b: &'static str,
    /// ISO 639-2/T.
    pub iso2t: &'static str,
    pub name: &'static str,
}

impl Language {
    /// 639-1 when it exists, else 639-2/T.
    pub fn code(&self) -> &'static str {
        if self.iso1.is_empty() {
            self.iso2t
        } else {
            self.iso1
        }
    }
}

macro_rules! langs {
    ($(($iso1:literal, $b:literal, $t:literal, $name:literal)),* $(,)?) => {
        &[$(Language { iso1: $iso1, iso2b: $b, iso2t: $t, name: $name }),*]
    };
}

pub static LANGUAGES: &[Language] = langs![
    ("aa", "aar", "aar", "Afar"), ("ab", "abk", "abk", "Abkhazian"), ("af", "afr", "afr", "Afrikaans"),
    ("ak", "aka", "aka", "Akan"), ("am", "amh", "amh", "Amharic"), ("an", "arg", "arg", "Aragonese"),
    ("ar", "ara", "ara", "Arabic"), ("as", "asm", "asm", "Assamese"), ("av", "ava", "ava", "Avaric"),
    ("ay", "aym", "aym", "Aymara"), ("az", "aze", "aze", "Azerbaijani"), ("ba", "bak", "bak", "Bashkir"),
    ("be", "bel", "bel", "Belarusian"), ("bg", "bul", "bul", "Bulgarian"), ("bi", "bis", "bis", "Bislama"),
    ("bm", "bam", "bam", "Bambara"), ("bn", "ben", "ben", "Bengali"), ("bo", "tib", "bod", "Tibetan"),
    ("br", "bre", "bre", "Breton"), ("bs", "bos", "bos", "Bosnian"), ("ca", "cat", "cat", "Catalan"),
    ("ce", "che", "che", "Chechen"), ("ch", "cha", "cha", "Chamorro"), ("co", "cos", "cos", "Corsican"),
    ("cr", "cre", "cre", "Cree"), ("cs", "cze", "ces", "Czech"), ("cu", "chu", "chu", "Church Slavic"),
    ("cv", "chv", "chv", "Chuvash"), ("cy", "wel", "cym", "Welsh"), ("da", "dan", "dan", "Danish"),
    ("de", "ger", "deu", "German"), ("dv", "div", "div", "Divehi"), ("dz", "dzo", "dzo", "Dzongkha"),
    ("ee", "ewe", "ewe", "Ewe"), ("el", "gre", "ell", "Greek"), ("en", "eng", "eng", "English"),
    ("eo", "epo", "epo", "Esperanto"), ("es", "spa", "spa", "Spanish"), ("et", "est", "est", "Estonian"),
    ("eu", "baq", "eus", "Basque"), ("fa", "per", "fas", "Persian"), ("ff", "ful", "ful", "Fulah"),
    ("fi", "fin", "fin", "Finnish"), ("fj", "fij", "fij", "Fijian"), ("fo", "fao", "fao", "Faroese"),
    ("fr", "fre", "fra", "French"), ("fy", "fry", "fry", "Western Frisian"), ("ga", "gle", "gle", "Irish"),
    ("gd", "gla", "gla", "Scottish Gaelic"), ("gl", "glg", "glg", "Galician"), ("gn", "grn", "grn", "Guarani"),
    ("gu", "guj", "guj", "Gujarati"), ("gv", "glv", "glv", "Manx"), ("ha", "hau", "hau", "Hausa"),
    ("he", "heb", "heb", "Hebrew"), ("hi", "hin", "hin", "Hindi"), ("ho", "hmo", "hmo", "Hiri Motu"),
    ("hr", "hrv", "hrv", "Croatian"), ("ht", "hat", "hat", "Haitian"), ("hu", "hun", "hun", "Hungarian"),
    ("hy", "arm", "hye", "Armenian"), ("hz", "her", "her", "Herero"), ("ia", "ina", "ina", "Interlingua"),
    ("id", "ind", "ind", "Indonesian"), ("ie", "ile", "ile", "Interlingue"), ("ig", "ibo", "ibo", "Igbo"),
    ("ii", "iii", "iii", "Sichuan Yi"), ("ik", "ipk", "ipk", "Inupiaq"), ("io", "ido", "ido", "Ido"),
    ("is", "ice", "isl", "Icelandic"), ("it", "ita", "ita", "Italian"), ("iu", "iku", "iku", "Inuktitut"),
    ("ja", "jpn", "jpn", "Japanese"), ("jv", "jav", "jav", "Javanese"), ("ka", "geo", "kat", "Georgian"),
    ("kg", "kon", "kon", "Kongo"), ("ki", "kik", "kik", "Kikuyu"), ("kj", "kua", "kua", "Kuanyama"),
    ("kk", "kaz", "kaz", "Kazakh"), ("kl", "kal", "kal", "Kalaallisut"), ("km", "khm", "khm", "Khmer"),
    ("kn", "kan", "kan", "Kannada"), ("ko", "kor", "kor", "Korean"), ("kr", "kau", "kau", "Kanuri"),
    ("ks", "kas", "kas", "Kashmiri"), ("ku", "kur", "kur", "Kurdish"), ("kv", "kom", "kom", "Komi"),
    ("kw", "cor", "cor", "Cornish"), ("ky", "kir", "kir", "Kyrgyz"), ("la", "lat", "lat", "Latin"),
    ("lb", "ltz", "ltz", "Luxembourgish"), ("lg", "lug", "lug", "Ganda"), ("li", "lim", "lim", "Limburgish"),
    ("ln", "lin", "lin", "Lingala"), ("lo", "lao", "lao", "Lao"), ("lt", "lit", "lit", "Lithuanian"),
    ("lu", "lub", "lub", "Luba-Katanga"), ("lv", "lav", "lav", "Latvian"), ("mg", "mlg", "mlg", "Malagasy"),
    ("mh", "mah", "mah", "Marshallese"), ("mi", "mao", "mri", "Maori"), ("mk", "mac", "mkd", "Macedonian"),
    ("ml", "mal", "mal", "Malayalam"), ("mn", "mon", "mon", "Mongolian"), ("mr", "mar", "mar", "Marathi"),
    ("ms", "may", "msa", "Malay"), ("mt", "mlt", "mlt", "Maltese"), ("my", "bur", "mya", "Burmese"),
    ("na", "nau", "nau", "Nauru"), ("nb", "nob", "nob", "Norwegian Bokmål"), ("nd", "nde", "nde", "North Ndebele"),
    ("ne", "nep", "nep", "Nepali"), ("ng", "ndo", "ndo", "Ndonga"), ("nl", "dut", "nld", "Dutch"),
    ("nn", "nno", "nno", "Norwegian Nynorsk"), ("no", "nor", "nor", "Norwegian"), ("nr", "nbl", "nbl", "South Ndebele"),
    ("nv", "nav", "nav", "Navajo"), ("ny", "nya", "nya", "Chichewa"), ("oc", "oci", "oci", "Occitan"),
    ("oj", "oji", "oji", "Ojibwa"), ("om", "orm", "orm", "Oromo"), ("or", "ori", "ori", "Odia"),
    ("os", "oss", "oss", "Ossetian"), ("pa", "pan", "pan", "Punjabi"), ("pi", "pli", "pli", "Pali"),
    ("pl", "pol", "pol", "Polish"), ("ps", "pus", "pus", "Pashto"), ("pt", "por", "por", "Portuguese"),
    ("qu", "que", "que", "Quechua"), ("rm", "roh", "roh", "Romansh"), ("rn", "run", "run", "Rundi"),
    ("ro", "rum", "ron", "Romanian"), ("ru", "rus", "rus", "Russian"), ("rw", "kin", "kin", "Kinyarwanda"),
    ("sa", "san", "san", "Sanskrit"), ("sc", "srd", "srd", "Sardinian"), ("sd", "snd", "snd", "Sindhi"),
    ("se", "sme", "sme", "Northern Sami"), ("sg", "sag", "sag", "Sango"), ("si", "sin", "sin", "Sinhala"),
    ("sk", "slo", "slk", "Slovak"), ("sl", "slv", "slv", "Slovenian"), ("sm", "smo", "smo", "Samoan"),
    ("sn", "sna", "sna", "Shona"), ("so", "som", "som", "Somali"), ("sq", "alb", "sqi", "Albanian"),
    ("sr", "srp", "srp", "Serbian"), ("ss", "ssw", "ssw", "Swati"), ("st", "sot", "sot", "Southern Sotho"),
    ("su", "sun", "sun", "Sundanese"), ("sv", "swe", "swe", "Swedish"), ("sw", "swa", "swa", "Swahili"),
    ("ta", "tam", "tam", "Tamil"), ("te", "tel", "tel", "Telugu"), ("tg", "tgk", "tgk", "Tajik"),
    ("th", "tha", "tha", "Thai"), ("ti", "tir", "tir", "Tigrinya"), ("tk", "tuk", "tuk", "Turkmen"),
    ("tl", "tgl", "tgl", "Tagalog"), ("tn", "tsn", "tsn", "Tswana"), ("to", "ton", "ton", "Tongan"),
    ("tr", "tur", "tur", "Turkish"), ("ts", "tso", "tso", "Tsonga"), ("tt", "tat", "tat", "Tatar"),
    ("tw", "twi", "twi", "Twi"), ("ty", "tah", "tah", "Tahitian"), ("ug", "uig", "uig", "Uyghur"),
    ("uk", "ukr", "ukr", "Ukrainian"), ("ur", "urd", "urd", "Urdu"), ("uz", "uzb", "uzb", "Uzbek"),
    ("ve", "ven", "ven", "Venda"), ("vi", "vie", "vie", "Vietnamese"), ("vo", "vol", "vol", "Volapük"),
    ("wa", "wln", "wln", "Walloon"), ("wo", "wol", "wol", "Wolof"), ("xh", "xho", "xho", "Xhosa"),
    ("yi", "yid", "yid", "Yiddish"), ("yo", "yor", "yor", "Yoruba"), ("za", "zha", "zha", "Zhuang"),
    ("zh", "chi", "zho", "Chinese"), ("zu", "zul", "zul", "Zulu"),
    // 639-2/3 only, but common in media files.
    ("", "fil", "fil", "Filipino"), ("", "yue", "yue", "Cantonese"), ("", "cmn", "cmn", "Mandarin"),
    ("", "tcy", "tcy", "Tulu"), ("", "kok", "kok", "Konkani"), ("", "bho", "bho", "Bhojpuri"),
    ("", "mai", "mai", "Maithili"), ("", "mni", "mni", "Manipuri"), ("", "sat", "sat", "Santali"),
    ("", "und", "und", "Undetermined"), ("", "zxx", "zxx", "No linguistic content"),
    ("", "mul", "mul", "Multiple languages"),
];

/// Resolves any common spelling of a language — 639-1, 639-2/B, 639-2/T, a
/// BCP 47 tag (`pt-BR`, `zh-Hans`) or an English name — case-insensitively.
pub fn lookup(code_or_name: &str) -> Option<&'static Language> {
    let raw = code_or_name.trim();
    if raw.is_empty() {
        return None;
    }
    // BCP 47: only the primary subtag identifies the language.
    let primary = raw.split(['-', '_']).next().unwrap_or(raw).to_ascii_lowercase();
    LANGUAGES
        .iter()
        .find(|l| l.iso1 == primary || l.iso2b == primary || l.iso2t == primary)
        .or_else(|| LANGUAGES.iter().find(|l| l.name.eq_ignore_ascii_case(raw)))
}

/// Canonical comparison code; unknown inputs are lower-cased as-is so they
/// still compare consistently with themselves.
pub fn canonical(code: &str) -> String {
    lookup(code)
        .map(|l| l.code().to_string())
        .unwrap_or_else(|| code.trim().to_ascii_lowercase())
}

pub fn same(a: &str, b: &str) -> bool {
    !a.trim().is_empty() && canonical(a) == canonical(b)
}

/// English display name ("Telugu"); unknown codes are shown upper-cased.
pub fn display_name(code: &str) -> String {
    lookup(code)
        .map(|l| l.name.to_string())
        .unwrap_or_else(|| code.trim().to_ascii_uppercase())
}

/// "Undetermined"/"no linguistic content" aren't real languages for the
/// purposes of language-based keep rules.
pub fn is_undetermined(code: &str) -> bool {
    matches!(canonical(code).as_str(), "" | "und" | "zxx" | "mul" | "mis")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_all_common_spellings() {
        for s in ["en", "eng", "EN", "en-US", "English", "english"] {
            assert_eq!(canonical(s), "en", "{s}");
        }
        for s in ["de", "ger", "deu", "de-AT"] {
            assert_eq!(canonical(s), "de", "{s}");
        }
        assert_eq!(canonical("tel"), "te");
        assert_eq!(canonical("mal"), "ml");
        assert_eq!(canonical("kan"), "kn");
        assert_eq!(canonical("tam"), "ta");
        assert_eq!(canonical("hin"), "hi");
        assert_eq!(canonical("chi"), "zh");
        assert_eq!(canonical("zh-Hans"), "zh");
        assert_eq!(canonical("fil"), "fil");
        assert_eq!(canonical("xyz"), "xyz");
        assert!(same("fre", "fra"));
        assert!(same("fr", "French"));
        assert!(!same("", ""));
    }

    #[test]
    fn display_names() {
        assert_eq!(display_name("tel"), "Telugu");
        assert_eq!(display_name("pt-BR"), "Portuguese");
        assert_eq!(display_name("xyz"), "XYZ");
        assert!(is_undetermined("und"));
        assert!(!is_undetermined("en"));
    }

    #[test]
    fn table_has_no_duplicate_codes() {
        let mut seen = std::collections::HashSet::new();
        for l in LANGUAGES {
            assert!(seen.insert(l.code()), "duplicate {}", l.code());
        }
    }
}
