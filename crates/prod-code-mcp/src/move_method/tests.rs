/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[cfg(test)]
mod tests {
    use super::super::edits::{cut_from_impl, insertion, item_span};
    use super::super::syntax::{base_name, receiver_as_type, snake_case, swap_names};

    #[test]
    fn an_impl_left_empty_goes_with_its_last_item() {
        let t = "struct A;\n\nimpl A {\n    fn f() {}\n}\n\nfn g() {}\n";
        let impl_at = t.find("impl").unwrap();
        let open = t.find("{\n    fn").unwrap();
        let close = t.rfind("}\n\nfn g").unwrap();
        let span = (t.find("    fn f").unwrap(), t.find("}\n\nfn g").unwrap());
        let (s, e) = cut_from_impl(t, (impl_at, open, close), span);
        assert_eq!(
            format!("{}{}", &t[..s], &t[e..]),
            "struct A;\n\nfn g() {}\n"
        );
        let two = "impl A {\n    fn f() {}\n    fn h() {}\n}\n";
        let span = (two.find("    fn f").unwrap(), two.find("    fn h").unwrap());
        let (s, e) = cut_from_impl(
            two,
            (0, two.find('{').unwrap(), two.rfind('}').unwrap()),
            span,
        );
        assert_eq!((s, e), span);
    }

    #[test]
    fn a_function_goes_into_the_type_s_impl_or_a_new_one_after_it() {
        let with = "pub struct T;\n\nimpl T {\n    fn a() {}\n}\n";
        let (at, text) = insertion(with, "T", 1, "    fn b() {}\n").unwrap();
        let mut out = with.to_string();
        out.insert_str(at, &text);
        assert_eq!(
            out,
            "pub struct T;\n\nimpl T {\n    fn a() {}\n\n    fn b() {}\n}\n"
        );
        let empty = "pub struct T;\n\nimpl T {\n}\n";
        let (at, text) = insertion(empty, "T", 1, "    fn b() {}\n").unwrap();
        let mut out = empty.to_string();
        out.insert_str(at, &text);
        assert_eq!(out, "pub struct T;\n\nimpl T {\n    fn b() {}\n}\n");
        let none = "pub struct T {\n    x: u8,\n}\nfn f() {}\n";
        let (at, text) = insertion(none, "T", 1, "    fn b() {}\n").unwrap();
        let mut out = none.to_string();
        out.insert_str(at, &text);
        assert_eq!(
            out,
            "pub struct T {\n    x: u8,\n}\n\nimpl T {\n    fn b() {}\n}\nfn f() {}\n"
        );
    }

    #[test]
    fn names_swap_in_one_pass_and_fields_keep_theirs() {
        let body = "{ self.total + self.total * tax.rate + tax.tax + Self::zero().x }";
        let out = swap_names(
            body,
            &[("self", "order"), ("tax", "self"), ("Self", "crate::Order")],
        );
        assert_eq!(
            out,
            "{ order.total + order.total * self.rate + self.tax + crate::Order::zero().x }"
        );
        assert_eq!(
            swap_names("taxes + mytax", &[("tax", "self")]),
            "taxes + mytax"
        );
        assert_eq!(swap_names("a..tax", &[("tax", "self")]), "a..self");
    }

    #[test]
    fn a_receiver_becomes_the_type_it_borrowed() {
        assert_eq!(receiver_as_type("&self", "O"), Some(("&O".into(), false)));
        assert_eq!(
            receiver_as_type("&mut self", "O"),
            Some(("&mut O".into(), false))
        );
        assert_eq!(
            receiver_as_type("&'a self", "O"),
            Some(("&'a O".into(), false))
        );
        assert_eq!(receiver_as_type("self", "O"), Some(("O".into(), false)));
        assert_eq!(receiver_as_type("mut self", "O"), Some(("O".into(), true)));
        assert_eq!(receiver_as_type("self: Box<Self>", "O"), None);
        assert_eq!(snake_case("LineItem"), "line_item");
        assert_eq!(snake_case("Order"), "order");
        assert_eq!(base_name("crate::m::Wrapper<T>"), "Wrapper");
    }

    #[test]
    fn an_item_takes_its_doc_comment_and_its_last_line() {
        let t =
            "impl O {\n    /// Doc.\n    #[inline]\n    pub fn f(&self) {\n        1;\n    }\n}\n";
        let name_at = t.find("f(").unwrap();
        let close = t.find("    }").unwrap() + 4;
        let (s, e) = item_span(t, name_at, close);
        assert_eq!(
            &t[s..e],
            "    /// Doc.\n    #[inline]\n    pub fn f(&self) {\n        1;\n    }\n"
        );
    }
}
