// Known false positives for the 4.0.0 language-test sweep.
// A pair is expected only when the file's error wording matches.
// A fix removes a line. A new pair fails the sweep.
#[rustfmt::skip]
const KNOWN_FALSE_POSITIVES: &[(&str, &str, &str)] = &[
    ("api/errors/caught_grouped_query_failure_leaves_no_writes.surql", "operator-type", "The file declares error = \"The table 't' does not exist\" for the later select. 1 + 'a' is the nested failure, and this operator-type wording is not that declaration."),
    ("bench/executor/rt_import.surql", "argument-type", "A bench import. The checker says an element of vector::divide is bool where a number is required. The file declares no error."),
    ("bench/executor/rt_import.surql", "operator-type", "The same bench import. bool plus or minus a number is reported. The file declares no error."),
    ("language/expression/shared_subexpression_fallible.surql", "operator-type", "The checker reports bool + int. The file's errors are casts and a negation, not that addition."),
    ("language/statements/define/function/_small_body_functions.surql", "return-type", "fn::wrong is declared -> int and returns a string. The file is a setup import and declares no error."),
    ("language/statements/define/function/inline_site_values.surql", "return-type", "fn::text is declared -> string and returns an int. The file's declared error is fn::double's argument, not this return."),
    ("language/statements/select/group/a_value_that_ran_a_nested_statement_fails_a_grouped_query.surql", "operator-type", "The file declares addition of int and string. The checker also reports string and int, and that sentence is not in the file."),
    ("language/types/named/alter_checks_named_types.surql", "unknown-type", "The type `missing` is not defined in this file. The engine says the type does not exist. `Unknown type` is not that sentence."),
    ("language/types/named/alter_field_in_function.surql", "unknown-type", "The type `missing` is not defined in this file. The engine says the type does not exist. `Unknown type` is not that sentence."),
    ("language/types/named/cast_to_unreadable_type_is_refused.surql", "unknown-type", "`hidden` and `shown` are not defined in this file. The engine's error is that `hidden` cannot be read. `Unknown type` is not that sentence."),
    ("language/types/named/closure.surql", "argument-count", "A parameter left out is judged as NONE. $k() and fn::pct() are reported as empty calls; the file says the argument was NONE, and it names the closure ANONYMOUS."),
    ("language/types/named/define_time_checks.surql", "unknown-type", "The type `nope` is not defined in this file. The engine says the type does not exist. `Unknown type` is not that sentence."),
    ("language/types/named/define_time_checks_clauses.surql", "unknown-type", "The type `missing` is not defined in this file. The engine says the type does not exist. `Unknown type` is not that sentence."),
    ("language/types/named/define_time_checks_definitions.surql", "unknown-type", "The type `missing` is not defined in this file. The engine says the type does not exist. `Unknown type` is not that sentence."),
    ("language/types/named/event_if_not_exists_skips_type_checks.surql", "unknown-type", "The type `missing` is not defined in this file. The engine says the type does not exist. `Unknown type` is not that sentence."),
    ("language/types/named/field_if_not_exists_skips_type_checks.surql", "unknown-type", "The type `missing` is not defined in this file. The engine says the type does not exist. `Unknown type` is not that sentence."),
    ("language/types/named/function_if_not_exists_skips_type_checks.surql", "unknown-type", "The type `missing` is not defined in this file. The engine says the type does not exist. `Unknown type` is not that sentence."),
    ("reproductions/6616_fields_through_a_computed_reference.surql", "argument-type", "array::sort is reported on an object. The file declares no error."),
    ("reproductions/a_call_after_a_data_dependent_schema_statement_calls_the_definition_it_meets.surql", "argument-count", "fn::f is reported as one argument, found two. The file's errors are the coercion and the missing function."),
    ("reproductions/fold_function_capability_gate.surql", "argument-type", "type::file is reported as wanting a string and finding a number. The file's error is that the function is not allowed to run."),
    ("self_tests/multi_line.surql", "operator-type", "1 + \"1\" is declared error = true, with no wording."),
];
