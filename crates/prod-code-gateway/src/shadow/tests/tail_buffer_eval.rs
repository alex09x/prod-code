/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::tail_buffer::TailBuffer;

#[test]
fn tail_buffer_keeps_only_the_end_and_counts_everything() {
    let mut tail = TailBuffer::new(5);
    tail.push(b"abc");
    tail.push(b"defgh");
    tail.push(b"ij");
    assert_eq!(tail.bytes(), b"fghij");
    assert_eq!(tail.total, 10);
    tail.push(b"0123456789");
    assert_eq!(tail.bytes(), b"56789");
}

#[test]
fn tail_buffer_retains_compiler_errors_when_warnings_exceed_limit() {
    let mut tail = TailBuffer::new(200);
    let error_text = b"error[E0061]: this function takes 2 arguments but 1 argument was supplied\n  --> server/src/lib.rs:42:15\n   |\n42 |     calculate(foo);\n   |     ^^^^^^^^^ expected 2 arguments\nhelp: provide the argument: `, bar`\n";
    tail.push(error_text);
    // Push warnings that far exceed the 200 byte limit
    for i in 0..20 {
        tail.push(format!("warning: call to unsafe function {i} (error E0133)\n").as_bytes());
    }
    tail.push(b"error: could not compile `server` due to 1 previous error\n");

    assert_eq!(tail.bytes().len(), 200);
    let output = String::from_utf8_lossy(&tail.to_output()).into_owned();
    assert!(output.contains("error[E0061]"), "{output}");
    assert!(output.contains("server/src/lib.rs:42:15"), "{output}");
    assert!(output.contains("help: provide the argument"), "{output}");
    assert!(output.contains("could not compile"), "{output}");
}
