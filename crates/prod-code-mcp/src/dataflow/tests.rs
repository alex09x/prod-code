/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::slice::slice_intra_function;
use super::types::SliceCompleteness;

#[test]
fn test_intra_function_linear_arithmetic() {
    let code = r#"fn calculate(a: i32, b: i32) -> i32 {
    let x = a + 1;
    let unused_val = 100;
    let y = x * 2;
    let unrelated = unused_val + 10;
    println!("log: {}", unrelated);
    y
}"#;
    let slice = slice_intra_function(code, 1, 8, "calculate", "src/calc.rs", Some(7), Some("y"));
    assert!(slice.completeness.is_complete());
    assert_eq!(slice.retained_lines, 5); // fn sig, let x, let y, y, }
    let retained_text = slice.formatted_slice;
    assert!(retained_text.contains("let x = a + 1;"));
    assert!(retained_text.contains("let y = x * 2;"));
    assert!(!retained_text.contains("unused_val"));
    assert!(!retained_text.contains("unrelated"));
    assert!(retained_text.contains("COMPLETE"));
}

#[test]
fn test_intra_function_conditional_branches() {
    let code = r#"fn process(input: i32, flag: bool) -> i32 {
    let mut res = 0;
    let mut debug_count = 0;
    if flag {
        res = input * 10;
        debug_count += 1;
    } else {
        res = input + 5;
        debug_count += 2;
    }
    println!("debug: {}", debug_count);
    res
}"#;
    let slice = slice_intra_function(code, 1, 14, "process", "src/proc.rs", Some(13), Some("res"));
    assert!(slice.completeness.is_complete());
    let text = slice.formatted_slice;
    assert!(text.contains("if flag {"));
    assert!(text.contains("res = input * 10;"));
    assert!(text.contains("res = input + 5;"));
    assert!(!text.contains("debug_count"));
    assert!(!text.contains("println!"));
}

#[test]
fn test_intra_function_mutation_and_loops() {
    let code = r#"fn sum_positive(items: &[i32]) -> i32 {
    let mut total = 0;
    let mut skipped = 0;
    for &x in items {
        if x > 0 {
            total += x;
        } else {
            skipped += 1;
        }
    }
    log_metric(skipped);
    total
}"#;
    let slice = slice_intra_function(
        code,
        1,
        13,
        "sum_positive",
        "src/sum.rs",
        Some(12),
        Some("total"),
    );
    assert!(slice.completeness.is_complete());
    let text = slice.formatted_slice;
    assert!(text.contains("total += x;"));
    assert!(text.contains("if x > 0 {"));
    assert!(!text.contains("skipped"));
    assert!(!text.contains("log_metric"));
}

#[test]
fn test_intra_function_go_syntax() {
    let code = r#"func Process(id string, amount float64, fast bool) float64 {
    logger := log.New()
    logger.Print(id)
    discount := 0.0
    if amount > 100.0 {
        discount = amount * 0.1
    }
    fee := 5.0
    if fast {
        fee = 15.0
    }
    total := amount - discount + fee
    metrics.Incr()
    return total
}"#;
    let slice = slice_intra_function(code, 1, 15, "Process", "main.go", Some(14), Some("total"));
    assert!(slice.completeness.is_complete());
    let text = slice.formatted_slice;
    assert!(text.contains("discount := 0.0"));
    assert!(text.contains("fee := 5.0"));
    assert!(text.contains("total := amount - discount + fee"));
    assert!(!text.contains("logger"));
    assert!(!text.contains("metrics"));
}

#[test]
fn test_intra_function_python_syntax() {
    let code = r#"def calculate_payout(user_id, base_salary, performance_score):
    audit_log(user_id, "start")
    multiplier = 1.0
    if performance_score > 90:
        multiplier = 1.5
    else:
        multiplier = 1.1
    payout = base_salary * multiplier
    send_notification(user_id, payout)
    return payout
"#;
    let slice = slice_intra_function(
        code,
        1,
        10,
        "calculate_payout",
        "service.py",
        Some(10),
        Some("payout"),
    );
    assert!(slice.completeness.is_complete());
    let text = slice.formatted_slice;
    assert!(text.contains("multiplier = 1.0"));
    assert!(text.contains("if performance_score > 90:"));
    assert!(text.contains("payout = base_salary * multiplier"));
    assert!(!text.contains("audit_log"));
    assert!(!text.contains("send_notification"));
}

#[test]
fn test_intra_function_typescript_syntax() {
    let code = r#"function getDiscountedPrice(item: Item, user: User, isVIP: boolean): number {
    console.log("Checking item", item.id);
    const base = item.price;
    let rate = 0.05;
    if (isVIP) {
        rate = 0.20;
    }
    const finalPrice = base * (1 - rate);
    trackAnalytics("price_computed", finalPrice);
    return finalPrice;
}"#;
    let slice = slice_intra_function(
        code,
        1,
        11,
        "getDiscountedPrice",
        "pricing.ts",
        Some(10),
        Some("finalPrice"),
    );
    assert!(slice.completeness.is_complete());
    let text = slice.formatted_slice;
    assert!(text.contains("const base = item.price;"));
    assert!(text.contains("if (isVIP) {"));
    assert!(text.contains("const finalPrice = base * (1 - rate);"));
    assert!(!text.contains("console.log"));
    assert!(!text.contains("trackAnalytics"));
}

#[test]
fn test_intra_function_auto_target_return() {
    let code = r#"fn compute_total(price: f64, tax_rate: f64) -> f64 {
    let subtotal = price;
    let unused_counter = 42;
    let tax = subtotal * tax_rate;
    let unneeded_str = format!("counter={}", unused_counter);
    let total = subtotal + tax;
    total
}"#;
    let slice = slice_intra_function(code, 1, 8, "compute_total", "src/math.rs", None, None);
    assert!(slice.completeness.is_complete());
    let text = slice.formatted_slice;
    assert!(text.contains("let subtotal = price;"));
    assert!(text.contains("let tax = subtotal * tax_rate;"));
    assert!(text.contains("let total = subtotal + tax;"));
    assert!(!text.contains("unused_counter"));
    assert!(!text.contains("unneeded_str"));
}

#[test]
fn test_intra_function_incomplete_evidence() {
    let code = r#"fn compute(x: i32) -> i32 {
    let y = x + external_magic_value;
    y
}"#;
    let slice = slice_intra_function(code, 1, 4, "compute", "src/magic.rs", Some(3), Some("y"));
    assert!(!slice.completeness.is_complete());
    assert!(matches!(
        slice.completeness,
        SliceCompleteness::Incomplete { .. }
    ));
    assert!(slice.formatted_slice.contains("INCOMPLETE"));
}
