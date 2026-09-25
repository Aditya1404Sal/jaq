//! Tests for named core filters, sorted by name.

pub mod common;

use common::{fail, give};
use jaq_json::Error;
use serde_json::json;

yields!(bsearch_absent1, "[1, 3] | bsearch(0)", -1);
yields!(bsearch_absent2, "[1, 3] | bsearch(2)", -2);
yields!(bsearch_absent3, "[1, 3] | bsearch(4)", -3);
yields!(bsearch_present, "[1, 3] | [bsearch(1, 3)]", [0, 1]);

// FA-070: `"a" * 4294967296` used to abort the whole (sandboxed) process attempting the
// allocation instead of refusing — jq itself refuses any repeat past `INT_MAX` result bytes,
// but even a sub-`INT_MAX` allocation this large aborts the sandbox, so the cap here is a real
// platform limit, well below jq's own (see `MAX_REPEATED_STRING_LEN`'s own doc comment).
#[test]
fn string_repeat_refuses_rather_than_aborts() {
    give(json!(null), r#""ab" | . * 100"#, json!("ab".repeat(100)));
    fail(
        json!(null),
        "\"a\" * 4294967296",
        Error::str("Repeat string result too long"),
    );
}

// Like jq, an index past the end extends the array with nulls, up to jq's limit of
// `INT_MAX >> 2`; past it jq refuses instead of allocating.
yields!(
    index_update_extends,
    "[] | .[3] = 1",
    json!([null, null, null, 1])
);
yields!(
    index_update_too_large,
    "[[536870912, 2147483648, 4294967295][] as $i | try ([] | .[$i] = 1 | length) catch .]",
    [
        "Array index too large",
        "Array index too large",
        "Array index too large"
    ]
);

yields!(
    fromjson_inf,
    r#"["Infinity", "+Infinity", "-Infinity"] | map(fromjson | tostring)"#,
    [
        "1.7976931348623157e+308",
        "1.7976931348623157e+308",
        "-1.7976931348623157e+308"
    ]
);
yields!(fromjson_uint, r#"" 1" | fromjson"#, 1);
yields!(fromjson_pint, r#""+1" | fromjson"#, 1);
yields!(fromjson_nint, r#""-1" | fromjson"#, -1);

#[test]
fn has() {
    /* TODO: reenable these tests
    let err = Error::Index(Val::Null, Val::Int(0));
    fail(json!(null), "has(0)", err);
    let err = Error::Index(Val::Int(0), Val::Null);
    fail(json!(0), "has([][0])", err);
    let err = Error::Index(Val::Int(0), Val::Int(1));
    fail(json!(0), "has(1)", err);
    let err = Error::Index(Val::Str("a".to_string().into()), Val::Int(0));
    fail(json!("a"), "has(0)", err);
    */

    give(json!([0, null]), "has(0)", json!(true));
    give(json!([0, null]), "has(1)", json!(true));
    give(json!([0, null]), "has(2)", json!(false));

    give(json!({"a": 1, "b": null}), r#"has("a")"#, json!(true));
    give(json!({"a": 1, "b": null}), r#"has("b")"#, json!(true));
    give(json!({"a": 1, "b": null}), r#"has("c")"#, json!(false));
}

yields!(indices_str, r#""a,b, cd, efg" | indices(", ")"#, [3, 7]);
yields!(
    indices_arr_num,
    "[0, 1, 2, 1, 3, 1, 4] | indices(1)",
    [1, 3, 5]
);
yields!(
    indices_arr_arr,
    "[0, 1, 2, 3, 1, 4, 2, 5, 1, 2, 6, 7] | indices([1, 2])",
    [1, 8]
);
yields!(indices_arr_str, r#"["a", "b", "c"] | indices("b")"#, [1]);

yields!(indices_arr_empty, "[0, 1] | indices([])", json!([]));
yields!(indices_arr_larger, "[1, 2] | indices([1, 2, 3])", json!([]));

yields!(indices_arr_overlap, "[0, 0, 0] | indices([0, 0])", [0, 1]);
yields!(indices_str_overlap, r#""aaa" | indices("aa")"#, [0, 1]);
yields!(indices_str_gb1, r#""🇬🇧!" | indices("!")"#, [2]);
yields!(indices_str_gb2, r#""🇬🇧🇬🇧" | indices("🇬🇧")"#, [0, 2]);

yields!(length_str_foo, r#""ƒoo" | length"#, 3);
yields!(length_str_namaste, r#""नमस्ते" | length"#, 6);
yields!(length_obj, r#"{"a": 5, "b": 3} | length"#, 2);
yields!(length_int_pos, " 2 | length", 2);
yields!(length_int_neg, "-2 | length", 2);
yields!(length_float_pos, " 2.5 | length", 2.5);
yields!(length_float_neg, "-2.5 | length", 2.5);

yields!(tojson_fl0, "1.0 | tojson", "1.0");
yields!(tojson_fl1, "1.1 | tojson", "1.1");
// These used to construct NaN/Infinity via `0.0/0.0`/`1.0/0.0`/`-1.0/0.0`, but `/` now raises
// jq's own "divisor is zero" error on an exact-zero divisor (matching real jq) instead of
// letting it through to the IEEE result. `1e1000` is parsed lazily as an exact decimal, not yet
// a float; forcing it through an arithmetic op (`+ 0`) converts it to an IEEE double, which
// overflows to +-infinity, matching jq's own `builtin.jq` (`def infinite: 1e1000;`) — jaq just
// needs the nudge into arithmetic that jq's eager float parsing does for free.
yields!(tojson_nan, "((1e1000 + 0) - (1e1000 + 0)) | tojson", "null");
yields!(
    tojson_inf,
    "(1e1000 + 0) | tojson",
    "1.7976931348623157e+308"
);
yields!(
    tojson_ninf,
    "(-1e1000 + 0) | tojson",
    "-1.7976931348623157e+308"
);

#[test]
fn tonumber() {
    give(json!(1.0), "tonumber", json!(1.0));
    give(json!("1.0"), "tonumber", json!(1.0));
    give(json!("42"), "tonumber", json!(42));
    give(json!("null"), "try tonumber catch -7", json!(-7));
    give(json!("true"), "try tonumber catch -7", json!(-7));
    give(json!("str"), "try tonumber catch -7", json!(-7));
    give(json!("\"str\""), "try tonumber catch -7", json!(-7));
    give(json!("[3, 4]"), "try tonumber catch -7", json!(-7));
    give(json!("{\"a\": 1}"), "try tonumber catch -7", json!(-7));
}

#[test]
fn toboolean() {
    give(json!(false), "toboolean", json!(false));
    give(json!("true"), "toboolean", json!(true));
    give(json!("false"), "toboolean", json!(false));
    give(json!("null"), "try toboolean catch -7", json!(-7));
    give(json!("3"), "try toboolean catch -7", json!(-7));
    give(json!("str"), "try toboolean catch -7", json!(-7));
    give(json!("\"str\""), "try toboolean catch -7", json!(-7));
    give(json!("[3, 4]"), "try toboolean catch -7", json!(-7));
    give(json!("{\"a\": 1}"), "try toboolean catch -7", json!(-7));
}

#[test]
fn math_rem() {
    // jq's `%` truncates both sides to integers (`dtoi`) and refuses a divisor that truncates
    // to 0; each expected value below is real jq 1.8.2's output.
    // TODO: use fail!()?
    give(json!(null), "-2 % -2", json!(0));
    give(json!(null), "-2 % -1", json!(0));
    give(
        json!(null),
        "try (-2 % 0) catch .",
        json!(
            "number (-2) and number (0) cannot be divided (remainder) because the divisor is zero"
        ),
    );
    give(json!(null), "-2 % 2.1", json!(0));
    give(json!(null), "-2 % 3", json!(-2));
    give(json!(null), "-2 % 2000000001", json!(-2));
    give(json!(null), "-1 % -2", json!(-1));
    give(json!(null), "-1 % -1", json!(0));
    give(
        json!(null),
        "try (-1 % 0) catch .",
        json!(
            "number (-1) and number (0) cannot be divided (remainder) because the divisor is zero"
        ),
    );
    give(json!(null), "-1 % 2.1", json!(-1));
    give(json!(null), "-1 % 3", json!(-1));
    give(json!(null), "-1 % 2000000001", json!(-1));
    give(json!(null), "0 % -2", json!(0));
    give(json!(null), "0 % -1", json!(0));
    give(
        json!(null),
        "try (0 % 0) catch .",
        json!(
            "number (0) and number (0) cannot be divided (remainder) because the divisor is zero"
        ),
    );
    give(json!(null), "0 % 2.1", json!(0));
    give(json!(null), "0 % 3", json!(0));
    give(json!(null), "0 % 2000000001", json!(0));
    give(json!(null), "2.1 % -2 | . * 1000 | round", json!(0));
    give(json!(null), "2.1 % -1 | . * 1000 | round", json!(0));
    give(
        json!(null),
        "try (2.1 % 0) catch .",
        json!(
            "number (2.1) and number (0) cannot be divided (remainder) because the divisor is zero"
        ),
    );
    give(json!(null), "2.1 % 2.1", json!(0));
    give(json!(null), "2.1 % 3", json!(2));
    give(json!(null), "2.1 % 2000000001", json!(2));
    give(json!(null), "3 % -2", json!(1));
    give(json!(null), "3 % -1", json!(0));
    give(
        json!(null),
        "try (3 % 0) catch .",
        json!(
            "number (3) and number (0) cannot be divided (remainder) because the divisor is zero"
        ),
    );
    give(json!(null), "3 % 2.1 | . * 1000 | round", json!(1000));
    give(json!(null), "3 % 3", json!(0));
    give(json!(null), "3 % 2000000001", json!(3));
    give(json!(null), "2000000001 % -2", json!(1));
    give(json!(null), "2000000001 % -1", json!(0));
    give(json!(null), "try (2000000001 % 0) catch .", json!("number (2000000001) and number (0) cannot be divided (remainder) because the divisor is zero"));
    give(
        json!(null),
        "2000000001 % 2.1 | . * 1000 | round",
        json!(1000),
    );
    give(json!(null), "2000000001 % 3", json!(0));
    give(json!(null), "2000000001 % 2000000001", json!(0));
}

yields!(
    format_urid_invalid,
    r#"("%FF" | @urid) == ([255] | tobytes | tostring)"#,
    true
);

// jq 1.8 compatibility: number printing.
yields!(jq_integral_float, "10 / 2 | tojson", "5");
yields!(
    jq_float_forms,
    "[0.00001 * 1, 1e17 * 1, 1 / 3, 1.5e300 * 1, 123456789012 * 1, 0.0001 * 1] | tojson",
    "[1e-05,1e+17,0.3333333333333333,1.5e+300,123456789012,0.0001]"
);
yields!(
    jq_decimal_literals,
    "[1.0, 1.50, 3e2, 3.0e2, 1e-5, 12e-9, 0.050, 1.500e3] | tojson",
    "[1.0,1.50,3E+2,3.0E+2,0.00001,1.2E-8,0.050,1500]"
);
yields!(jq_interpolated_float, r#""\(10 / 4) \(9 / 3)""#, "2.5 3");

// jq 1.8 compatibility: updates create structure through null and past array ends.
yields!(
    jq_null_key_update,
    r#"null | .a.b = 1 | tojson"#,
    r#"{"a":{"b":1}}"#
);
yields!(jq_null_index_update, "null | .[1] = 1 | tojson", "[null,1]");
yields!(
    jq_array_extension,
    "[1] | .[3] = 4 | tojson",
    "[1,null,null,4]"
);
yields!(
    jq_nested_extension,
    r#"{"a":[]} | .a[2].b = 1 | tojson"#,
    r#"{"a":[null,null,{"b":1}]}"#
);
yields!(
    jq_negative_zero_literal,
    r#""[-0.0]" | fromjson | tojson"#,
    "[-0.0]"
);
yields!(jq_null_slice, "null | .[1:] | tojson", "null");
yields!(
    jq_negative_out_of_bounds,
    "[1] | try (.[-5] = 9) catch .",
    "Out of bounds negative array index"
);

// jq 1.8 syntax: `.5` and `1.` literals, escape runs with surrogate pairs, `?//`.
yields!(
    jq_dot_numbers,
    "[.1 + .2, 1., .5e1, 1.50] | tojson",
    "[0.30000000000000004,1,5,1.50]"
);
yields!(jq_surrogate_pair, r#""😀" | explode"#, [128512]);
yields!(
    jq_lone_low_surrogate,
    r#""a\ude00b" | explode"#,
    [97, 65533, 98]
);
yields!(
    jq_destructuring_alternatives,
    r#"[[[1,2],{"a":3}] | .[] as [$a,$b] ?// {a:$a} | [$a,$b]] | tojson"#,
    "[[1,2],[3,null]]"
);
yields!(
    jq_destructuring_alternative_after_error,
    r#"[[3]] | .[] as [$a] ?// [$b] | if $a != null then error("x") else {$a,$b} end | tojson"#,
    r#"{"a":null,"b":3}"#
);

// jq 1.8 definitions jaq lacked.
yields!(
    jq_tostream,
    r#"{"a":[1,{"b":2}],"c":[]} | [tostream] | tojson"#,
    r#"[[["a",0],1],[["a",1,"b"],2],[["a",1,"b"]],[["a",1]],[["c"],[]],[["c"]]]"#
);
yields!(
    jq_fromstream,
    r#"{"a":[1,{"b":2}],"c":[]} | fromstream(tostream) | tojson"#,
    r#"{"a":[1,{"b":2}],"c":[]}"#
);
yields!(
    jq_truncate_stream,
    "[1 | truncate_stream([[0],1],[[1,0],2],[[1,0]],[[1]])] | tojson",
    "[[[0],2],[[0]]]"
);
yields!(
    jq_in,
    "[2 | IN(1, 2), ([1, 5] | IN(.[]; 5, 6))]",
    [true, true]
);
yields!(
    jq_index,
    r#"[{"id":1,"v":"a"},{"id":2,"v":"b"}] | INDEX(.id) | tojson"#,
    r#"{"1":{"id":1,"v":"a"},"2":{"id":2,"v":"b"}}"#
);

// jq 1.8's `tonumber` and `fromjson`: decNumber's syntax and jq's messages.
yields!(
    jq_tonumber,
    r#"["1", " 1", "nan", "+1", ".5", "-0", "0x10"] | map(try tonumber catch .) | tojson"#,
    r#"[1,"string (\" 1\") cannot be parsed as a number",null,1,0.5,-0,"string (\"0x10\") cannot be parsed as a number"]"#
);
yields!(
    jq_tonumber_array,
    "[1] | try tonumber catch .",
    "array ([1]) cannot be parsed as a number"
);
yields!(
    jq_fromjson_errors,
    r#"["1 2", "{", "[1,]", "x"] | map(try fromjson catch .)"#,
    [
        "Unexpected extra JSON values (while parsing '1 2')",
        "Unfinished JSON term at EOF at line 1, column 1 (while parsing '{')",
        "Expected another array element at line 1, column 4 (while parsing '[1,]')",
        "Invalid numeric literal at EOF at line 1, column 1 (while parsing 'x')"
    ]
);
yields!(
    jq_fromjson_type,
    "1 | try fromjson catch .",
    "number (1) only strings can be parsed"
);
yields!(
    jq_implode_errors,
    r#"[(["a"] | try implode catch .), ("a" | try implode catch .)]"#,
    [
        "string (\"a\") can't be imploded, unicode codepoint needs to be numeric",
        "implode input must be an array"
    ]
);

// jq 1.8 numbers: doubles past 2^53, and the sign of zero.
yields!(
    jq_big_int_arithmetic,
    "[12345678901234567890123] | map(. + 0) | tojson",
    "[12345678901234568000000]"
);
yields!(
    jq_negative_zero,
    "[0 * -1, -0.0, 0 / -1, -(0), (-0 | tostring)] | tojson",
    r#"[-0,0.0,-0,0,"0"]"#
);
yields!(jq_rem_truncates, "[5.5 % 2, 5 % 2.5, 1e30 % 7]", [1, 1, 0]);
