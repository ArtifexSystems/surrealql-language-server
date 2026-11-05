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
//!   ~1,900 files of `language-tests/` and asserts the *exact* set of
//!   diagnostics, so a new false positive fails the build rather than hiding in
//!   a count. It skips without a checkout.
//!
//! To refresh the fixture, extract one call per distinct function from files
//! that declare no expected error:
//!
//! ```text
//! cd $SURREALDB/language-tests/tests
//! # keep lines ending in `;` that call a namespaced builtin, from files with
//! # no `error =` / `parsing-error`, skipping clause fragments and `api::`
//! ```

use std::collections::BTreeSet;
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

/// Every `(file, code)` pair the sweep is allowed to report on the SurrealDB
/// 4.0.0 language tests.
///
/// The set is what this catalogue and the pinned grammar report there. It
/// includes calls the engine rejects, and types the pinned grammar does not
/// know yet (`date`, `time`, `timestamp`, `interval`, `enum`). A new pair
/// fails the sweep instead of blending into a count. A pair that disappears
/// fails it too.
const EXPECTED: &[(&str, &str)] = &[
    (
        "api/errors/caught_grouped_query_failure_leaves_no_writes.surql",
        "operator-type",
    ),
    ("bench/executor/rt_import.surql", "argument-type"),
    ("bench/executor/rt_import.surql", "operator-type"),
    (
        "bench/timeseries/define_id_components_scan.surql",
        "unknown-type",
    ),
    ("bench/util/readings-keyed-empty.surql", "unknown-type"),
    ("graphql/_temporal_schema.surql", "unknown-type"),
    ("graphql/id_components_default_import.surql", "unknown-type"),
    ("graphql/id_components_import.surql", "unknown-type"),
    ("language/closure/basic.surql", "argument-type"),
    (
        "language/closure/filter_and_map_surface_forms.surql",
        "argument-type",
    ),
    (
        "language/closure/filter_and_map_surface_forms.surql",
        "unknown-method",
    ),
    ("language/coerce/regex.surql", "argument-type"),
    (
        "language/control_flow/dependence/a_failing_write_is_reported_before_a_later_throw.surql",
        "let-type",
    ),
    ("language/enums/aggregates.surql", "unknown-type"),
    ("language/enums/fields_and_ordering.surql", "unknown-type"),
    ("language/enums/ordering_scopes.surql", "unknown-type"),
    ("language/enums/range_membership.surql", "unknown-type"),
    (
        "language/enums/rename_value_maintenance.surql",
        "unknown-type",
    ),
    (
        "language/enums/rename_value_optional_elements.surql",
        "unknown-type",
    ),
    ("language/enums/rename_value_rewrites.surql", "unknown-type"),
    ("language/enums/two_enums_compared.surql", "unknown-type"),
    ("language/enums/typed_positions.surql", "unknown-type"),
    (
        "language/expression/a_false_left_gate_never_runs_its_right_operand.surql",
        "argument-type",
    ),
    (
        "language/expression/operators/operand_semantics.surql",
        "operator-type",
    ),
    (
        "language/expression/shared_subexpression_fallible.surql",
        "operator-type",
    ),
    ("language/functions/array/add.surql", "argument-count"),
    ("language/functions/array/add.surql", "argument-type"),
    ("language/functions/array/all.surql", "argument-type"),
    ("language/functions/array/any.surql", "argument-type"),
    ("language/functions/array/append.surql", "argument-type"),
    ("language/functions/array/at.surql", "argument-type"),
    ("language/functions/array/combine.surql", "argument-type"),
    (
        "language/functions/array/complement.surql",
        "argument-count",
    ),
    ("language/functions/array/complement.surql", "argument-type"),
    ("language/functions/array/concat.surql", "argument-type"),
    (
        "language/functions/array/difference.surql",
        "argument-count",
    ),
    ("language/functions/array/difference.surql", "argument-type"),
    ("language/functions/array/distinct.surql", "argument-type"),
    ("language/functions/array/every.surql", "argument-type"),
    ("language/functions/array/first.surql", "argument-type"),
    ("language/functions/array/flatten.surql", "argument-type"),
    ("language/functions/array/group.surql", "argument-type"),
    ("language/functions/array/includes.surql", "argument-type"),
    ("language/functions/array/index_of.surql", "argument-count"),
    ("language/functions/array/insert.surql", "argument-count"),
    ("language/functions/array/intersect.surql", "argument-count"),
    ("language/functions/array/intersect.surql", "argument-type"),
    ("language/functions/array/is_empty.surql", "argument-type"),
    ("language/functions/array/join.surql", "argument-count"),
    ("language/functions/array/len.surql", "argument-type"),
    ("language/functions/array/max.surql", "argument-type"),
    ("language/functions/array/min.surql", "argument-type"),
    ("language/functions/array/prepend.surql", "argument-type"),
    ("language/functions/array/push.surql", "argument-type"),
    ("language/functions/array/reverse.surql", "argument-type"),
    ("language/functions/array/shuffle.surql", "argument-type"),
    ("language/functions/array/slice.surql", "argument-type"),
    ("language/functions/array/some.surql", "argument-type"),
    ("language/functions/array/sort.surql", "argument-type"),
    ("language/functions/array/sort_asc.surql", "argument-count"),
    ("language/functions/array/sort_desc.surql", "argument-count"),
    ("language/functions/array/union.surql", "argument-type"),
    ("language/functions/bytes/len.surql", "argument-type"),
    ("language/functions/math/bottom.surql", "argument-type"),
    ("language/functions/math/top.surql", "argument-type"),
    ("language/functions/meta/id.surql", "argument-type"),
    ("language/functions/meta/tb.surql", "argument-type"),
    ("language/functions/object/entries.surql", "argument-type"),
    ("language/functions/object/extend.surql", "argument-type"),
    (
        "language/functions/object/extend_function.surql",
        "argument-type",
    ),
    ("language/functions/parse/url/domain.surql", "argument-type"),
    ("language/functions/parse/url/host.surql", "argument-type"),
    ("language/functions/parse/url/path.surql", "argument-type"),
    ("language/functions/parse/url/scheme.surql", "argument-type"),
    ("language/functions/set/add.surql", "argument-type"),
    ("language/functions/set/all.surql", "argument-type"),
    ("language/functions/set/any.surql", "argument-type"),
    ("language/functions/set/complement.surql", "argument-type"),
    (
        "language/functions/set/complex_values.surql",
        "argument-type",
    ),
    ("language/functions/set/contains.surql", "argument-type"),
    ("language/functions/set/difference.surql", "argument-type"),
    ("language/functions/set/intersect.surql", "argument-type"),
    ("language/functions/set/is_empty.surql", "argument-type"),
    ("language/functions/set/len.surql", "argument-type"),
    ("language/functions/set/remove.surql", "argument-type"),
    ("language/functions/set/union.surql", "argument-type"),
    (
        "language/functions/signature_parity.surql",
        "argument-count",
    ),
    (
        "language/functions/string/distance/damerau_levenshtein.surql",
        "argument-type",
    ),
    (
        "language/functions/string/distance/levenshtein.surql",
        "argument-type",
    ),
    ("language/functions/string/reverse.surql", "argument-type"),
    (
        "language/functions/string/similarity/fuzzy.surql",
        "argument-type",
    ),
    (
        "language/functions/string/similarity/smithwaterman.surql",
        "argument-type",
    ),
    ("language/functions/time/set.surql", "argument-type"),
    (
        "language/functions/udf_argument_read_in_predicate.surql",
        "argument-type",
    ),
    ("language/functions/value/chain.surql", "argument-type"),
    (
        "language/functions/value/expect_function_syntax.surql",
        "argument-count",
    ),
    (
        "language/functions/value/expect_function_syntax.surql",
        "argument-type",
    ),
    (
        "language/functions/value/expect_method_arity.surql",
        "argument-count",
    ),
    (
        "language/functions/value/expect_method_errors.surql",
        "argument-type",
    ),
    ("language/functions/vector/add.surql", "argument-type"),
    (
        "language/functions/vector/distance_minkowski.surql",
        "argument-count",
    ),
    (
        "language/functions/vector/distance_minkowski.surql",
        "argument-type",
    ),
    ("language/functions/vector/scale.surql", "argument-type"),
    (
        "language/indexes/spatial/spatial_lifecycle_and_edge_cases_differential.surql",
        "argument-type",
    ),
    ("language/ir/and_or_return_operands.surql", "operator-type"),
    ("language/ir/computed_field_reads.surql", "operator-type"),
    (
        "language/ir/dispatch_before_arguments.surql",
        "argument-count",
    ),
    (
        "language/ir/dispatch_before_arguments.surql",
        "argument-type",
    ),
    (
        "language/ir/dispatch_before_arguments.surql",
        "operator-type",
    ),
    (
        "language/ir/dispatch_before_arguments.surql",
        "unknown-method",
    ),
    (
        "language/ir/limit_start_and_only_errors.surql",
        "operator-type",
    ),
    (
        "language/ir/two_raising_subexpressions_report_the_first.surql",
        "argument-count",
    ),
    (
        "language/ir/two_raising_subexpressions_report_the_first.surql",
        "operator-type",
    ),
    ("language/mock/outside_from.surql", "argument-type"),
    (
        "language/planner/explain_matrix/index_on_id_component_differential.surql",
        "unknown-type",
    ),
    (
        "language/planner/id_components_datetime_bound_without_widening.surql",
        "unknown-type",
    ),
    (
        "language/planner/id_components_datetime_subsecond.surql",
        "unknown-type",
    ),
    (
        "language/planner/id_components_differential_permissions_import.surql",
        "unknown-type",
    ),
    (
        "language/planner/id_components_differential_reads.surql",
        "unknown-type",
    ),
    (
        "language/planner/id_components_differential_reads_plan.surql",
        "unknown-type",
    ),
    (
        "language/planner/id_components_index_import.surql",
        "unknown-type",
    ),
    (
        "language/planner/id_components_int_index_mixed_number_bounds.surql",
        "unknown-type",
    ),
    (
        "language/planner/id_components_key_range_import.surql",
        "unknown-type",
    ),
    (
        "language/planner/id_components_layer_elision.surql",
        "unknown-type",
    ),
    (
        "language/planner/id_components_mutation_where.surql",
        "unknown-type",
    ),
    (
        "language/planner/id_components_order_limit.surql",
        "unknown-type",
    ),
    (
        "language/planner/id_components_permission_bounds_import.surql",
        "unknown-type",
    ),
    (
        "language/planner/id_components_raising_bound_matches_index_semantics.surql",
        "argument-type",
    ),
    (
        "language/planner/id_components_raising_bound_matches_index_semantics.surql",
        "unknown-type",
    ),
    (
        "language/planner/id_components_record_range_vs_where.surql",
        "unknown-type",
    ),
    (
        "language/planner/id_components_version.surql",
        "unknown-type",
    ),
    (
        "language/planner/id_components_version_reads_keys_as_written.surql",
        "unknown-type",
    ),
    (
        "language/primitive/array/arithmic_operations.surql",
        "operator-type",
    ),
    ("language/primitive/bigdecimal/casts.surql", "unknown-type"),
    (
        "language/primitive/bigdecimal/define_field.surql",
        "unknown-type",
    ),
    (
        "language/primitive/duration/arithmatic_operations.surql",
        "operator-type",
    ),
    (
        "language/primitive/number_kinds/bigdecimal_indexes.surql",
        "unknown-type",
    ),
    (
        "language/primitive/number_kinds/bigdecimal_packed.surql",
        "unknown-type",
    ),
    (
        "language/primitive/number_kinds/casts.surql",
        "unknown-type",
    ),
    (
        "language/primitive/number_kinds/coercion.surql",
        "unknown-type",
    ),
    (
        "language/primitive/number_kinds/ddl_and_packed.surql",
        "unknown-type",
    ),
    (
        "language/primitive/number_kinds/literals.surql",
        "unknown-type",
    ),
    (
        "language/primitive/set/set_array_common_behaviour.surql",
        "operator-type",
    ),
    (
        "language/primitive/temporal_kinds/casts.surql",
        "unknown-type",
    ),
    (
        "language/primitive/temporal_kinds/coercion.surql",
        "unknown-type",
    ),
    (
        "language/primitive/temporal_kinds/define_field.surql",
        "unknown-type",
    ),
    (
        "language/primitive/temporal_kinds/index_scans.surql",
        "unknown-type",
    ),
    (
        "language/primitive/temporal_kinds/indexes.surql",
        "unknown-type",
    ),
    (
        "language/script/division_and_regex_after_braces.surql",
        "unknown-method",
    ),
    (
        "language/statements/alter/alter_field_id_components.surql",
        "unknown-type",
    ),
    (
        "language/statements/create/id_components.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/default_value_does_not_match_type.surql",
        "field-type",
    ),
    (
        "language/statements/define/field/id_components_clash_existing_index_on_body_field.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_clash_existing_view_on_body_field.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_clash_field_then_id.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_clash_id_then_field.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_coarse_names_outlive_the_records.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_define_then_clash_same_tx.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_id_clauses_read_dropped_name.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_id_clauses_read_unresolved.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_if_not_exists_id_reads_no_record.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_kinds_refused_any.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_kinds_refused_bytes.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_kinds_refused_decimal.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_kinds_refused_float.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_kinds_refused_geometry.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_kinds_refused_literal.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_kinds_refused_nested_array.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_kinds_refused_number.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_kinds_refused_object.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_kinds_refused_option.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_kinds_refused_set.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_kinds_refused_union.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_name_not_plain_refused.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_name_read_through_function.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_relation_refused.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_remove_or_alter_component_name.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_rename_allowed_for_whole_record_values.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_rename_used_by_dynamic_name.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_rename_used_by_event.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_rename_used_by_event_input.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_rename_used_by_event_through_record_params.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_rename_used_by_field.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_rename_used_by_parent_and_destructure.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_rename_used_by_permission.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_rename_used_by_reference_repair.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_rename_used_by_view.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_rename_used_through_function.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_reserved_name_out.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_reserved_names.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_retype_allowed.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_scan_alter_unnamed_to_named.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_scan_body_conflict_refused.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_scan_conforming_kinds.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_scan_conforming_rows.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_scan_copy_recipe.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_scan_create_then_define_in_one_transaction.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_scan_decimal_in_int_refused.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_scan_empty_table.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_scan_equal_body_copy_accepted.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_scan_float_in_int_refused.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_scan_if_not_exists_skips.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_scan_names_readded_in_one_transaction.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_scan_non_whole_number_in_int_refused.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_scan_overwrite_unchanged.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_scan_scalar_key_refused.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_scan_string_in_datetime_refused.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_scan_table_unchanged_after_refusal.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_scan_wrong_arity_refused.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_scan_wrong_record_table_refused.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_scan_year_10000_refused.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_select_permission_refused.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_view_grouping_by_an_alias_reads_no_stored_field.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_view_refused.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_wildcard_view_controls.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_wildcard_view_empty_allowed.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_wildcard_view_judged_when_the_statement_runs.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_wildcard_view_naming_refused.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_wildcard_view_remedy_in_one_transaction.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_components_wildcard_view_rename_and_remove_refused.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/field/id_default.surql",
        "field-type",
    ),
    (
        "language/statements/define/field/migrate/interaction_table_step_judged_last.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/function/_small_body_functions.surql",
        "return-type",
    ),
    (
        "language/statements/define/function/custom_optional_args.surql",
        "argument-count",
    ),
    (
        "language/statements/define/function/inline_site_values.surql",
        "argument-count",
    ),
    (
        "language/statements/define/function/inline_site_values.surql",
        "return-type",
    ),
    (
        "language/statements/define/index/id_components_count_guard_bracketed_component.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/index/id_components_count_guard_through_the_record.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/index/id_components_index_defined_with_the_names.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/index/on_id_component.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/table/id_components_table_conversion_refused.surql",
        "unknown-type",
    ),
    (
        "language/statements/define/table/view/temporal_kind_extremes.surql",
        "unknown-type",
    ),
    (
        "language/statements/delete/id_components_retention.surql",
        "unknown-type",
    ),
    (
        "language/statements/insert/id_components.surql",
        "unknown-type",
    ),
    (
        "language/statements/insert/id_components_insert_relation.surql",
        "unknown-type",
    ),
    ("language/statements/let/typed.surql", "let-type"),
    (
        "language/statements/let/typed_let_in_block.surql",
        "let-type",
    ),
    (
        "language/statements/select/group/a_failing_nested_statement_fails_a_grouped_query.surql",
        "operator-type",
    ),
    (
        "language/statements/select/group/a_value_that_ran_a_nested_statement_fails_a_grouped_query.surql",
        "operator-type",
    ),
    (
        "language/statements/select/id_components_import.surql",
        "unknown-type",
    ),
    (
        "language/statements/select/id_components_info_export.surql",
        "unknown-type",
    ),
    (
        "language/statements/select/id_components_permissions_import.surql",
        "unknown-type",
    ),
    (
        "language/statements/select/id_range/structured_and_lightweight.surql",
        "unknown-type",
    ),
    (
        "language/statements/update/id_components_immutable.surql",
        "unknown-type",
    ),
    (
        "language/statements/update/id_components_merge_none.surql",
        "unknown-type",
    ),
    (
        "language/statements/upsert/id_components_upsert_by_key.surql",
        "unknown-type",
    ),
    ("language/types/named/alias_ordering.surql", "unknown-type"),
    (
        "language/types/named/alias_shadows_union_field.surql",
        "unknown-type",
    ),
    (
        "language/types/named/alter_checks_named_types.surql",
        "unknown-type",
    ),
    (
        "language/types/named/alter_field_in_function.surql",
        "unknown-type",
    ),
    (
        "language/types/named/cast_to_unreadable_type_is_refused.surql",
        "unknown-type",
    ),
    ("language/types/named/closure.surql", "argument-count"),
    ("language/types/named/closure.surql", "unknown-type"),
    ("language/types/named/computed_field.surql", "unknown-type"),
    (
        "language/types/named/define_time_checks.surql",
        "unknown-type",
    ),
    (
        "language/types/named/define_time_checks_clauses.surql",
        "unknown-type",
    ),
    (
        "language/types/named/define_time_checks_definitions.surql",
        "unknown-type",
    ),
    (
        "language/types/named/event_if_not_exists_skips_type_checks.surql",
        "unknown-type",
    ),
    (
        "language/types/named/field_if_not_exists_skips_type_checks.surql",
        "unknown-type",
    ),
    (
        "language/types/named/function_if_not_exists_skips_type_checks.surql",
        "unknown-type",
    ),
    (
        "language/types/named/nested_reads_are_not_the_union_field.surql",
        "unknown-type",
    ),
    (
        "language/types/named/object_absent_field.surql",
        "unknown-type",
    ),
    (
        "language/types/named/omitted_named_argument_import.surql",
        "unknown-type",
    ),
    ("language/types/named/ordering.surql", "unknown-type"),
    (
        "language/types/named/overwrite_in_use.surql",
        "unknown-type",
    ),
    (
        "language/types/named/overwrite_judges_by_coercion.surql",
        "unknown-type",
    ),
    (
        "language/types/named/overwrite_judges_record_key.surql",
        "unknown-type",
    ),
    (
        "language/types/named/overwrite_renders_found_value.surql",
        "unknown-type",
    ),
    (
        "language/types/named/overwrite_validates_nested_values.surql",
        "unknown-type",
    ),
    (
        "language/types/named/overwrite_validates_wildcard_fields.surql",
        "unknown-type",
    ),
    ("language/types/named/record_id.surql", "unknown-type"),
    (
        "language/types/named/referenced_by_access_methods.surql",
        "unknown-type",
    ),
    (
        "language/types/named/referenced_by_events_and_permissions.surql",
        "unknown-type",
    ),
    (
        "language/types/named/referenced_by_expressions.surql",
        "unknown-type",
    ),
    (
        "language/types/named/referenced_by_params_and_apis.surql",
        "unknown-type",
    ),
    (
        "language/types/named/referenced_by_references_indexes_buckets_configs.surql",
        "unknown-type",
    ),
    (
        "language/types/named/referenced_by_views.surql",
        "unknown-type",
    ),
    (
        "language/types/named/rename_held_in_id_subpath.surql",
        "unknown-type",
    ),
    (
        "language/types/named/rename_held_in_record_id.surql",
        "unknown-type",
    ),
    (
        "language/types/named/rename_through_alias.surql",
        "unknown-type",
    ),
    ("language/types/named/session_let.surql", "unknown-type"),
    (
        "language/types/named/set_length_and_union_refusal.surql",
        "unknown-type",
    ),
    ("language/types/named/temporal_union.surql", "unknown-type"),
    ("language/types/named/tuple.surql", "unknown-type"),
    (
        "language/types/named/type_default_on_create.surql",
        "unknown-type",
    ),
    (
        "language/types/named/union_member_order.surql",
        "unknown-type",
    ),
    (
        "language/types/named/view_extreme_of_union.surql",
        "unknown-type",
    ),
    ("language/types/named/write_path.surql", "unknown-type"),
    (
        "language/types/named/write_where_order.surql",
        "unknown-type",
    ),
    (
        "language/write/id_components_catalog_change_between_writes.surql",
        "unknown-type",
    ),
    (
        "language/write/id_components_changefeed.surql",
        "unknown-type",
    ),
    (
        "language/write/id_components_defined_in_the_block.surql",
        "unknown-type",
    ),
    (
        "language/write/id_components_equal_body_copy_cut_on_write.surql",
        "unknown-type",
    ),
    (
        "language/write/id_components_error_precedence.surql",
        "unknown-type",
    ),
    (
        "language/write/id_components_foreign_view.surql",
        "unknown-type",
    ),
    (
        "language/write/id_components_guard_derivations_import.surql",
        "unknown-type",
    ),
    ("language/write/id_components_import.surql", "unknown-type"),
    (
        "language/write/id_components_insert_ignore.surql",
        "unknown-type",
    ),
    (
        "language/write/id_components_keyword_names.surql",
        "unknown-type",
    ),
    (
        "language/write/id_components_mutation_snapshot.surql",
        "unknown-type",
    ),
    (
        "language/write/id_components_return_forms.surql",
        "unknown-type",
    ),
    (
        "language/write/id_components_schemafull.surql",
        "unknown-type",
    ),
    (
        "language/write/observers_see_id_components.surql",
        "unknown-type",
    ),
    ("mysql/_shirts_schema.surql", "unknown-type"),
    ("mysql/_temporal_columns.surql", "unknown-type"),
    (
        "parsing/errors/id_components_duplicate_escaped_name.surql",
        "unknown-type",
    ),
    (
        "parsing/errors/id_components_duplicate_name.surql",
        "unknown-type",
    ),
    ("parsing/errors/id_components_in_cast.surql", "unknown-type"),
    (
        "parsing/errors/id_components_missing_comma.surql",
        "unknown-type",
    ),
    (
        "parsing/errors/id_components_mixed_named_and_unnamed.surql",
        "unknown-type",
    ),
    (
        "parsing/errors/id_components_nested_tuple.surql",
        "unknown-type",
    ),
    (
        "parsing/errors/id_components_not_the_whole_type.surql",
        "unknown-type",
    ),
    (
        "parsing/errors/id_components_on_idiom_name.surql",
        "unknown-type",
    ),
    (
        "parsing/errors/id_components_other_field_keeps_the_kind_error.surql",
        "unknown-type",
    ),
    (
        "parsing/errors/id_components_reserved_name.surql",
        "unknown-type",
    ),
    (
        "parsing/errors/id_components_unnamed_then_named.surql",
        "unknown-type",
    ),
    ("pgsql/_domain_column.surql", "unknown-type"),
    ("pgsql/_enum_schema.surql", "unknown-type"),
    ("pgsql/_interval_qualifiers.surql", "unknown-type"),
    ("pgsql/_json_not_null_domain.surql", "unknown-type"),
    ("pgsql/_temporal_columns.surql", "unknown-type"),
    ("pgsql/_temporal_infinity.surql", "unknown-type"),
    ("pgsql/sql_types/_text_columns.surql", "unknown-type"),
    (
        "reproductions/6616_fields_through_a_computed_reference.surql",
        "argument-type",
    ),
    (
        "reproductions/6697_literal_kind_union_and_cast_members.surql",
        "let-type",
    ),
    (
        "reproductions/7377_view_index_matches_a_table_scan.surql",
        "unknown-method",
    ),
    (
        "reproductions/7532_compound_index_range_matches_a_table_scan.surql",
        "unknown-method",
    ),
    (
        "reproductions/7593_write_with_in_matches_a_table_scan.surql",
        "unknown-method",
    ),
    (
        "reproductions/a_call_after_a_data_dependent_schema_statement_calls_the_definition_it_meets.surql",
        "argument-count",
    ),
    (
        "reproductions/a_call_after_a_data_dependent_schema_statement_calls_the_definition_it_meets.surql",
        "argument-type",
    ),
    (
        "reproductions/a_default_that_does_not_coerce_is_refused_and_not_stored.surql",
        "field-type",
    ),
    (
        "reproductions/a_method_argument_error_is_named_as_main_names_it.surql",
        "argument-count",
    ),
    (
        "reproductions/a_method_argument_error_is_named_as_main_names_it.surql",
        "argument-type",
    ),
    (
        "reproductions/a_nested_broadcast_maps_in_turn.surql",
        "unknown-method",
    ),
    (
        "reproductions/a_scan_keeps_its_sort_when_a_coarse_record_id_keeps_its_names.surql",
        "unknown-type",
    ),
    (
        "reproductions/a_schema_clause_without_a_logical_form.surql",
        "argument-type",
    ),
    (
        "reproductions/a_versioned_scan_delivers_id_order_only_where_the_stamp_says_so.surql",
        "unknown-type",
    ),
    (
        "reproductions/a_versioned_scan_keeps_its_sort_when_the_id_names_are_gone_import.surql",
        "unknown-type",
    ),
    (
        "reproductions/an_error_names_what_failed_as_main_names_it.surql",
        "field-type",
    ),
    (
        "reproductions/an_undeclared_array_id_is_not_in_id_order.surql",
        "unknown-type",
    ),
    (
        "reproductions/fold_function_capability_gate.surql",
        "argument-type",
    ),
    (
        "reproductions/value_clause_type_validation.surql",
        "field-type",
    ),
    ("self_tests/multi_line.surql", "operator-type"),
    (
        "upgrade/define/3.00.00_field_temporal_retype_date.surql",
        "unknown-type",
    ),
    (
        "upgrade/define/3.00.00_field_temporal_retype_interval.surql",
        "unknown-type",
    ),
    (
        "upgrade/define/3.00.00_field_temporal_retype_timestamp.surql",
        "unknown-type",
    ),
    (
        "upgrade/define/3.00.00_id_components_float_in_int_position.surql",
        "unknown-type",
    ),
    (
        "upgrade/define/3.00.00_id_components_nonconforming_old_rows.surql",
        "unknown-type",
    ),
    (
        "upgrade/define/3.00.00_id_components_onto_array_ids.surql",
        "unknown-type",
    ),
    (
        "upgrade/define/3.00.00_id_components_onto_array_ids_current.surql",
        "unknown-type",
    ),
    (
        "upgrade/define/3.00.00_id_components_under_wildcard_view.surql",
        "unknown-type",
    ),
    (
        "upgrade/define/3.00.00_index_datetime_retype_timestamp.surql",
        "unknown-type",
    ),
    (
        "upgrade/define/3.00.00_index_datetime_retype_timestamp_current.surql",
        "unknown-type",
    ),
];

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

    let mut found: BTreeSet<(String, String)> = BTreeSet::new();
    let mut detail: Vec<String> = Vec::new();
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
            .to_string();
        for (code, message) in argument_diagnostics(body) {
            found.insert((relative.clone(), code.clone()));
            detail.push(format!("{relative} :: {code} :: {message}"));
        }
    }

    let expected: BTreeSet<(String, String)> = EXPECTED
        .iter()
        .map(|(file, code)| ((*file).to_string(), (*code).to_string()))
        .collect();

    let unexpected: Vec<&(String, String)> = found.difference(&expected).collect();
    assert!(
        unexpected.is_empty(),
        "the checks fired on {} file(s) not in the expected set:\n{}",
        unexpected.len(),
        detail
            .iter()
            .filter(|line| unexpected
                .iter()
                .any(|(file, code)| line.starts_with(&format!("{file} :: {code}"))))
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );

    // The other direction: a check that silently stops working should also fail.
    let missing: Vec<&(String, String)> = expected.difference(&found).collect();
    assert!(
        missing.is_empty(),
        "these known-bad calls are no longer reported: {missing:?}"
    );
}
