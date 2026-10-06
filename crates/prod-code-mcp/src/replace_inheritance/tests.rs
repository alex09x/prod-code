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
    use std::path::Path;

    use crate::pull_push::parse_classes_in_text;

    use super::super::languages::{transform_python, transform_typescript};

    #[test]
    fn test_python_replace_inheritance_with_delegation() {
        let py_code = r#"class List:
    def push(self, item):
        pass

    def pop(self):
        pass

class CustomQueue(List):
    def peek(self):
        return self._items[0]
"#;
        let classes = parse_classes_in_text(py_code, "python", Path::new("test.py"));
        let base = &classes[0];
        let sub = &classes[1];

        let transformed = transform_python(
            py_code,
            sub,
            "List",
            "_base",
            &["push".to_string(), "pop".to_string()],
            Some(base),
        )
        .unwrap();

        assert!(!transformed.contains("class CustomQueue(List):"));
        assert!(transformed.contains("class CustomQueue:"));
        assert!(transformed.contains("self._base = List(*args, **kwargs)"));
        assert!(transformed.contains("def push(self, *args, **kwargs):"));
        assert!(transformed.contains("return self._base.push(*args, **kwargs)"));
        assert!(transformed.contains("def pop(self, *args, **kwargs):"));
        assert!(transformed.contains("return self._base.pop(*args, **kwargs)"));
        assert!(transformed.contains("def peek(self):"));
    }

    #[test]
    fn test_ts_replace_inheritance_with_delegation() {
        let ts_code = r#"export class Animal {
    speak(): string {
        return "...";
    }
}

export class Dog extends Animal {
    override bark(): string {
        return "woof";
    }
}
"#;
        let classes = parse_classes_in_text(ts_code, "typescript", Path::new("test.ts"));
        let base = &classes[0];
        let sub = &classes[1];

        let transformed = transform_typescript(
            ts_code,
            sub,
            "Animal",
            "animal",
            &["speak".to_string()],
            Some(base),
        )
        .unwrap();

        assert!(!transformed.contains("class Dog extends Animal"));
        assert!(transformed.contains("class Dog {"));
        assert!(transformed.contains("private animal: Animal;"));
        assert!(transformed.contains("this.animal = new Animal("));
        assert!(transformed.contains("return this.animal.speak("));
        // override should be stripped from bark
        assert!(!transformed.contains("override bark"));
        assert!(transformed.contains("bark(): string"));
    }
}
