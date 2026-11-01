/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::sync::LazyLock;

use regex::Regex;

// Deliberately lexical, not an analyzer: collect declarations without loading a workspace,
// resolving imports, running build scripts, or starting a language server.
static DECLARATIONS: LazyLock<Vec<(&[&str], Regex)>> = LazyLock::new(|| {
    let patterns: &[(&[&str], &str)] = &[
        (
            &["rs"],
            r"\b(?:struct|enum|trait|type|fn|mod|const|static)\s+(?:mut\s+)?([\p{XID_Start}_][\p{XID_Continue}]*)",
        ),
        (
            &["go"],
            r"\b(?:type|const)\s+(\w+)|\bfunc\s+(?:\([^\n)]*\)\s*)?(\w+)",
        ),
        (
            &["py", "pyi"],
            r"\b(?:class|def)\s+(\w+)|(?m)^\s*([A-Z][A-Z_0-9]+)\s*(?::[^=\n]+)?=",
        ),
        (
            &["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts"],
            r"\b(?:class|interface|type|enum|namespace|module|function|const)\s+(\w+)",
        ),
        (
            &["swift"],
            r"\b(?:class|struct|enum|protocol|typealias|func|let)\s+(\w+)",
        ),
        (
            &["c", "h", "cc", "cpp", "cxx", "hpp", "hh", "hxx"],
            r"\b(?:class|struct|enum|namespace|concept)\s+(\w+)|(?m)^\s*#\s*define\s+(\w+)|(?m)^\s*(?:[\w:*&<>]+\s+)+([A-Za-z_]\w*)\s*\(",
        ),
        (
            &["java", "kt", "kts", "cs"],
            r"\b(?:class|interface|enum|record|object|fun|namespace|const)\s+(\w+)|(?m)^\s*(?:[\w<>?\[\]]+\s+)+([A-Za-z_]\w*)\s*\(",
        ),
        (
            &["rb"],
            r"\b(?:class|module|def)\s+(\w+)|(?m)^\s*([A-Z]\w*)\s*=",
        ),
        (
            &["php"],
            r"\b(?:class|interface|trait|function|const|namespace)\s+(\w+)",
        ),
    ];
    patterns
        .iter()
        .map(|(extensions, pattern)| (*extensions, Regex::new(pattern).expect("declaration regex")))
        .collect()
});

pub(super) fn pattern(extension: &str) -> Option<&'static Regex> {
    DECLARATIONS
        .iter()
        .find(|(extensions, _)| extensions.contains(&extension))
        .map(|(_, regex)| regex)
}

/// Returns true if `name` is a compound identifier (snake_case, camelCase, PascalCase,
/// SCREAMING_SNAKE_CASE, or contains digits). Compound identifiers are never generic.
fn is_compound(name: &str) -> bool {
    if name.contains('_') || name.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }
    let mut chars = name.chars();
    if chars.next().is_some_and(|c| c.is_ascii_lowercase()) && chars.any(|c| c.is_ascii_uppercase())
    {
        return true;
    }
    if name.chars().filter(|c| c.is_ascii_uppercase()).count() > 1 {
        return true;
    }
    false
}

/// Generic language, library, technical and ordinary English vocabulary is useful in a neutral
/// reproduction. Compound identifiers and project domain identifiers are always blocked.
pub(super) fn generic(name: &str) -> bool {
    if is_compound(name) {
        return false;
    }
    if name.chars().count() < 4 {
        return true;
    }
    if is_in_generic(name) {
        return true;
    }
    is_derivational(name)
}

fn is_in_generic(name: &str) -> bool {
    GENERIC
        .split_whitespace()
        .any(|word| name.eq_ignore_ascii_case(word))
}

fn is_derivational(name: &str) -> bool {
    if !name.chars().all(|c| c.is_ascii_lowercase()) {
        return false;
    }
    if name == "oversized" {
        return false;
    }
    const SUFFIXES: &[&str] = &[
        "tions", "tion", "sions", "sion", "ments", "ment", "nesses", "ness", "ities", "ity",
        "ables", "able", "ibles", "ible", "ances", "ance", "ences", "ence", "ings", "ing", "ed",
        "ly", "ives", "ive", "ous", "ful", "less", "al",
    ];
    for suffix in SUFFIXES {
        if let Some(stem) = name.strip_suffix(suffix)
            && stem.len() >= 3
            && (is_in_generic(stem)
                || is_in_generic(&format!("{stem}e"))
                || (stem.len() >= 4
                    && stem.as_bytes()[stem.len() - 1] == stem.as_bytes()[stem.len() - 2]
                    && is_in_generic(&stem[..stem.len() - 1])))
        {
            return true;
        }
    }
    false
}

const GENERIC: &str = "
    abstract alignas alignof and args array as assert async await auto bool boolean break byte
    case catch char class clone collect const continue crate debug decimal default defer delete
    derive dict double drop dyn else enum error err except export extends extern false final
    finally float fn for foreach from func function get global goto hash hashmap hashset impl
    import in inline int integer interface into is iterator lambda len let list long loop main
    map match mod module move mut namespace native new nil none not null object ok option or
    out override package pass pathbuf duration instant arc mutex rwlock box cow btreemap btreeset
    parse create handle render print println private protected protocol pub public raise range
    read readonly ref register repr result return self set short signed sizeof some static str
    string struct super switch sync test this throw throws trait true try tuple type typedef
    typeof union unsafe unsigned use using value var vec vector virtual void volatile where
    while with write yield
    a about after all also an any are argument arguments at back be because before between body
    both buffer build by call called calls can change check client code column command common
    config count could data declaration declarations detail details did directory disk do does
    done each empty end entry example expected failed fails failure field file filename files
    find first flag following format found generic given has have help here host how id index
    input instead issue it its key kind label labels language last length limit line load local
    lookup memory message method mode name names neutral no node nothing number of on once one
    only open other output parameter parser path paths point prefix problem process project
    record reference remote report reports repository reproduction request response results
    returned returns root run sample scan search second see server should size source start
    state status store stream success symbol text than that the their them then there these
    they time title to token tool tracked two up update used user version was way we went
    were what when which who will word work working would wrong you your
    api app cli common constants errors handler handlers helper helpers lib model models
    schema schemas service services tests types util utils
    available current requested verification expected observed actual behavior commands
    outputs releases gateway gateways clients servers cluster clusters workspace workspaces
    worktree worktrees session sessions timeout timeouts diagnostic diagnostics symbols
    references definition definitions rename check checks lint lints syncs probe probes
    languages editor editors bridge bridges processes thread threads disks volume volumes
    external fallback fallbacks
    action actions connection connections condition conditions exception exceptions
    collection collections operation operations validation notification notifications
    generation integration registration documentation mutation completion assertion
    specification representation migration extension extensions conversion division permission
    element elements environment statement statements placement implement implements document
    documents comment comments replacement assignment development requirement requirements
    management correctness readiness completeness capacity severity priority authority security
    quality properties utilities visibility compatibility complexity functionality validity
    flexibility entity entities readable writable nullable iterable scalable capable acceptable
    comparable executable reliable reusable accessible possible performance clearance instance
    instances distance allowance balance maintenance sequence sequences difference differences
    presence inference evidence sentence pending holding fixed resolved needed required started
    stopped skipped closed opened added removed saved defined named configured connected
    disconnected received sent blocked wrapped mapped dropped emitted committed pushed pulled
    merged cloned deployed verified deeply newly strictly typically normally naturally recently
    previously currently correctly properly active relative derivative primitive positive
    negative effective collective recursive interactive selective continuous asynchronous
    synchronous obvious anonymous rigorous useful successful helpful powerful careful stateless
    headless endless harmless useless internal literal canonical signal signals initial optional
    general central partial formal visual conditional original practical functional manual
";

#[cfg(test)]
mod tests {
    use super::generic;

    #[test]
    fn ordinary_english_prose_words_are_generic_while_compound_and_domain_names_are_not() {
        for word in [
            "available",
            "current",
            "requested",
            "verification",
            "expected",
            "observed",
            "actual",
            "behavior",
            "diagnostic",
            "diagnostics",
            "workspace",
            "worktree",
        ] {
            assert!(generic(word), "`{word}` should be recognized as generic");
        }
        for blocked in [
            "sample_record",
            "SampleRecord",
            "sampleRecord",
            "SAMPLE_RECORD",
            "oversized",
            "tako",
        ] {
            assert!(!generic(blocked), "`{blocked}` must remain non-generic");
        }
    }
}
