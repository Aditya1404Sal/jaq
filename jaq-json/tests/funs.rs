//! Tests for named core filters, sorted by name.

pub mod common;

use common::give;
use serde_json::json;

yields!(bsearch_absent1, "[1, 3] | bsearch(0)", -1);
yields!(bsearch_absent2, "[1, 3] | bsearch(2)", -2);
yields!(bsearch_absent3, "[1, 3] | bsearch(4)", -3);
yields!(bsearch_present, "[1, 3] | [bsearch(1, 3)]", [0, 1]);

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
    r#""Infinity +Infinity -Infinity" | [fromjson | tostring]"#,
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
    // generated with this command with modification for errors and float rounding
    // cargo run -- -rn 'def f: -2, -1, 0, 2.1, 3, 2000000001; f as $a | f as $b | "give!(json!(null), \"\($a) / \($b)\", \(try ($a % $b) catch tojson));"'
    // TODO: use fail!()?
    // The zero-divisor cases below were regenerated by hand: `%` now raises jq's own "divisor is
    // zero" error (matching real jq) instead of letting a zero divisor through to whatever IEEE
    // gives (which used to be jaq's own, non-jq behavior) — verified against the oracle.
    give(json!(null), "-2 % -2", json!(0));
    give(json!(null), "-2 % -1", json!(0));
    give(
        json!(null),
        "try (-2 % 0) catch .",
        json!(
            "number (-2) and number (0) cannot be divided (remainder) because the divisor is zero"
        ),
    );
    give(json!(null), "-2 % 2.1", json!(-2.0));
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
    give(json!(null), "-1 % 2.1", json!(-1.0));
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
    give(json!(null), "0 % 2.1", json!(0.0));
    give(json!(null), "0 % 3", json!(0));
    give(json!(null), "0 % 2000000001", json!(0));
    give(json!(null), "2.1 % -2 | . * 1000 | round", json!(100));
    give(json!(null), "2.1 % -1 | . * 1000 | round", json!(100));
    give(
        json!(null),
        "try (2.1 % 0) catch .",
        json!(
            "number (2.1) and number (0) cannot be divided (remainder) because the divisor is zero"
        ),
    );
    give(json!(null), "2.1 % 2.1", json!(0.0));
    give(json!(null), "2.1 % 3", json!(2.1));
    give(json!(null), "2.1 % 2000000001", json!(2.1));
    give(json!(null), "3 % -2", json!(1));
    give(json!(null), "3 % -1", json!(0));
    give(
        json!(null),
        "try (3 % 0) catch .",
        json!(
            "number (3) and number (0) cannot be divided (remainder) because the divisor is zero"
        ),
    );
    give(json!(null), "3 % 2.1 | . * 1000 | round", json!(900));
    give(json!(null), "3 % 3", json!(0));
    give(json!(null), "3 % 2000000001", json!(3));
    give(json!(null), "2000000001 % -2", json!(1));
    give(json!(null), "2000000001 % -1", json!(0));
    give(
        json!(null),
        "try (2000000001 % 0) catch .",
        json!(
            "number (2000000001) and number (0) cannot be divided (remainder) because the divisor is zero"
        ),
    );
    give(
        json!(null),
        "2000000001 % 2.1 | . * 1000 | round",
        json!(1800), // 1000 in jq
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
