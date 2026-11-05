//! Which bare words SurrealQL accepts as a type name.
//!
//! SurrealDB 4.0.0's kind parser (`parser-next/src/parse/kind.rs` at
//! `284fe93af`) accepts the closed builtin list below. Any other ident is a
//! named kind: a `DEFINE TYPE` name, not a parse error. The diagnostic that
//! uses this list stays silent for a name the same file defines, and for a
//! named record-id component. A word that is neither still reports, so a typo
//! such as `strng` stays visible.
//!
//! That is the whole reason this module can be certain where
//! [`crate::semantic::assign`] deliberately is not. `assign` answers "is this
//! coercion safe to judge?", and its doc comment requires silence on an
//! unrecognised name so a new SurrealDB release cannot make the checker flag
//! working code. This module answers a different and much narrower question —
//! "does the parser have this word at all?" — where the answer is a fact about a
//! finite keyword table rather than an inference. The two must not be merged:
//! [`PRIMITIVES`](crate::semantic::assign) is a coercion allowlist and diverges
//! from this list on purpose.

/// The type names SurrealDB's kind grammar accepts.
///
/// Transcribed from the kind parser. The first line was read from
/// `parse_concrete_kind` (`syn/parser/kind.rs:45-220`) and checked against a
/// live 3.2.3 engine. The second line was read from
/// `parser-next/src/parse/kind.rs` at SurrealDB `284fe93af` (4.0.0): the sized
/// numbers, `bigdecimal`, `date`, `time`, `timestamp`, `interval` and `enum`.
///
/// Three entries need a word of explanation:
///
/// * `option` is here although it is legal *only* as `option<T>` — bare `option`
///   fails with `expected <`. `enum` is the same: the parser accepts it only as
///   `enum<'a', 'b'>`. Arity is a separate rule this module does not model, so
///   the name counts as known.
/// * `i64` is the engine's spelling of `int`, and `f64` of `float`. Both words
///   parse, so both are known.
/// * `value` is deliberately **absent**. `LET $x: value = 1` fails with
///   ``Unexpected token `VALUE`, expected a kind name``, even though `VALUE` is a
///   keyword elsewhere in the language. Note that `assign::PRIMITIVES` does list
///   `value`, because there it means "the top of the lattice", not "a name an
///   author may write".
pub const KIND_NAMES: &[&str] = &[
    "any",
    "array",
    "bool",
    "bytes",
    "datetime",
    "decimal",
    "duration",
    "file",
    "float",
    "function",
    "geometry",
    "int",
    "none",
    "null",
    "number",
    "object",
    "option",
    "point",
    "range",
    "record",
    "regex",
    "set",
    "string",
    "table",
    "uuid",
    "bigdecimal",
    "date",
    "enum",
    "f16",
    "f32",
    "f64",
    "i16",
    "i32",
    "i64",
    "i8",
    "interval",
    "time",
    "timestamp",
    "u16",
    "u32",
    "u64",
    "u8",
];

/// The two boolean literal types, which reach a type position as a bare
/// `TypeName`.
///
/// The engine reads them as literal types (`t!("true") => Kind::Literal(…)`), so
/// `<true> true` and `LET $x: false = 1` are both legal — SurrealDB's own
/// `casting/basic_literal_kind` test relies on it.
///
/// Kept apart from [`KIND_NAMES`] because they are not kind *names*, and they are
/// here at all only because the tree-sitter `LiteralType` rule covers `String`,
/// `Number`, `Duration`, `ArrayType` and `ObjectType` but omits booleans. `NaN`
/// and `Infinity` need no entry: the grammar reads those as
/// `LiteralType(Number(Float))`, so they never arrive as a `TypeName`.
const BOOLEAN_LITERAL_TYPES: &[&str] = &["true", "false"];

/// Constructors whose `<…>` arguments name something other than a type.
///
/// This list is what stops the check reporting valid SurrealQL. `record<person>`,
/// `table<person>`, `file<bucket>` and `geometry<multipoint>` all parse to the very
/// same shape, `ParameterizedType(TypeName, TypeName)`, so the argument cannot be
/// told apart from a type by its node kind alone:
///
/// * `record` and `table` take table names,
/// * `file` takes bucket names,
/// * `geometry` takes a name from its own closed set (`point`, `line`, `polygon`,
///   `multipoint`, `multiline`, `multipolygon`, `collection`), which this module
///   does not check.
///
/// `array`, `set` and `option` are absent on purpose: their arguments really are
/// types, and `array<xxx>` must be reported.
pub const FOREIGN_ARGUMENT_CONSTRUCTORS: &[&str] = &["record", "table", "file", "geometry"];

/// True when SurrealDB's kind grammar has this name.
///
/// Case-insensitive, because the engine lexes these through `UniCase`
/// (`syn/lexer/keywords.rs`) and so accepts `INT` and `String`.
pub fn is_known(name: &str) -> bool {
    KIND_NAMES
        .iter()
        .chain(BOOLEAN_LITERAL_TYPES)
        .any(|known| known.eq_ignore_ascii_case(name))
}

/// True when `name` is a constructor whose `<…>` arguments are not types.
pub fn takes_foreign_arguments(name: &str) -> bool {
    FOREIGN_ARGUMENT_CONSTRUCTORS
        .iter()
        .any(|known| known.eq_ignore_ascii_case(name))
}

/// The closest real type name to `name`, for a "did you mean" hint.
///
/// Two guards, both borrowed from [`super::analyzer`]'s `keyword_typo_hint`:
///
/// * a `jaro_winkler` score above [`NEAR_MISS_THRESHOLD`],
/// * a length difference of three characters or less, and
/// * the same first letter. `num` is three from both `number` and `enum`, and
///   without the letter `enum` wins.
///
/// The length guard is what makes this safe on a list of very short words. Without
/// it the prefix bonus pairs any two names that merely start alike. Three rather
/// than two because the useful abbreviations are exactly three characters short of
/// their target — `str`, `rec`, `num` and `obj` all land on the right name at 0.88,
/// while unrelated words (`text`, `json`, `char`) stay well under the threshold.
///
/// # This length guard is a recall choice, not a fast path
///
/// Do not copy the `abs_diff(len) <= 3` shape into a sweep over user-defined
/// names. It is **not** sound as a prefilter: `person` and `personaddress` differ
/// by 7 characters and still score 0.892, so on a table or field sweep it would
/// silently drop real near-misses. It is defensible here only because
/// [`KIND_NAMES`] is a closed list of short words where a loose match is worse
/// than no match, and because the candidates cost nothing to score.
///
/// The sound prefilter, for the sweeps where speed matters, is
/// [`crate::semantic::model::can_reach_near_miss_threshold`].
///
/// [`NEAR_MISS_THRESHOLD`]: crate::semantic::model::NEAR_MISS_THRESHOLD
pub fn nearest(name: &str) -> Option<&'static str> {
    let query = name.to_ascii_lowercase();
    KIND_NAMES
        .iter()
        .filter(|known| {
            known.len().abs_diff(query.len()) <= 3
                && known.starts_with(query.chars().next().unwrap_or('\0'))
        })
        .map(|known| (strsim::jaro_winkler(&query, known), *known))
        .filter(|(score, _)| *score > crate::semantic::model::NEAR_MISS_THRESHOLD)
        .max_by(|left, right| {
            left.0
                .partial_cmp(&right.0)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(_, known)| known)
}

/// Names introduced by `DEFINE TYPE` in this file, lowercased.
///
/// The grammar has no `DEFINE TYPE` alternative, so the statement arrives as an
/// error node and the name never becomes a definition the tree can walk. The
/// text is the authority the engine uses: `DEFINE TYPE`, then optional
/// `OVERWRITE` or `IF NOT EXISTS`, then the name. A name inside a string or a
/// comment is not a definition.
pub fn defined_type_names(source: &str) -> std::collections::HashSet<String> {
    let bytes = source.as_bytes();
    let mut names = std::collections::HashSet::new();
    let mut index = 0;
    while index < bytes.len() {
        if !source.is_char_boundary(index) {
            index += 1;
            continue;
        }
        if bytes[index] == b'\'' || bytes[index] == b'"' {
            index = skip_string(source, index);
            continue;
        }
        if source[index..].starts_with("--") || source[index..].starts_with("//") {
            index = source[index..]
                .find('\n')
                .map(|end| index + end + 1)
                .unwrap_or(source.len());
            continue;
        }
        if source[index..].starts_with("/*") {
            index = source[index + 2..]
                .find("*/")
                .map(|end| index + 2 + end + 2)
                .unwrap_or(source.len());
            continue;
        }
        if is_ident_start(bytes[index]) {
            let start = index;
            index += 1;
            while index < bytes.len() && is_ident_cont(bytes[index]) {
                index += 1;
            }
            if source[start..index].eq_ignore_ascii_case("define")
                && let Some(name) = type_defined_at(source, index)
            {
                names.insert(name);
            }
            continue;
        }
        index += source[index..]
            .chars()
            .next()
            .map(|ch| ch.len_utf8())
            .unwrap_or(1);
    }
    names
}

fn type_defined_at(source: &str, after_define: usize) -> Option<String> {
    let (word, mut at) = next_word(source, after_define)?;
    if word != "type" {
        return None;
    }
    loop {
        let (word, next) = next_word(source, at)?;
        match word.as_str() {
            "overwrite" => at = next,
            "if" => {
                let (not, next) = next_word(source, next)?;
                if not != "not" {
                    return None;
                }
                let (exists, next) = next_word(source, next)?;
                if exists != "exists" {
                    return None;
                }
                at = next;
            }
            name => return Some(name.to_string()),
        }
    }
}

fn next_word(source: &str, mut index: usize) -> Option<(String, usize)> {
    index = skip_gap(source, index);
    let bytes = source.as_bytes();
    if index >= bytes.len() {
        return None;
    }
    if bytes[index] == b'`' {
        let end = source[index + 1..].find('`')?;
        let name = source[index + 1..index + 1 + end].to_ascii_lowercase();
        return Some((name, index + 1 + end + 1));
    }
    if !is_ident_start(bytes[index]) {
        return None;
    }
    let start = index;
    index += 1;
    while index < bytes.len() && is_ident_cont(bytes[index]) {
        index += 1;
    }
    Some((source[start..index].to_ascii_lowercase(), index))
}

fn skip_gap(source: &str, mut index: usize) -> usize {
    let bytes = source.as_bytes();
    loop {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index < bytes.len()
            && (source[index..].starts_with("--") || source[index..].starts_with("//"))
        {
            index = source[index..]
                .find('\n')
                .map(|end| index + end + 1)
                .unwrap_or(source.len());
            continue;
        }
        if index < bytes.len() && source[index..].starts_with("/*") {
            index = source[index + 2..]
                .find("*/")
                .map(|end| index + 2 + end + 2)
                .unwrap_or(source.len());
            continue;
        }
        return index;
    }
}

fn skip_string(source: &str, start: usize) -> usize {
    let bytes = source.as_bytes();
    if source[start..].starts_with("\"\"\"") {
        return source[start + 3..]
            .find("\"\"\"")
            .map(|end| start + 3 + end + 3)
            .unwrap_or(source.len());
    }
    let quote = bytes[start];
    let mut index = start + 1;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index += 2;
            continue;
        }
        if bytes[index] == quote {
            return index + 1;
        }
        index += 1;
    }
    source.len()
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_ident_cont(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_kind_names_surrealdb_4_added() {
        // `parser-next/src/parse/kind.rs` at 284fe93af. A missing name here is
        // an `unknown-type` on code the engine parses.
        for name in [
            "date",
            "time",
            "timestamp",
            "interval",
            "enum",
            "i8",
            "u64",
            "f16",
            "f64",
            "i64",
            "bigdecimal",
        ] {
            assert!(is_known(name), "{name} must be known");
        }
    }

    #[test]
    fn accepts_every_name_the_engine_accepts() {
        for name in KIND_NAMES {
            assert!(is_known(name), "{name} must be known");
        }
    }

    #[test]
    fn ignores_letter_case_because_the_engine_does() {
        // `LET $a: INT = 2` and `LET $a: String = 2` both parse in the engine.
        for name in ["INT", "String", "RECORD", "Option", "TABLE"] {
            assert!(is_known(name), "{name} must be known");
        }
    }

    #[test]
    fn rejects_a_word_the_kind_grammar_does_not_have() {
        for name in ["xxx", "strng", "integer", "boolean", "text", "json", "char"] {
            assert!(!is_known(name), "{name} must not be known");
        }
    }

    #[test]
    fn rejects_value_although_it_is_a_keyword_elsewhere() {
        // `LET $x: value = 1` -> "Unexpected token `VALUE`, expected a kind name".
        // `assign::PRIMITIVES` lists it for a different purpose; see this module's
        // doc comment.
        assert!(!is_known("value"));
    }

    #[test]
    fn accepts_table_which_the_coercion_allowlist_omits() {
        // `LET $x: table = 1` parses. `assign::PRIMITIVES` does not list `table`,
        // which is a separate, pre-existing divergence.
        assert!(is_known("table"));
    }

    #[test]
    fn suggests_the_obvious_correction() {
        assert_eq!(nearest("strng"), Some("string"));
        assert_eq!(nearest("nubmer"), Some("number"));
        assert_eq!(nearest("recrod"), Some("record"));
        assert_eq!(nearest("datetim"), Some("datetime"));
    }

    #[test]
    fn suggests_the_target_of_a_short_abbreviation() {
        // The reason the length guard is three and not two.
        assert_eq!(nearest("str"), Some("string"));
        assert_eq!(nearest("rec"), Some("record"));
        assert_eq!(nearest("num"), Some("number"));
        assert_eq!(nearest("obj"), Some("object"));
    }

    #[test]
    fn stays_quiet_when_nothing_is_close() {
        // A wrong suggestion is worse than none: it sends the reader to a type that
        // is not what they meant.
        for name in ["xxx", "zzz", "yyy", "text", "json", "char", "varchar"] {
            assert_eq!(nearest(name), None, "{name} must get no suggestion");
        }
    }

    #[test]
    fn reads_define_type_names() {
        let names = defined_type_names(
            "DEFINE TYPE mood AS string;\n\
             DEFINE TYPE IF NOT EXISTS PosInt AS int;\n\
             DEFINE TYPE OVERWRITE `City` AS string;",
        );
        assert!(names.contains("mood"), "{names:?}");
        assert!(names.contains("posint"), "{names:?}");
        assert!(names.contains("city"), "{names:?}");
    }

    #[test]
    fn ignores_a_define_type_that_is_only_text() {
        let names = defined_type_names(
            "RETURN \"DEFINE TYPE mood AS string\";\n-- DEFINE TYPE hidden AS int\n",
        );
        assert!(names.is_empty(), "{names:?}");
    }

    #[test]
    fn reads_a_name_beside_a_multibyte_dash() {
        let names = defined_type_names("— DEFINE TYPE mood AS string;\n");
        assert!(names.contains("mood"), "{names:?}");
    }
}
