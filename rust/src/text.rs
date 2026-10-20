//! Title matching as in the Python scripts (normalize, accent folding, difflib's
//! SequenceMatcher ratio), and the Python rules for URL quoting and repr in messages.

use regex::Regex;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::canonical_combining_class;

/// Title and artist similarity needed to count as the same song.
pub const MATCH_THRESHOLD: f64 = 0.85;

// Version and feature tags Spotify adds to titles that library files usually do not have.
// shortcut: Rust's \b counts combining marks as word characters and Python's does not, so a
// mark or a "²" next to a tag word can differ; upgrade if a parity run shows such a title.
static NOISE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\s*[\(\[-][^)\]]*\b(remaster(ed)?|feat\.?|ft\.?|with|version|edit|mono|stereo|live|deluxe|bonus|explicit)\b[^)\]]*[\)\]]?",
    )
    .unwrap()
});
// Python's [^\w\s]: its \w is letters, numbers and "_", and its \s also has \x1c to \x1f.
static NOT_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}_\s\x1c-\x1f]").unwrap());
// All parenthetical annotations, for the stem comparison only. Dash tags are NOISE's job.
static ALL_PARENS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*[\(\[][^\)\]]*[\)\]]?").unwrap());

/// Python's str.isspace.
fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// Lowercase, without version tags and punctuation, so the same song compares equal.
pub fn normalize(text: &str) -> String {
    let text = NOISE.replace_all(text, "").to_lowercase();
    let text = NOT_WORD.replace_all(&text, " ");
    text.split(is_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn fold_accents(text: &str) -> String {
    text.nfkd()
        .filter(|&c| canonical_combining_class(c) == 0)
        .collect()
}

/// The sync's similarity: accents folded, and equal bare stems count as a match.
pub fn similar(a: &str, b: &str) -> f64 {
    let (a, b) = (fold_accents(a), fold_accents(b));
    let base = ratio(&normalize(&a), &normalize(&b));
    // "(feat. Drake)", "(PMEDIA)" or "(Deluxe)" on one side only lowers the ratio without a
    // real mismatch, so equal stems with all parens stripped get a boost.
    let sa = normalize(&ALL_PARENS.replace_all(&a, ""));
    let sb = normalize(&ALL_PARENS.replace_all(&b, ""));
    if !sa.is_empty() && sa == sb {
        base.max(0.95)
    } else {
        base
    }
}

/// The Exportify import's similarity: normalized titles only.
pub fn similar_plain(a: &str, b: &str) -> f64 {
    ratio(&normalize(a), &normalize(b))
}

/// difflib.SequenceMatcher(None, a, b).ratio(), autojunk included, on code points.
pub fn ratio(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let total = a.len() + b.len();
    if total == 0 {
        return 1.0;
    }
    2.0 * matching_chars(&a, &b) as f64 / total as f64
}

fn matching_chars(a: &[char], b: &[char]) -> usize {
    let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
    for (j, &c) in b.iter().enumerate() {
        b2j.entry(c).or_default().push(j);
    }
    // autojunk: in a long b, drop the characters that fill more than 1% of it.
    if b.len() >= 200 {
        let ntest = b.len() / 100 + 1;
        b2j.retain(|_, idxs| idxs.len() <= ntest);
    }
    let mut queue = vec![(0, a.len(), 0, b.len())];
    let mut total = 0;
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = longest_match(a, b, &b2j, (alo, ahi, blo, bhi));
        if k > 0 {
            total += k;
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    total
}

/// find_longest_match without junk: the popular characters are left out of b2j but the
/// extension steps still take them, as in difflib.
fn longest_match(
    a: &[char],
    b: &[char],
    b2j: &HashMap<char, Vec<usize>>,
    (alo, ahi, blo, bhi): (usize, usize, usize, usize),
) -> (usize, usize, usize) {
    let (mut besti, mut bestj, mut size) = (alo, blo, 0);
    let mut j2len: HashMap<usize, usize> = HashMap::new();
    for (i, c) in a.iter().enumerate().take(ahi).skip(alo) {
        let mut next = HashMap::new();
        for &j in b2j.get(c).map_or(&[][..], Vec::as_slice) {
            if j < blo {
                continue;
            }
            if j >= bhi {
                break;
            }
            let k = j.checked_sub(1).and_then(|p| j2len.get(&p)).unwrap_or(&0) + 1;
            next.insert(j, k);
            if k > size {
                (besti, bestj, size) = (i + 1 - k, j + 1 - k, k);
            }
        }
        j2len = next;
    }
    while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
        (besti, bestj, size) = (besti - 1, bestj - 1, size + 1);
    }
    while besti + size < ahi && bestj + size < bhi && a[besti + size] == b[bestj + size] {
        size += 1;
    }
    (besti, bestj, size)
}

/// urllib.parse.quote: UTF-8 bytes as %XX, except letters, digits, "_.-~" and `safe`.
pub fn quote(s: &str, safe: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) || safe.as_bytes().contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// urllib.parse.quote_plus.
pub fn quote_plus(s: &str) -> String {
    quote(s, " ").replace(' ', "+")
}

/// urllib.parse.urlencode of key and value pairs.
pub fn urlencode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", quote_plus(k), quote_plus(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Python's repr of a str, for messages that print names with {name!r}.
pub fn py_repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(q);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == q => out.extend(['\\', c]),
            c if c.is_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

/// Counter(keys).most_common(): the most frequent first, ties in first-seen order.
pub fn most_common<K: Eq + Hash + Clone>(keys: impl IntoIterator<Item = K>) -> Vec<(K, usize)> {
    let mut index: HashMap<K, usize> = HashMap::new();
    let mut counts: Vec<(K, usize)> = Vec::new();
    for key in keys {
        match index.get(&key) {
            Some(&i) => counts[i].1 += 1,
            None => {
                index.insert(key.clone(), counts.len());
                counts.push((key, 1));
            }
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    counts
}

/// sys.exit(message): the message on stderr and exit status 1.
pub fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_equals_python_difflib_on_real_titles() {
        // Expected values from python3 difflib.SequenceMatcher(None, a, b).ratio(); the last
        // pairs are long enough to switch on autojunk.
        let pairs: &[(&str, &str, f64)] = &[
            (
                "Hyouriittai",
                "Hyouriittai (HunterxHunter Ver.)",
                0.5116279069767442,
            ),
            ("DARK AURA", "DARK SAUCE", 0.7368421052631579),
            (
                "LOST FOREVER (feat. Westside Gunn)",
                "Lost",
                0.15789473684210525,
            ),
            (
                "Stereo Hearts (feat. Adam Levine)",
                "Stereo Love",
                0.36363636363636365,
            ),
            ("André 3000", "André Previn", 0.5454545454545454),
            ("BROKEN HALO", "BROKEN ROAD", 0.7272727272727273),
            ("Paranoid", "Paranoid (Remaster)", 0.5925925925925926),
            ("Blue Bird (Naruto)", "Blue Notes", 0.5),
            (
                "KERAUNOS KILLER (Speed Up)",
                "KERAUNOS KILLER - Speed Up",
                0.9230769230769231,
            ),
            ("Red Hot Chili Peppers", "Red Nation", 0.3225806451612903),
            ("BABY BOI", "Baby Bash", 0.35294117647058826),
            (
                "Nocturne No. 19 in E Minor, Op. 72 No. 1",
                "Nocturne in C minor, Op. 48, No. 1",
                0.7837837837837838,
            ),
            ("FUTURE", "Future", 0.16666666666666666),
            ("Acid Rain", "Acid Rap", 0.8235294117647058),
            (
                "Better Days",
                "Better Days (NEIKED x Mae Muller x Polo G)",
                0.41509433962264153,
            ),
            ("Long Live Clawz", "Long Time - Intro", 0.5),
            ("Little Dark Age", "Little Simz", 0.5384615384615384),
            ("BLOODY STREAM", "Bloody Mary", 0.25),
            ("Good News", "Break Stuff", 0.1),
            ("Self Care", "NDTGOK", 0.0),
            ("BIRDS OF A FEATHER", "So Many Tears", 0.25806451612903225),
            ("Coldest Winter", "G3 GELO", 0.09523809523809523),
            (
                "Through The Wire",
                "The Way Life Goes (feat. Oh Wonder)",
                0.27450980392156865,
            ),
            (
                "Sunlight On Your Skin - Bonus Track",
                "BR1X3R",
                0.04878048780487805,
            ),
            ("Stoney (Deluxe)", "Survival Tactics", 0.12903225806451613),
            (
                "Maurice Ravel",
                "Annihilate (Spider-Man: Across the Spider-Verse) (Metro Boomin & Swae Lee, Lil Wayne, Offset)",
                0.1509433962264151,
            ),
            ("Fugitive", "Anderson .Paak", 0.09090909090909091),
            ("Show Me", "Crystal Castles (II)", 0.14814814814814814),
            ("Immortal", "immortal (Remastered 2011)", 0.4117647058823529),
            ("Murs", "murs (Remastered 2011)", 0.23076923076923078),
            (
                "DONNY DARKO (FEAT. LIL UZI VERT)",
                "donny darko (feat. lil uzi vert) (Remastered 2011)",
                0.21951219512195122,
            ),
            (
                "Saint Laurent Mask",
                "saint laurent mask (Remastered 2011)",
                0.5555555555555556,
            ),
            (
                "Chopin: Études",
                "chopin: études (Remastered 2011)",
                0.5217391304347826,
            ),
            ("Stahhr", "stahhr (Remastered 2011)", 0.3333333333333333),
            (
                "Jordan Ward / Skepta / Woods / Eazy / California Love - Original Version / Summer Knights / Stairway to Heaven - Remaster / Trance (with Travis Scott & Young Thug) / Estelle / Love You Like A Love Song (David Laid Version) / CHROMAKOPIA / Schumann: Piano Music / g3ox_em / BULLY / Snow On Tha Bluff / BJ Lips / The Neighbourhood / Feel It (From “Invincible”) / Push The Feeling On / Wiz Khalifa / SKELETONS / NOT ALL HEROES WEAR CAPES (Deluxe) / KAYTRANADA / PHONK SPED UP SONGS | POPULAR SPED UP PHONK SONGS | TIK TOK SPED UP PHONK SONGS EP / Survival Tactics",
                "Olivia Dean / Loaded 9 / JKER / HAARPER / ESCAPE PLAN / MAFIA / a lot / Gravity / Black&Blue / Stereo Love - Slowed To Perfection / Ja Rule / Ronald Isley / Punkinloveee / Land Of The Snakes / noturgf / OLYMPIAN / Igor Levit / The Boys Choir Of Harlem / Beethoven: The Complete Piano Sonatas / Chris Brown / Reasonable Doubt / Ms. Jackson / Don't Be Dumb / 大主宰乐团 / Lady - Hear Me Tonight / Dave Chappelle",
                0.026970954356846474,
            ),
            (
                "Jordan Ward / Skepta / Woods / Eazy / California Love - Original Version / Summer Knights / Stairway to Heaven - Remaster / Trance (with Travis Scott & Young Thug) / Estelle / Love You Like A Love Song (David Laid Version) / CHROMAKOPIA / Schumann: Piano Music / g3ox_em / BULLY / Snow On Tha Bluff / BJ Lips / The Neighbourhood / Feel It (From “Invincible”) / Push The Feeling On / Wiz Khalifa / SKELETONS / NOT ALL HEROES WEAR CAPES (Deluxe) / KAYTRANADA / PHONK SPED UP SONGS | POPULAR SPED UP PHONK SONGS | TIK TOK SPED UP PHONK SONGS EP / Survival Tactics",
                "scitcaT lavivruS / PE SGNOS KNOHP PU DEPS KOT KIT | SGNOS KNOHP PU DEPS RALUPOP | SGNOS PU DEPS KNOHP / ADANARTYAK / )exuleD( SEPAC RAEW SEOREH LLA TON / SNOTELEKS / afilahK ziW / nO gnileeF ehT hsuP / )”elbicnivnI“ morF( tI leeF / doohruobhgieN ehT / spiL JB / ffulB ahT nO wonS / YLLUB / me_xo3g / cisuM onaiP :nnamuhcS / AIPOKAMORHC / )noisreV diaL divaD( gnoS evoL A ekiL uoY evoL / elletsE / )guhT gnuoY & ttocS sivarT htiw( ecnarT / retsameR - nevaeH ot yawriatS / sthginK remmuS / noisreV lanigirO - evoL ainrofilaC / yzaE / sdooW / atpekS / draW nadroJ",
                0.02142857142857143,
            ),
            ("", "", 1.0),
            ("", "x", 0.0),
            ("Beyoncé", "Beyonce", 0.8571428571428571),
            ("Red", "Red Dead Redemption", 0.2727272727272727),
            (
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaab",
                "baaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                0.0033222591362126247,
            ),
        ];
        assert!(pairs.len() >= 30);
        for (a, b, want) in pairs {
            assert_eq!(ratio(a, b), *want, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn normalize_drops_tags_and_punctuation() {
        assert_eq!(normalize("Money Trees (feat. Jay Rock)"), "money trees");
        assert_eq!(normalize("Hey Jude - Remastered 2015"), "hey jude");
        assert_eq!(normalize("Ain't  It_Fun?"), "ain t it_fun");
    }

    #[test]
    fn pmedia_suffix_matches() {
        assert!(similar("United in Grief (PMEDIA)", "United in Grief") >= MATCH_THRESHOLD);
    }

    #[test]
    fn short_prefix_does_not_match_unrelated_title() {
        assert!(similar("Red", "Red Dead Redemption") < MATCH_THRESHOLD);
    }

    #[test]
    fn feat_on_one_side_pmedia_on_other() {
        let s = similar("One More Night (feat. Drake)", "One More Night (PMEDIA)");
        assert!(s >= MATCH_THRESHOLD);
    }

    #[test]
    fn accented_artist_names_match_plain_ones() {
        assert!(similar("JAŸ-Z", "Jay-Z") >= MATCH_THRESHOLD);
        assert!(similar("Beyoncé", "Beyonce") >= MATCH_THRESHOLD);
    }

    #[test]
    fn quoting_and_repr_follow_python() {
        assert_eq!(
            quote_plus("AC/DC Back in Black+"),
            "AC%2FDC+Back+in+Black%2B"
        );
        assert_eq!(quote("Sigur Rós/x", "/"), "Sigur%20R%C3%B3s/x");
        assert_eq!(py_repr("it's"), "\"it's\"");
        assert_eq!(py_repr("New"), "'New'");
    }
}
