//! HTML character references: numeric references and the common named ones.

use std::borrow::Cow;

/// Longest named reference looked up.
const MAX_NAME: usize = 32;

/// Named references decoded (the rest are left as written).
const NAMED: &[(&str, &str)] = &[
    ("amp", "&"),
    ("lt", "<"),
    ("gt", ">"),
    ("quot", "\""),
    ("apos", "'"),
    ("nbsp", "\u{a0}"),
    ("ensp", "\u{2002}"),
    ("emsp", "\u{2003}"),
    ("thinsp", "\u{2009}"),
    ("zwnj", "\u{200c}"),
    ("zwj", "\u{200d}"),
    ("shy", "\u{ad}"),
    ("ndash", "\u{2013}"),
    ("mdash", "\u{2014}"),
    ("hellip", "\u{2026}"),
    ("lsquo", "\u{2018}"),
    ("rsquo", "\u{2019}"),
    ("sbquo", "\u{201a}"),
    ("ldquo", "\u{201c}"),
    ("rdquo", "\u{201d}"),
    ("bdquo", "\u{201e}"),
    ("laquo", "\u{ab}"),
    ("raquo", "\u{bb}"),
    ("lsaquo", "\u{2039}"),
    ("rsaquo", "\u{203a}"),
    ("bull", "\u{2022}"),
    ("middot", "\u{b7}"),
    ("prime", "\u{2032}"),
    ("Prime", "\u{2033}"),
    ("copy", "\u{a9}"),
    ("reg", "\u{ae}"),
    ("trade", "\u{2122}"),
    ("deg", "\u{b0}"),
    ("plusmn", "\u{b1}"),
    ("times", "\u{d7}"),
    ("divide", "\u{f7}"),
    ("minus", "\u{2212}"),
    ("le", "\u{2264}"),
    ("ge", "\u{2265}"),
    ("ne", "\u{2260}"),
    ("asymp", "\u{2248}"),
    ("infin", "\u{221e}"),
    ("larr", "\u{2190}"),
    ("rarr", "\u{2192}"),
    ("uarr", "\u{2191}"),
    ("darr", "\u{2193}"),
    ("harr", "\u{2194}"),
    ("larrow", "\u{2190}"),
    ("rarrow", "\u{2192}"),
    ("check", "\u{2713}"),
    ("cent", "\u{a2}"),
    ("pound", "\u{a3}"),
    ("yen", "\u{a5}"),
    ("euro", "\u{20ac}"),
    ("curren", "\u{a4}"),
    ("sect", "\u{a7}"),
    ("para", "\u{b6}"),
    ("dagger", "\u{2020}"),
    ("Dagger", "\u{2021}"),
    ("permil", "\u{2030}"),
    ("frac12", "\u{bd}"),
    ("frac14", "\u{bc}"),
    ("frac34", "\u{be}"),
    ("sup1", "\u{b9}"),
    ("sup2", "\u{b2}"),
    ("sup3", "\u{b3}"),
    ("micro", "\u{b5}"),
    ("iexcl", "\u{a1}"),
    ("iquest", "\u{bf}"),
    ("ordf", "\u{aa}"),
    ("ordm", "\u{ba}"),
    ("not", "\u{ac}"),
    ("macr", "\u{af}"),
    ("acute", "\u{b4}"),
    ("cedil", "\u{b8}"),
    ("uml", "\u{a8}"),
    ("szlig", "\u{df}"),
    ("agrave", "\u{e0}"),
    ("aacute", "\u{e1}"),
    ("acirc", "\u{e2}"),
    ("atilde", "\u{e3}"),
    ("auml", "\u{e4}"),
    ("aring", "\u{e5}"),
    ("aelig", "\u{e6}"),
    ("ccedil", "\u{e7}"),
    ("egrave", "\u{e8}"),
    ("eacute", "\u{e9}"),
    ("ecirc", "\u{ea}"),
    ("euml", "\u{eb}"),
    ("igrave", "\u{ec}"),
    ("iacute", "\u{ed}"),
    ("icirc", "\u{ee}"),
    ("iuml", "\u{ef}"),
    ("ntilde", "\u{f1}"),
    ("ograve", "\u{f2}"),
    ("oacute", "\u{f3}"),
    ("ocirc", "\u{f4}"),
    ("otilde", "\u{f5}"),
    ("ouml", "\u{f6}"),
    ("oslash", "\u{f8}"),
    ("ugrave", "\u{f9}"),
    ("uacute", "\u{fa}"),
    ("ucirc", "\u{fb}"),
    ("uuml", "\u{fc}"),
    ("yacute", "\u{fd}"),
    ("yuml", "\u{ff}"),
    ("Agrave", "\u{c0}"),
    ("Aacute", "\u{c1}"),
    ("Acirc", "\u{c2}"),
    ("Atilde", "\u{c3}"),
    ("Auml", "\u{c4}"),
    ("Aring", "\u{c5}"),
    ("AElig", "\u{c6}"),
    ("Ccedil", "\u{c7}"),
    ("Egrave", "\u{c8}"),
    ("Eacute", "\u{c9}"),
    ("Ecirc", "\u{ca}"),
    ("Euml", "\u{cb}"),
    ("Igrave", "\u{cc}"),
    ("Iacute", "\u{cd}"),
    ("Icirc", "\u{ce}"),
    ("Iuml", "\u{cf}"),
    ("Ntilde", "\u{d1}"),
    ("Ograve", "\u{d2}"),
    ("Oacute", "\u{d3}"),
    ("Ocirc", "\u{d4}"),
    ("Otilde", "\u{d5}"),
    ("Ouml", "\u{d6}"),
    ("Oslash", "\u{d8}"),
    ("Ugrave", "\u{d9}"),
    ("Uacute", "\u{da}"),
    ("Ucirc", "\u{db}"),
    ("Uuml", "\u{dc}"),
    ("Yacute", "\u{dd}"),
    ("alpha", "\u{3b1}"),
    ("beta", "\u{3b2}"),
    ("gamma", "\u{3b3}"),
    ("delta", "\u{3b4}"),
    ("epsilon", "\u{3b5}"),
    ("lambda", "\u{3bb}"),
    ("mu", "\u{3bc}"),
    ("pi", "\u{3c0}"),
    ("sigma", "\u{3c3}"),
    ("omega", "\u{3c9}"),
    ("Delta", "\u{394}"),
    ("Sigma", "\u{3a3}"),
    ("Omega", "\u{3a9}"),
];

/// References a legacy page may write without the trailing `;`.
const LEGACY: &[&str] = &["amp", "lt", "gt", "quot", "nbsp", "copy", "reg"];

fn named(name: &str) -> Option<&'static str> {
    NAMED
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, value)| *value)
}

fn numeric(digits: &str, radix: u32) -> char {
    u32::from_str_radix(digits, radix)
        .ok()
        .filter(|&code| code != 0)
        .and_then(char::from_u32)
        .unwrap_or('\u{fffd}')
}

/// Decode character references in `text`; unknown references stay as written.
pub fn decode(text: &str) -> Cow<'_, str> {
    if !text.contains('&') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match reference(after) {
            Some((decoded, consumed)) => {
                out.push_str(&decoded);
                rest = &after[consumed..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// The decoded text and the bytes consumed after `&`, if `after` starts a
/// reference.
fn reference(after: &str) -> Option<(Cow<'static, str>, usize)> {
    if let Some(number) = after.strip_prefix('#') {
        let (digits, radix, prefix) = match number.strip_prefix(['x', 'X']) {
            Some(hex) => (hex, 16, 2),
            None => (number, 10, 1),
        };
        let length = digits
            .bytes()
            .take_while(|byte| {
                if radix == 16 {
                    byte.is_ascii_hexdigit()
                } else {
                    byte.is_ascii_digit()
                }
            })
            .count();
        if length == 0 || length > 8 {
            return None;
        }
        let terminated = digits[length..].starts_with(';');
        let ch = numeric(&digits[..length], radix);
        return Some((
            Cow::Owned(ch.to_string()),
            prefix + length + usize::from(terminated),
        ));
    }
    let length = after
        .bytes()
        .take(MAX_NAME + 1)
        .take_while(u8::is_ascii_alphanumeric)
        .count();
    if length == 0 || length > MAX_NAME {
        return None;
    }
    let name = &after[..length];
    if after[length..].starts_with(';') {
        return named(name).map(|value| (Cow::Borrowed(value), length + 1));
    }
    LEGACY
        .contains(&name)
        .then(|| named(name).map(|value| (Cow::Borrowed(value), length)))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_numeric_and_legacy_references_decode() {
        assert_eq!(decode("plain"), "plain");
        assert_eq!(
            decode("a &amp; b &lt;c&gt; &quot;d&quot; &#39;e&#x27; &mdash; &hellip;"),
            "a & b <c> \"d\" 'e' \u{2014} \u{2026}"
        );
        assert_eq!(decode("&copy 2024 &amp more"), "\u{a9} 2024 & more");
        assert_eq!(decode("&#x1F600;"), "\u{1F600}");
        assert_eq!(
            decode("&#0; &#xD800; &#99999999;"),
            "\u{fffd} \u{fffd} \u{fffd}"
        );
        assert_eq!(decode("&unknown; & &;"), "&unknown; & &;");
        assert_eq!(decode("AT&T"), "AT&T");
        assert_eq!(decode("&nbsp;x"), "\u{a0}x");
    }
}
