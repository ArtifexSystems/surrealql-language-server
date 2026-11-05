#![cfg(feature = "server")]

//! Conformance of the argument checks against SurrealDB itself.
//!
//! The unit tests prove the checks fire where they should. These prove the far
//! more important half: that they stay **silent** on code the engine accepts. A
//! wrong diagnostic costs more than a missing one
//! (`src/semantic/assign.rs:1-16`), and an argument check runs at every call
//! site in every document, so a single bad catalogue entry squiggles working
//! queries.
//!
//! Two layers, because they fail in different situations:
//!
//! * [`valid_builtin_calls_stay_silent`] reads a committed fixture, so it runs
//!   everywhere — including continuous integration, which has no SurrealDB
//!   checkout.
//! * [`the_surrealdb_corpus_produces_only_expected_diagnostics`] sweeps all
//!   ~1,900 files of `language-tests/`. A pair is expected only when the file's
//!   front matter declares an `error =` whose wording matches the diagnostic.
//!   Every other pair is a false positive: it is listed in
//!   [`KNOWN_FALSE_POSITIVES`] with its cause, or the sweep fails. It skips
//!   without a checkout.
//!
//! To refresh the fixture, extract one call per distinct function from files
//! that declare no expected error:
//!
//! ```text
//! cd $SURREALDB/language-tests/tests
//! # keep lines ending in `;` that call a namespaced builtin, from files with
//! # no `error =` / `parsing-error`, skipping clause fragments and `api::`
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use surrealql_language_server::config::ServerSettings;
use surrealql_language_server::semantic::analyzer::analyze_document;
use surrealql_language_server::semantic::types::{
    MergedSemanticModel, SymbolOrigin, WorkspaceIndex,
};
use tower_lsp_server::ls_types::{Diagnostic, NumberOrString, Uri};

fn uri(path: &str) -> Uri {
    format!("file:///workspace/{path}").parse().expect("uri")
}

/// Every diagnostic this sweep judges for one document.
///
/// The semantic diagnostics, plus the one syntax diagnostic this crate reasons
/// about rather than merely relays: `unknown-type`. The other syntax codes are
/// left out on purpose — `parse` reports whatever the tree-sitter grammar cannot
/// read, including two shapes it rejects although SurrealDB accepts them
/// (a union in a `ParamDefinition`, and `array<float, 10>`). Those are grammar
/// defects tracked separately, and pulling them in would bury a real regression
/// in known noise.
fn diagnostics_for(source: &str) -> Vec<Diagnostic> {
    let Some(analysis) = analyze_document(uri("q.surql"), source, SymbolOrigin::Local) else {
        return Vec::new();
    };
    let mut workspace = WorkspaceIndex::default();
    workspace
        .documents
        .insert(uri("q.surql"), std::sync::Arc::new(analysis.clone()));
    let model = MergedSemanticModel::build(&workspace, &Default::default());
    let mut diagnostics = model.semantic_diagnostics(&analysis, &ServerSettings::default());
    diagnostics.extend(
        analysis
            .syntax_diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.code == Some(NumberOrString::String("unknown-type".to_string()))
            })
            .cloned(),
    );
    diagnostics
}

/// The type checks this crate owns: argument counts, argument types, declared
/// function return types, `LET` annotations, arithmetic operands, and type names.
///
/// `let-type` and `operator-type` are here because a false positive in either
/// would otherwise be structurally invisible to this sweep — the one test that
/// reads real-world SurrealQL at scale. `unknown-type` is here for the same
/// reason, and it needs the sweep more than most: it fires on a closed keyword
/// list, so a name SurrealDB adds in a later release shows up here first.
fn argument_diagnostics(source: &str) -> Vec<(String, String)> {
    diagnostics_for(source)
        .into_iter()
        .filter_map(|diagnostic| match &diagnostic.code {
            Some(NumberOrString::String(code))
                if code.starts_with("argument-")
                    || code == "return-type"
                    || code == "let-type"
                    || code == "operator-type"
                    || code == "unknown-method"
                    || code == "unknown-type"
                    || code == "field-type" =>
            {
                Some((code.clone(), diagnostic.message.clone()))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn valid_builtin_calls_stay_silent() {
    let fixture = include_str!("fixtures/builtin_calls_valid.surql");

    // One statement at a time, so a failure names the offending call rather
    // than the whole file.
    let mut offenders = Vec::new();
    for line in fixture.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("--") {
            continue;
        }
        for (code, message) in argument_diagnostics(line) {
            offenders.push(format!("{line}\n    → {code}: {message}"));
        }
    }

    assert!(
        offenders.is_empty(),
        "{} of SurrealDB's own valid calls were flagged:\n{}",
        offenders.len(),
        offenders.join("\n")
    );
}

#[test]
fn the_fixture_still_covers_the_breadth_it_was_built_for() {
    // Guards against the fixture being trimmed until it proves nothing.
    let fixture = include_str!("fixtures/builtin_calls_valid.surql");
    let calls = fixture
        .lines()
        .filter(|line| {
            let line = line.trim();
            !line.is_empty() && !line.starts_with("--")
        })
        .count();
    let namespaces: BTreeSet<&str> = fixture
        .lines()
        .filter_map(|line| line.trim().strip_prefix("-- "))
        .filter_map(|label| label.strip_suffix("::"))
        .collect();

    assert!(calls >= 120, "only {calls} calls left in the fixture");
    assert!(
        namespaces.len() >= 18,
        "only {} namespaces left: {namespaces:?}",
        namespaces.len()
    );
}

/// The SurrealDB corpus, when this machine has a checkout.
///
/// `SURREALDB_DIR` first, then the sibling layout the grammar already uses.
/// `None` in continuous integration, where the sweep skips.
fn corpus_dir() -> Option<PathBuf> {
    let candidates = [
        std::env::var_os("SURREALDB_DIR").map(PathBuf::from),
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../surrealdb")),
    ];
    candidates
        .into_iter()
        .flatten()
        .map(|path| path.join("language-tests/tests"))
        .find(|tests| tests.is_dir())
}

fn surql_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            surql_files(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("surql") {
            out.push(path);
        }
    }
}

// A `(file, code)` pair the sweep reports on code whose front matter does not
// declare a matching `error =`. Known false positives live in
// `known_false_positives.rs`. A fix removes the pair, and the sweep then fails
// until the entry is deleted. A new pair fails until it is fixed or entered
// there with its cause. A pair whose diagnostic matches a declared `error =`
// or `match` regex is expected, and must not appear there.
include!("known_false_positives.rs");

/// `error = "…"` strings in the leading `/** … */` block.
///
/// `error = true` and `error = false` declare that a call fails or succeeds
/// without saying the words, so they do not make a diagnostic expected. A
/// `match = "$error = /…/"` on the same result is the wording, and it counts.
/// `parsing-error` is a different key and is left alone.
fn declared_errors(source: &str) -> Vec<String> {
    let head = match source.split_once("*/") {
        Some((head, _)) if head.trim_start().starts_with("/**") => head,
        _ => return Vec::new(),
    };
    let lines: Vec<&str> = head.lines().collect();
    let mut errors = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if let Some(value) = assignment(lines[index], "match") {
            if let Some(text) = match_pattern(value) {
                errors.push(text);
            }
            index += 1;
            continue;
        }
        let Some(value) = assignment(lines[index], "error") else {
            index += 1;
            continue;
        };
        if value == "true" || value == "false" {
            index += 1;
            continue;
        }
        // `"""` is three quotes. A basic string would stop at the second.
        if let Some(rest) = value.strip_prefix("\"\"\"") {
            let (text, next) = multiline_string(&lines, index, rest);
            if !text.is_empty() {
                errors.push(text);
            }
            index = next;
            continue;
        }
        if let Some(text) = basic_string(value) {
            if !text.is_empty() {
                errors.push(text);
            }
            index += 1;
            continue;
        }
        index += 1;
    }
    errors
}

/// The value after `key =` on this line, when the key is exactly `key`.
fn assignment<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let trimmed = line.trim();
    if trimmed.starts_with('#') {
        return None;
    }
    let rest = trimmed.strip_prefix(key)?.trim_start();
    let rest = rest.strip_prefix('=')?.trim();
    if rest.is_empty() { None } else { Some(rest) }
}

/// The pattern inside `match = "$error = /…/"`.
fn match_pattern(value: &str) -> Option<String> {
    let text = basic_string(value)?;
    let rest = text.split_once("$error = /")?.1;
    let mut pattern = String::new();
    let mut chars = rest.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                pattern.push(next);
            }
            continue;
        }
        if ch == '/' {
            break;
        }
        pattern.push(ch);
    }
    if pattern.is_empty() {
        None
    } else {
        Some(pattern)
    }
}

fn basic_string(value: &str) -> Option<String> {
    let mut chars = value.chars();
    if chars.next() != Some('"') {
        return None;
    }
    let mut text = String::new();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => text.push('\n'),
                Some('t') => text.push('\t'),
                Some('r') => text.push('\r'),
                Some(other) => text.push(other),
                None => break,
            }
            continue;
        }
        if ch == '"' {
            return Some(text);
        }
        text.push(ch);
    }
    None
}

fn multiline_string(lines: &[&str], index: usize, first: &str) -> (String, usize) {
    if let Some(end) = first.find("\"\"\"") {
        return (first[..end].to_string(), index + 1);
    }
    let mut text = String::from(first);
    let mut cursor = index + 1;
    while cursor < lines.len() {
        if let Some(end) = lines[cursor].find("\"\"\"") {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&lines[cursor][..end]);
            return (text, cursor + 1);
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(lines[cursor]);
        cursor += 1;
    }
    (text, cursor)
}

fn normalize_wording(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '$' || ch == ':' || ch == '_' {
            out.push(ch.to_ascii_lowercase());
            space = false;
        } else if !space && !out.is_empty() {
            out.push(' ');
            space = true;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

fn has_token(text: &str, token: &str) -> bool {
    !token.is_empty() && text.split(' ').any(|word| word == token)
}

/// `phrase` as a whole run of words. `expected int` does not match
/// `expected interval`, and `2 arguments` does not match `12 arguments`.
fn has_phrase(text: &str, phrase: &str) -> bool {
    if phrase.is_empty() {
        return false;
    }
    let padded = format!(" {text} ");
    padded.contains(&format!(" {phrase} "))
}

fn backticked(message: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = message;
    while let Some(start) = rest.find('`') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('`') else {
            break;
        };
        out.push(&rest[..end]);
        rest = &rest[end + 1..];
    }
    out
}

/// True when `message` is the checker's paraphrase of `declared`.
///
/// The checker does not copy the engine's sentence. An argument-count
/// diagnostic says `` `array::add` expects 2 arguments, found 1. `` where the
/// file says `Incorrect arguments for function array::add(). Expected 2
/// arguments`. The match is the function and the expected arity or type, or
/// the same "cannot perform" sentence once quotes are ignored. A declaration
/// about a different failure does not match.
fn wording_matches(message: &str, declared: &str) -> bool {
    let message_n = normalize_wording(message);
    let declared_n = normalize_wording(declared);
    if message_n.is_empty() || declared_n.is_empty() {
        return false;
    }
    // A short declaration must not match by being a fragment of the
    // diagnostic. The sentences that are the same words are long.
    if message_n == declared_n
        || (declared_n.len() > 24
            && (declared_n.contains(&message_n) || message_n.contains(&declared_n)))
    {
        return true;
    }
    count_wording(message, &declared_n)
        || argument_type_wording(message, &declared_n)
        || declared_type_wording(message, &declared_n)
        || operator_wording(message, &declared_n)
        || unknown_method_wording(message, &declared_n)
        || unknown_wording(message, &declared_n)
}

fn unknown_method_wording(message: &str, declared: &str) -> bool {
    let Some(rest) = message.strip_prefix('`') else {
        return false;
    };
    let Some((_, rest)) = rest.split_once("` has no method `") else {
        return false;
    };
    let Some((method, _)) = rest.split_once('`') else {
        return false;
    };
    let method = normalize_wording(method);
    has_token(declared, &method)
        && (declared.contains("no such method") || declared.contains("not found"))
}

/// `array<int>` and `array` are the same refusal. The checker keeps the element
/// type; the engine's `error =` names the family.
fn operator_wording(message: &str, declared: &str) -> bool {
    let phrase = if let Some(rest) = message.strip_prefix("Cannot perform ") {
        let Some((verb, rest)) = rest.split_once(" with `") else {
            return false;
        };
        let Some((left, rest)) = rest.split_once("` and `") else {
            return false;
        };
        let Some((right, _)) = rest.split_once('`') else {
            return false;
        };
        format!(
            "cannot perform {} with {} and {}",
            normalize_wording(verb),
            base_type(left),
            base_type(right)
        )
    } else if let Some(rest) = message.strip_prefix("Cannot raise the value `") {
        let Some((left, rest)) = rest.split_once("` with `") else {
            return false;
        };
        let Some((right, _)) = rest.split_once('`') else {
            return false;
        };
        format!(
            "cannot raise the value {} with {}",
            base_type(left),
            base_type(right)
        )
    } else {
        return false;
    };
    has_phrase(declared, &phrase)
}

fn base_type(ty: &str) -> String {
    normalize_wording(ty.split('<').next().unwrap_or(ty))
}

fn count_wording(message: &str, declared: &str) -> bool {
    let Some(rest) = message.split_once(" expects ") else {
        return false;
    };
    let Some((expected, _)) = rest.1.split_once(", found ") else {
        return false;
    };
    let expected = normalize_wording(expected.trim_end_matches('.'));
    if !has_phrase(declared, &expected) {
        return false;
    }
    backticked(rest.0)
        .into_iter()
        .chain(backticked(rest.1))
        .map(normalize_wording)
        .any(|name| name.len() >= 3 && has_token(declared, &name))
}

fn argument_type_wording(message: &str, declared: &str) -> bool {
    // `Argument 2 of `.extend()` (`object::extend`) expects `object`, found …`
    // The closing backtick of the name is not always the last one before
    // "expects": a method names both the method and the function.
    let Some((head, rest)) = message.split_once(" expects `") else {
        return false;
    };
    let Some((expected, _)) = rest.split_once('`') else {
        return false;
    };
    let expected = normalize_wording(expected);
    if !has_phrase(declared, &format!("expected {expected}")) {
        return false;
    }
    backticked(head)
        .into_iter()
        .map(normalize_wording)
        .any(|name| {
            let bare = name.trim_matches('.').to_string();
            bare.len() >= 3 && (has_token(declared, &name) || has_token(declared, &bare))
        })
}

fn declared_type_wording(message: &str, declared: &str) -> bool {
    let Some(rest) = message.strip_prefix('`') else {
        return false;
    };
    let Some((name, rest)) = rest.split_once("` is declared `") else {
        return false;
    };
    let Some((ty, _)) = rest.split_once('`') else {
        return false;
    };
    let ty = normalize_wording(ty);
    if !has_phrase(declared, &format!("expected {ty}")) {
        return false;
    }
    let name = normalize_wording(name);
    name == "this field" || !name.starts_with('$') || has_token(declared, &name)
}

fn unknown_wording(message: &str, declared: &str) -> bool {
    let Some(name) = message.strip_prefix("Unknown type `") else {
        return false;
    };
    let Some((name, _)) = name.split_once('`') else {
        return false;
    };
    let name = normalize_wording(name);
    has_token(declared, &name)
        && (declared.contains("unknown")
            || declared.contains("unexpected")
            || declared.contains("kind name")
            || declared.contains("not a type"))
}

fn messages_match_declaration(messages: &[String], declared: &[String]) -> bool {
    !messages.is_empty()
        && messages
            .iter()
            .all(|message| declared.iter().any(|error| wording_matches(message, error)))
}

fn shorten(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= 160 {
        return flat;
    }
    flat.chars().take(160).collect::<String>() + "…"
}

/// The exhaustive oracle. Ignored by default because it re-analyses ~1,900
/// documents and takes about two minutes, which does not belong in a suite that
/// otherwise finishes in under a second.
///
/// Run it whenever the catalogue, the arity model, or an argument check changes:
///
/// ```bash
/// cargo test --test conformance -- --ignored --nocapture
/// ```
#[test]
#[ignore = "sweeps the whole SurrealDB corpus; ~2 minutes"]
fn the_surrealdb_corpus_produces_only_expected_diagnostics() {
    let Some(corpus) = corpus_dir() else {
        eprintln!("skipping: no SurrealDB checkout. Set SURREALDB_DIR to run this sweep.");
        return;
    };

    let mut files = Vec::new();
    surql_files(&corpus, &mut files);
    files.sort();
    assert!(
        files.len() > 1500,
        "expected the full corpus, found {} files",
        files.len()
    );

    let mut expected: BTreeSet<(String, String)> = BTreeSet::new();
    let mut false_positives: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut declarations: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for file in &files {
        let Ok(source) = std::fs::read_to_string(file) else {
            continue;
        };
        // The harness keeps its expectations in a leading TOML block comment.
        let body = match source.split_once("*/") {
            Some((head, tail)) if head.trim_start().starts_with("/**") => tail,
            _ => source.as_str(),
        };
        let relative = file
            .strip_prefix(&corpus)
            .unwrap_or(file)
            .to_string_lossy()
            .trim_start_matches('/')
            .to_string();
        let declared = declared_errors(&source);
        let mut by_code: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (code, message) in argument_diagnostics(body) {
            by_code.entry(code).or_default().push(message);
        }
        if by_code.is_empty() {
            continue;
        }
        declarations.insert(relative.clone(), declared.clone());
        for (code, messages) in by_code {
            if messages_match_declaration(&messages, &declared) {
                expected.insert((relative.clone(), code));
            } else {
                false_positives.insert((relative.clone(), code), messages);
            }
        }
    }

    let known: BTreeMap<(String, String), &str> = KNOWN_FALSE_POSITIVES
        .iter()
        .map(|(file, code, cause)| ((file.to_string(), code.to_string()), *cause))
        .collect();
    assert_eq!(
        known.len(),
        KNOWN_FALSE_POSITIVES.len(),
        "a false positive is listed twice"
    );

    let mut extra = Vec::new();
    for ((file, code), messages) in &false_positives {
        if known.contains_key(&(file.clone(), code.clone())) {
            continue;
        }
        let declared = declarations.get(file).map(Vec::as_slice).unwrap_or(&[]);
        let declared = if declared.is_empty() {
            "(no error = wording)".to_string()
        } else {
            declared
                .iter()
                .map(|error| shorten(error))
                .collect::<Vec<_>>()
                .join(" | ")
        };
        for message in messages {
            extra.push(format!(
                "{file} :: {code} :: {}\n  declared: {declared}",
                shorten(message)
            ));
        }
    }
    let mut stale = Vec::new();
    for (pair, cause) in &known {
        if false_positives.contains_key(pair) {
            continue;
        }
        let why = if expected.contains(pair) {
            "it now matches a declared error"
        } else {
            "the sweep no longer reports it"
        };
        stale.push(format!("{} :: {} — {why}. {cause}", pair.0, pair.1));
    }

    if !extra.is_empty() || !stale.is_empty() {
        let report = format!(
            "expected {} (wording matched a declared error)\n\
             known false positives {}\n\
             unlisted false positives {}:\n{}\n\
             stale known entries {}:\n{}",
            expected.len(),
            known.len(),
            extra.len(),
            extra.join("\n"),
            stale.len(),
            stale.join("\n")
        );
        let _ = std::fs::write("/tmp/conformance-sweep.txt", &report);
        panic!("{report}");
    }

    eprintln!(
        "expected {} pair(s); known false positives {}:",
        expected.len(),
        known.len()
    );
    for ((file, code), cause) in &known {
        eprintln!("  {file} :: {code} — {cause}");
    }
}

#[test]
fn an_error_declaration_matches_the_checkers_paraphrase() {
    let count = "`array::add` expects 2 arguments, found 1.";
    let declared_count = "Incorrect arguments for function array::add(). Expected 2 arguments";
    assert!(wording_matches(count, declared_count));

    let typed = "Argument 1 of `array::add` expects `array`, found `int`.";
    let declared_typed = "Incorrect arguments for function array::add(). Argument 1 was the \
         wrong type. Expected `array` but found `3`";
    assert!(wording_matches(typed, declared_typed));

    let letter = "`$bar` is declared `int` but the value is `string`.";
    let declared_letter =
        "Tried to set `$bar`, but couldn't coerce value: Expected `int` but found `'hello'`";
    assert!(wording_matches(letter, declared_letter));

    let operator = "Cannot perform addition with `string` and `int`.";
    assert!(wording_matches(
        operator,
        "Cannot perform addition with 'string' and 'int'"
    ));
    assert!(wording_matches(
        "Cannot perform addition with `array<int>` and `int`.",
        "Cannot perform addition with 'array' and 'int'"
    ));
    assert!(wording_matches(
        "Cannot raise the value `array<int>` with `int`.",
        "Cannot raise the value 'array' with 'int'"
    ));
    let source =
        "/**\nmatch = \"$error = /Expected `object` but found `'9'`/\"\nerror = true\n*/\n";
    assert_eq!(
        declared_errors(source),
        vec!["Expected `object` but found `'9'`".to_string()]
    );
}

#[test]
fn explain_is_text_so_contains_is_the_string_method() {
    let source = "(EXPLAIN SELECT VALUE id FROM child ORDER BY t).contains('Index');";
    let hits: Vec<_> = argument_diagnostics(source)
        .into_iter()
        .filter(|(code, _)| code == "unknown-method")
        .collect();
    assert!(hits.is_empty(), "{hits:?}");
}

#[test]
fn a_declaration_about_something_else_is_not_a_match() {
    let operator = "Cannot perform addition with `int` and `string`.";
    assert!(!wording_matches(operator, "The table 't' does not exist"));
    assert!(!wording_matches(
        "Unknown type `date`.",
        "The table 't' does not exist"
    ));
    assert!(wording_matches(
        "`string` has no method `filter`.",
        "There was a problem running the filter() function. no such method found for the string type"
    ));
    let other_let = "`$foo` is declared `int` but the value is `string`.";
    assert!(!wording_matches(
        other_let,
        "Tried to set `$bar`, but couldn't coerce value: Expected `int` but found `'hello'`"
    ));
}

#[test]
fn error_true_declares_no_wording() {
    let source = "/**\n[[test.results]]\nerror = true\n*/\n1 + \"1\";\n";
    assert!(declared_errors(source).is_empty());
    let source = "/**\n[[test.results]]\nerror = false\n*/\nRETURN 1;\n";
    assert!(declared_errors(source).is_empty());
}

#[test]
fn a_multiline_error_keeps_its_wording() {
    let source =
        "/**\nerror = \"\"\"\nCannot perform addition with 'int' and 'string'\n\"\"\"\n*/\n";
    let errors = declared_errors(source);
    assert_eq!(errors.len(), 1);
    assert!(wording_matches(
        "Cannot perform addition with `int` and `string`.",
        &errors[0]
    ));
}
