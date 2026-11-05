//! The receiver tables that decide which function a `value.method()` call runs.
//!
//! SurrealQL lets most builtins be called as a method, and the mapping from
//! method to function is **not** `<receiver type>::<method>`. `(5).round()` is
//! `math::round`, `123.to_float()` is `type::float`, `"abc".is_alphanum()` is
//! `string::is::alphanum`. A language server that guesses the convention gets
//! roughly half of them.
//!
//! The whole mapping lives in one engine function — `fnc::idiom` in
//! `fnc/mod.rs` — as a `match` on the receiver's `Value` variant with one
//! `dispatch!` invocation per arm. Eleven typed arms plus a catch-all, and about
//! 820 arms in total.
//!
//! # Why this is a separate scraper
//!
//! [`crate::engine_tables::parse_dispatch`] already reads `fnc/mod.rs` line by
//! line, and the idiom arms stream straight past it — its
//! `!name.contains("::")` guard drops every one, because a method name is bare
//! (`len`, `round`). Its flat `BTreeMap<String, String>` would also collide
//! `len` five ways: `Set`, `Array`, `Bytes`, `Object` and `String` each define
//! one, and they are five different implementations.
//!
//! # Why this reads tokens rather than lines
//!
//! The receiver grouping is a plain `match` **outside** the macro, so `syn`
//! hands the eleven keys over as typed patterns and there is no dependence on
//! the engine's indentation. Inside the macro the arm grammar is regular — this
//! is the matcher at `fnc/mod.rs`, and it is exactly what [`parse_arms`] walks:
//!
//! ```text
//! [ exp(TARGET) ] "name" => [ (wrapper) ]* ident (:: ident)* [ (arg) ]* [ .await ]* ,
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use proc_macro2::{TokenStream, TokenTree};
use syn::{Expr, Item, Pat, Stmt};

use crate::engine_tables::strip_raw;

/// One method, and the function it dispatches to.
#[derive(Debug, Clone)]
pub struct MethodArm {
    /// The name written after the dot.
    pub method: String,
    /// The implementation's Rust module path, such as `string::is::alphanum`.
    /// This is the key [`crate::signatures::Implementation`] uses.
    pub path: String,
    /// The experimental target this method sits behind, when it has one.
    pub experimental: Option<String>,
}

/// The methods available on one receiver.
#[derive(Debug, Clone)]
pub struct Receiver {
    /// The engine `Value` variant, or an empty string for the catch-all arm
    /// that serves `bool`, `uuid`, `regex`, `range`, `none` and the rest.
    pub kind: String,
    pub methods: Vec<MethodArm>,
}

/// Read every receiver table.
///
/// SurrealDB 3 keeps them in `fn idiom` inside `fnc/mod.rs`. SurrealDB 4
/// registers them from `exec/function/method.rs` instead.
pub fn parse(fnc_mod_rs: &Path) -> Result<Vec<Receiver>, String> {
    let source = std::fs::read_to_string(fnc_mod_rs)
        .map_err(|error| format!("cannot read {}: {error}", fnc_mod_rs.display()))?;
    if !source.contains("fn idiom") {
        let method_rs = fnc_mod_rs
            .parent()
            .and_then(|fnc| fnc.parent())
            .map(|src| src.join("exec/function/method.rs"))
            .ok_or_else(|| format!("no method registry beside {}", fnc_mod_rs.display()))?;
        return parse_registry(&method_rs);
    }
    parse_idiom(fnc_mod_rs)
}

fn parse_idiom(fnc_mod_rs: &Path) -> Result<Vec<Receiver>, String> {
    let source = std::fs::read_to_string(fnc_mod_rs)
        .map_err(|error| format!("cannot read {}: {error}", fnc_mod_rs.display()))?;
    let file = syn::parse_file(&source)
        .map_err(|error| format!("cannot parse {}: {error}", fnc_mod_rs.display()))?;

    let idiom = file
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(function) if function.sig.ident == "idiom" => Some(function),
            _ => None,
        })
        .ok_or_else(|| {
            format!(
                "no `fn idiom` in {} — the engine's method dispatch moved",
                fnc_mod_rs.display()
            )
        })?;

    // The body is a guard statement followed by the `match value { … }`.
    let dispatch_match = idiom
        .block
        .stmts
        .iter()
        .find_map(|statement| match statement {
            Stmt::Expr(Expr::Match(expression), _) => Some(expression),
            _ => None,
        })
        .ok_or_else(|| "`fn idiom` has no `match` over the receiver".to_string())?;

    let mut receivers = Vec::new();
    for arm in &dispatch_match.arms {
        let kind = receiver_kind(&arm.pat);
        let Some(tokens) = arm_macro_tokens(&arm.body) else {
            continue;
        };
        // Every arm must splice the receiver in at position zero. The whole
        // argument-shift model downstream rests on it, so a change here has to
        // break the build rather than silently shift every check by one.
        if !splices_receiver_first(&arm.body) {
            return Err(format!(
                "receiver arm `{kind}` does not call `args.insert(0, …)` — the \
                 receiver may no longer be argument zero"
            ));
        }
        receivers.push(Receiver {
            kind,
            methods: parse_arms(tokens),
        });
    }

    if receivers.len() < 11 {
        return Err(format!(
            "found only {} receiver tables, expected at least 11",
            receivers.len()
        ));
    }
    let arms: usize = receivers.iter().map(|entry| entry.methods.len()).sum();
    if arms < 700 {
        return Err(format!(
            "read only {arms} method arms, expected about 820 — the engine's \
             dispatch layout probably changed, so the tables are not trustworthy"
        ));
    }

    Ok(receivers)
}

/// `build_method_registry` in SurrealDB 4: one call per method arm.
fn parse_registry(method_rs: &Path) -> Result<Vec<Receiver>, String> {
    let receivers = read_method_registry(method_rs)?;
    if receivers.len() < 11 {
        return Err(format!(
            "found only {} receiver tables, expected at least 11",
            receivers.len()
        ));
    }
    let arms: usize = receivers.iter().map(|entry| entry.methods.len()).sum();
    // SurrealDB 4's registry is one call per arm, about 370, rather than the
    // old idiom match that repeated a method on every receiver.
    if arms < 300 {
        return Err(format!(
            "read only {arms} method arms from {} — the registry calls moved",
            method_rs.display()
        ));
    }
    Ok(receivers)
}

fn read_method_registry(method_rs: &Path) -> Result<Vec<Receiver>, String> {
    let source = std::fs::read_to_string(method_rs)
        .map_err(|error| format!("cannot read {}: {error}", method_rs.display()))?;
    let file = syn::parse_file(&source)
        .map_err(|error| format!("cannot parse {}: {error}", method_rs.display()))?;
    let function = file
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(function) if function.sig.ident == "build_method_registry" => Some(function),
            _ => None,
        })
        .ok_or_else(|| format!("no `build_method_registry` in {}", method_rs.display()))?;

    let mut by_kind: BTreeMap<String, Vec<MethodArm>> = BTreeMap::new();
    let mut aliases = Vec::new();
    collect_registrations(&function.block.stmts, &mut by_kind, &mut aliases);
    for (alias, target) in aliases {
        let copied: Vec<(String, MethodArm)> = by_kind
            .iter()
            .flat_map(|(kind, arms)| {
                arms.iter().filter(|arm| arm.method == target).map(|arm| {
                    (
                        kind.clone(),
                        MethodArm {
                            method: alias.clone(),
                            path: arm.path.clone(),
                            experimental: arm.experimental.clone(),
                        },
                    )
                })
            })
            .collect();
        for (kind, arm) in copied {
            by_kind.entry(kind).or_default().push(arm);
        }
    }
    Ok(by_kind
        .into_iter()
        .map(|(kind, methods)| Receiver { kind, methods })
        .collect())
}

fn collect_registrations(
    statements: &[syn::Stmt],
    by_kind: &mut BTreeMap<String, Vec<MethodArm>>,
    aliases: &mut Vec<(String, String)>,
) {
    for statement in statements {
        let expression = match statement {
            syn::Stmt::Expr(expression, _) => expression,
            syn::Stmt::Local(local) => {
                if let Some(init) = &local.init {
                    walk_registration(&init.expr, by_kind, aliases);
                }
                continue;
            }
            syn::Stmt::Item(_) | syn::Stmt::Macro(_) => continue,
        };
        walk_registration(expression, by_kind, aliases);
    }
}

fn walk_registration(
    expression: &Expr,
    by_kind: &mut BTreeMap<String, Vec<MethodArm>>,
    aliases: &mut Vec<(String, String)>,
) {
    match expression {
        Expr::MethodCall(call) => {
            record_registration(&call.method.to_string(), &call.args, by_kind, aliases);
            for argument in &call.args {
                walk_registration(argument, by_kind, aliases);
            }
        }
        Expr::Block(block) => collect_registrations(&block.block.stmts, by_kind, aliases),
        Expr::If(item) => {
            collect_registrations(&item.then_branch.stmts, by_kind, aliases);
            if let Some((_, branch)) = &item.else_branch {
                walk_registration(branch, by_kind, aliases);
            }
        }
        _ => {}
    }
}

fn record_registration(
    name: &str,
    args: &syn::punctuated::Punctuated<Expr, syn::Token![,]>,
    by_kind: &mut BTreeMap<String, Vec<MethodArm>>,
    aliases: &mut Vec<(String, String)>,
) {
    let arguments: Vec<&Expr> = args.iter().collect();
    match name {
        "register_generic" => {
            let (Some(method), Some(path)) = (
                lit_str(arguments.first().copied()),
                get_path(arguments.get(1).copied()),
            ) else {
                return;
            };
            by_kind.entry(String::new()).or_default().push(MethodArm {
                method,
                path,
                experimental: None,
            });
        }
        "register_typed" => {
            let (Some(method), Some(kind), Some(path)) = (
                lit_str(arguments.first().copied()),
                kind_name(arguments.get(1).copied()),
                get_path(arguments.get(2).copied()),
            ) else {
                return;
            };
            by_kind.entry(kind).or_default().push(MethodArm {
                method,
                path,
                experimental: None,
            });
        }
        "register_alias" => {
            let (Some(alias), Some(target)) = (
                lit_str(arguments.first().copied()),
                lit_str(arguments.get(1).copied()),
            ) else {
                return;
            };
            aliases.push((alias, target));
        }
        _ => {}
    }
}

fn lit_str(expression: Option<&Expr>) -> Option<String> {
    match expression? {
        Expr::Lit(literal) => match &literal.lit {
            syn::Lit::Str(value) => Some(value.value()),
            _ => None,
        },
        _ => None,
    }
}

fn kind_name(expression: Option<&Expr>) -> Option<String> {
    let Expr::Path(path) = expression? else {
        return None;
    };
    let name = path.path.segments.last()?.ident.to_string();
    // The language server's receiver table still calls a record `RecordId`,
    // which is the `Value` variant. The registry's kind is `Record`.
    Some(match name.as_str() {
        "Record" => "RecordId".to_string(),
        other => other.to_string(),
    })
}

fn get_path(expression: Option<&Expr>) -> Option<String> {
    let Expr::Call(call) = expression? else {
        return None;
    };
    lit_str(call.args.iter().nth(1))
}

/// `Value::String(s)` gives `String`. A bare binding is the catch-all.
fn receiver_kind(pattern: &Pat) -> String {
    let path = match pattern {
        Pat::TupleStruct(tuple) => &tuple.path,
        Pat::Path(bare) => &bare.path,
        // `x => { … }`, the arm that serves every remaining variant.
        _ => return String::new(),
    };
    path.segments
        .last()
        .map(|segment| segment.ident.to_string())
        .unwrap_or_default()
}

/// The `dispatch!(…)` token stream inside an arm body.
fn arm_macro_tokens(body: &Expr) -> Option<TokenStream> {
    let statements = match body {
        Expr::Block(block) => &block.block.stmts,
        _ => return None,
    };
    statements.iter().find_map(|statement| {
        let expression = match statement {
            Stmt::Expr(expression, _) => expression,
            _ => return None,
        };
        match expression {
            Expr::Macro(macro_expression) => Some(macro_expression.mac.tokens.clone()),
            _ => None,
        }
    })
}

/// True when the arm body contains `args.insert(0, …)`.
fn splices_receiver_first(body: &Expr) -> bool {
    let Expr::Block(block) = body else {
        return false;
    };
    let rendered = quote::quote! { #block }.to_string();
    // `args . insert (0usize , …)` after tokenisation; match loosely on purpose,
    // since only the literal zero matters.
    rendered.contains("args . insert (0")
}

/// Walk the dispatch arms of one table.
///
/// The four fixed leading arguments (`ctx`, `name`, `args`, the message) are
/// skipped structurally rather than by counting: an arm is recognised only by a
/// string literal *followed by* `=>`, and none of the four is.
fn parse_arms(tokens: TokenStream) -> Vec<MethodArm> {
    let tokens: Vec<TokenTree> = tokens.into_iter().collect();
    let mut arms = Vec::new();
    let mut experimental: Option<String> = None;
    let mut index = 0;

    while index < tokens.len() {
        // `exp(Files)` marks a method behind an experimental target.
        if let TokenTree::Ident(identifier) = &tokens[index]
            && identifier == "exp"
            && let Some(TokenTree::Group(group)) = tokens.get(index + 1)
        {
            experimental = Some(group.stream().to_string().trim().to_string());
            index += 2;
            continue;
        }

        let TokenTree::Literal(literal) = &tokens[index] else {
            index += 1;
            continue;
        };
        if !is_fat_arrow(&tokens, index + 1) {
            index += 1;
            continue;
        }

        let method = literal.to_string().trim_matches('"').to_string();
        match read_path(&tokens, index + 3) {
            Some((path, next)) => {
                arms.push(MethodArm {
                    method,
                    path,
                    experimental: experimental.take(),
                });
                index = next;
            }
            None => {
                experimental = None;
                index += 3;
            }
        }
    }

    arms
}

fn is_fat_arrow(tokens: &[TokenTree], at: usize) -> bool {
    matches!(tokens.get(at), Some(TokenTree::Punct(punct)) if punct.as_char() == '=')
        && matches!(tokens.get(at + 1), Some(TokenTree::Punct(punct)) if punct.as_char() == '>')
}

/// `ident (:: ident)*` starting at `at`, and the index just past it.
///
/// Stops at the first token that is not a `::`-joined identifier, which is what
/// leaves the `(…)` context argument and the `.await` suffix alone.
fn read_path(tokens: &[TokenTree], at: usize) -> Option<(String, usize)> {
    let TokenTree::Ident(first) = tokens.get(at)? else {
        return None;
    };
    let mut segments = vec![strip_raw(&first.to_string()).to_string()];
    let mut index = at + 1;

    loop {
        let joined = matches!(tokens.get(index), Some(TokenTree::Punct(punct)) if punct.as_char() == ':')
            && matches!(tokens.get(index + 1), Some(TokenTree::Punct(punct)) if punct.as_char() == ':');
        if !joined {
            break;
        }
        let Some(TokenTree::Ident(segment)) = tokens.get(index + 2) else {
            break;
        };
        segments.push(strip_raw(&segment.to_string()).to_string());
        index += 3;
    }

    Some((segments.join("::"), index))
}

/// Pair each method with the SurrealQL name authors would write for the same
/// implementation.
///
/// `dispatch` maps a SurrealQL name to a Rust path, so this inverts it. The
/// inverse is not one-to-one — `array::all`, `array::every`, `array::some` and
/// `array::includes` share two implementations between them — but every
/// candidate necessarily has the *same parameters*, because they are the same
/// function. So the choice only affects the name shown in hover, and taking the
/// first in sorted order keeps it deterministic.
///
/// A path with no entry at all is a method-only function. `value::chain` is the
/// single case: it is registered in the parser's `PATHS` but has no callable
/// dispatch arm, so `value::chain(x, f)` parses and then fails while
/// `x.chain(f)` works.
pub fn surrealql_names(dispatch: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut by_path: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, path) in dispatch {
        by_path.entry(path.clone()).or_default().push(name.clone());
    }
    by_path
        .into_iter()
        .filter_map(|(path, mut names)| {
            names.sort();
            let chosen = choose_name(&path, &names)?;
            Some((path, chosen))
        })
        .collect()
}

/// Pick the name to show for an implementation with several spellings.
///
/// Prefer the spelling that *is* the implementation's path. `record::tb` and
/// `meta::tb` both run `record::tb`, and `meta::` is the legacy spelling the
/// engine kept for compatibility — sorting alphabetically would show the old
/// name. Preferring the exact match also picks `array::all` over its alias
/// `array::every`, which names the implementation rather than a synonym.
fn choose_name(path: &str, names: &[String]) -> Option<String> {
    if names.iter().any(|name| name == path) {
        return Some(path.to_string());
    }
    // Otherwise prefer a spelling from the module the implementation lives in.
    let namespace = path.split("::").next().unwrap_or_default();
    names
        .iter()
        .find(|name| name.starts_with(&format!("{namespace}::")))
        .or_else(|| names.first())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registry_call_becomes_a_method_arm() {
        let dir = std::env::temp_dir().join("xtask-method-registry");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("method.rs");
        std::fs::write(
            &file,
            r#"
            fn build_method_registry() {
                m.register_generic("len", get(funcs, "string::len"));
                m.register_typed("len", ValueKind::Array, get(funcs, "array::len"));
                m.register_alias("size", "len");
            }
            "#,
        )
        .unwrap();
        let receivers = read_method_registry(&file).unwrap();
        let generic = receivers
            .iter()
            .find(|entry| entry.kind.is_empty())
            .unwrap();
        assert!(
            generic
                .methods
                .iter()
                .any(|arm| arm.method == "len" && arm.path == "string::len")
        );
        assert!(
            generic
                .methods
                .iter()
                .any(|arm| arm.method == "size" && arm.path == "string::len")
        );
        let array = receivers
            .iter()
            .find(|entry| entry.kind == "Array")
            .unwrap();
        assert!(
            array
                .methods
                .iter()
                .any(|arm| arm.method == "size" && arm.path == "array::len")
        );
    }
}
