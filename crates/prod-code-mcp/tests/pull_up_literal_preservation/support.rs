/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::pull_push::{parse_classes_in_text, pull_up_impl};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

pub struct Case {
    ws: Workspace,
    file: PathBuf,
    base: Option<PathBuf>,
    original: String,
}

impl Case {
    pub fn new(name: &str, source: &str, multi_file: bool) -> Self {
        let ws = Workspace::new(&[(
            "Cargo.toml",
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )]);
        let file = ws.root().join(name);
        let base = multi_file.then(|| ws.root().join("base.py"));
        if let Some(base) = &base {
            let (parent, children) = source.split_once("class Dog").unwrap();
            fs::write(base, parent).unwrap();
            fs::write(
                &file,
                format!("from base import Pet\n\nclass Dog{children}"),
            )
            .unwrap();
        } else {
            fs::write(&file, source).unwrap();
        }
        let mut case = Self {
            ws,
            file,
            base,
            original: String::new(),
        };
        case.original = case.source();
        case
    }

    pub async fn pull(&self) {
        // This gateway scripts only transport/diagnostic plumbing. Empty replies are NOT
        // semantic evidence: Python/Node execute both original and written programs below.
        let gw = ScriptedGateway::start(|method, _| match method {
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        })
        .await;
        let result = pull_up_impl(
            gw.addr(),
            &self.ws.root(),
            &self.file,
            "Dog",
            Some("Pet"),
            &["speak".to_string()],
            true,
            true,
            false,
            None,
        )
        .await
        .unwrap();
        assert!(result.applied);
        assert!(
            !result.verified,
            "scripted diagnostics cannot prove compilation"
        );
    }

    pub async fn assert_pull_refused(&self, message: &str) {
        // An unsupported declaration must be rejected before analyzer access or writes.
        let result = pull_up_impl(
            "127.0.0.1:1".parse().unwrap(),
            &self.ws.root(),
            &self.file,
            "Dog",
            Some("Pet"),
            &["speak".to_string()],
            true,
            true,
            false,
            None,
        )
        .await;
        assert!(result.unwrap_err().to_string().contains(message));
        assert_eq!(self.source(), self.original, "refused pull-up wrote source");
    }

    pub fn replace_cpp_inheritance(&self) {
        let source = self.source();
        let classes = parse_classes_in_text(&source, "cpp", &self.file);
        let sub = classes.iter().find(|c| c.name == "Dog").unwrap();
        let base = classes.iter().find(|c| c.name == "Pet").unwrap();
        let after = prod_code_mcp::replace_inheritance::languages::transform_cpp(
            &source,
            sub,
            "Pet",
            "pet",
            &[],
            Some(base),
        )
        .unwrap();
        fs::write(&self.file, after).unwrap();
    }

    pub fn assert_member(&self, class: &str, name: &str, payload: &str) {
        let path = if class == "Pet" {
            self.base.as_ref().unwrap_or(&self.file)
        } else {
            &self.file
        };
        let source = fs::read_to_string(path).unwrap();
        let language = self.language();
        let classes = parse_classes_in_text(&source, language, path);
        let class = classes.iter().find(|c| c.name == class).unwrap();
        let member = class.members.iter().find(|m| m.name == name).unwrap();
        assert_eq!(
            &source[member.start_offset..member.end_offset],
            member.full_text
        );
        assert!(
            member.full_text.contains(payload),
            "selected member: {:?}",
            member.full_text
        );
        assert!(!member.full_text.contains("class Cat"));
    }

    pub fn assert_no_member(&self, class: &str, name: &str) {
        let source = fs::read_to_string(&self.file).unwrap();
        let language = self.language();
        let classes = parse_classes_in_text(&source, language, &self.file);
        let class = classes.iter().find(|c| c.name == class).unwrap();
        assert!(class.members.iter().all(|m| m.name != name), "{source}");
    }

    pub fn source(&self) -> String {
        let base = self
            .base
            .as_ref()
            .map(|p| fs::read_to_string(p).unwrap())
            .unwrap_or_default();
        format!("{base}{}", fs::read_to_string(&self.file).unwrap())
    }

    fn language(&self) -> &'static str {
        match self.file.extension().unwrap().to_str().unwrap() {
            "py" => "python",
            "cpp" => "cpp",
            "ts" => "typescript",
            _ => "javascript",
        }
    }

    pub fn python_values(&self, classes: &[&str]) -> Vec<String> {
        let script = format!(
            "import json, runpy\ns = runpy.run_path({:?})\nprint(json.dumps([str(s[c]().speak()) for c in {:?}]))",
            self.file.to_str().unwrap(),
            classes,
        );
        self.run("python3", "-c", &script)
    }

    pub fn javascript_values(&self, classes: &[&str]) -> Vec<String> {
        let script = format!(
            "{}\nconsole.log(JSON.stringify([{}]));",
            self.source(),
            classes
                .iter()
                .map(|c| format!("String(new {c}().speak())"))
                .collect::<Vec<_>>()
                .join(",")
        );
        self.run("node", "-e", &script)
    }

    pub fn cpp_values(&self, classes: &[&str]) -> Vec<String> {
        let binary = self.ws.root().join("check-values");
        let script = format!(
            "{}\nint main() {{ std::cout << \"[\" {} << \" ]\"; }}\n",
            self.source(),
            classes
                .iter()
                .enumerate()
                .map(|(i, c)| format!(
                    "{} << std::quoted({c}().speak())",
                    if i == 0 { "" } else { " << \",\"" }
                ))
                .collect::<String>()
        );
        let mut child = Command::new("c++")
            .args(["-std=c++17", "-x", "c++", "-", "-o"])
            .arg(&binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "c++: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = Command::new(binary).output().unwrap();
        assert!(output.status.success());
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn run(&self, runtime: &str, flag: &str, script: &str) -> Vec<String> {
        let output = Command::new(runtime)
            .args([flag, script])
            .current_dir(self.ws.root())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{runtime}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    pub fn evidence(&self, before: &[String], after: &[String]) {
        fn hash(text: &str) -> String {
            let output = Command::new("python3")
                .args([
                    "-c",
                    "import hashlib,sys; print(hashlib.sha256(sys.argv[1].encode()).hexdigest())",
                    text,
                ])
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        }
        let source = self.source();
        // Write directly so the normal successful-test capture does not hide the
        // source/value evidence needed to compare the baseline with the repair.
        let evidence = format!(
            "before source={:?} sha256={} runtime={before:?}\nafter source={source:?} sha256={} runtime={after:?}\n",
            self.original,
            hash(&self.original),
            hash(&source)
        );
        std::io::stderr().write_all(evidence.as_bytes()).unwrap();
    }
}
