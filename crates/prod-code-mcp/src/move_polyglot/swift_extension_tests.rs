/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{insert_member, member_destination};

fn line_of(source: &str, needle: &str) -> u32 {
    source
        .lines()
        .position(|l| l.contains(needle))
        .expect("the declaration is in the source") as u32
        + 1
}

/// The member's text from its line to the end of its braces, as the move cuts it.
fn item_at(source: &str, needle: &str) -> String {
    let start = line_of(source, needle) as usize - 1;
    let lines: Vec<&str> = source.lines().collect();
    let mut depth = 0i32;
    let mut out = String::new();
    for line in &lines[start..] {
        out.push_str(line);
        out.push('\n');
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        if depth <= 0 {
            break;
        }
    }
    out
}

fn destination(source: &str, needle: &str) -> Result<String, String> {
    member_destination(source, line_of(source, needle), &item_at(source, needle))
}

const VIEW: &str = "import Foundation\n\n/// A page.\npublic final class SettingsPage<Model>: NSObject {\n    var title = \"\"\n    let id: Int\n    init(id: Int) { self.id = id }\n    convenience init() { self.init(id: 0) }\n\n    /// Saves it.\n    @discardableResult\n    public func save(to url: URL) -> Bool {\n        if title.isEmpty { return false }\n        return true\n    }\n\n    var isEmpty: Bool {\n        title.isEmpty\n    }\n\n    private func helper() {}\n    override func copy() -> Any { self }\n    deinit {}\n\n    struct Row {\n        func render() -> String { \"\" }\n    }\n\n    func outer() {\n        func local() {}\n    }\n}\n\nextension SettingsPage where Model: Equatable {\n    func same() -> Bool { true }\n}\n\nprotocol Driver {\n    func drive()\n}\n";

/// #2040: a method, a computed property, a convenience initializer and a method of a nested
/// type can live in an extension of their type; the path names that type.
#[test]
fn a_member_an_extension_can_hold_names_its_type() {
    assert_eq!(
        destination(VIEW, "public func save").unwrap(),
        "SettingsPage"
    );
    assert_eq!(destination(VIEW, "var isEmpty").unwrap(), "SettingsPage");
    assert_eq!(
        destination(VIEW, "convenience init").unwrap(),
        "SettingsPage"
    );
    assert_eq!(
        destination(VIEW, "func render").unwrap(),
        "SettingsPage.Row"
    );
}

/// What Swift does not allow in an extension, or what the move would cut off from its type, is
/// refused with the reason.
#[test]
fn a_member_an_extension_cannot_hold_is_refused_with_the_reason() {
    for (needle, reason) in [
        ("var title", "stored property"),
        ("let id", "stored property"),
        ("    init(id", "designated"),
        ("private func helper", "`private`"),
        ("override func copy", "`override`"),
        ("deinit", "deinitializer"),
        ("func local", "inside a function"),
        ("func same", "`where` clause"),
        ("func drive", "protocol"),
    ] {
        let error = destination(VIEW, needle).unwrap_err();
        assert!(error.contains(reason), "{needle}: {error}");
    }
}

/// The analyzer accepts a call from another file to a `private` member and the compiler does
/// not: a member that uses one of this file's private declarations is refused, naming it.
#[test]
fn a_member_using_a_private_declaration_of_its_file_is_refused() {
    let source = "final class Page {\n    private var cache = 0\n    private func secret() -> Int { 1 }\n    func useSecret() -> Int { secret() + cache }\n    func plain() -> Int { 2 }\n    func secretive() -> Int { 3 }\n}\n";
    let error = destination(source, "func useSecret").unwrap_err();
    assert!(error.contains("`cache`, `secret`"), "{error}");
    assert!(error.contains("make them `internal`"), "{error}");
    assert_eq!(destination(source, "func plain").unwrap(), "Page");
    // A longer name that starts with a private one is not a use of it.
    assert_eq!(destination(source, "func secretive").unwrap(), "Page");
}

/// The member goes into the target's extension of the same type, before its closing brace,
/// re-indented under it; a constrained extension or one of another type is not used.
#[test]
fn the_member_joins_an_existing_extension_of_its_type() {
    let target = "import Foundation\n\nextension SettingsPage where Model: Equatable {\n    func same() -> Bool { true }\n}\n\nextension Other {\n}\n\nextension SettingsPage: CustomStringConvertible {\n    var description: String { title }\n}\n";
    let item = "    /// Saves it.\n    public func save() -> Bool {\n        true\n    }\n";
    let out = insert_member(target, "SettingsPage", item);
    assert_eq!(
        out,
        "import Foundation\n\nextension SettingsPage where Model: Equatable {\n    func same() -> Bool { true }\n}\n\nextension Other {\n}\n\nextension SettingsPage: CustomStringConvertible {\n    var description: String { title }\n\n    /// Saves it.\n    public func save() -> Bool {\n        true\n    }\n}\n"
    );
}

/// Without such an extension, a new one is appended; an empty or new target gets just it.
#[test]
fn without_an_extension_of_its_type_a_new_one_is_appended() {
    let item = "        func render() -> String { \"\" }\n";
    assert_eq!(
        insert_member("import Foundation\n", "SettingsPage.Row", item),
        "import Foundation\n\nextension SettingsPage.Row {\n    func render() -> String { \"\" }\n}\n"
    );
    // A new file's blank header does not leave blank lines above the extension.
    assert_eq!(
        insert_member("\n\n", "SettingsPage.Row", item),
        "extension SettingsPage.Row {\n    func render() -> String { \"\" }\n}\n"
    );
    assert_eq!(
        insert_member("extension SettingsPage {}\n", "SettingsPage", item),
        "extension SettingsPage {\n    func render() -> String { \"\" }\n}\n"
    );
}
