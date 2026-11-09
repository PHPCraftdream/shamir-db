//! Structural guard for the "runtime-free compiler" decision (design
//! section 6): the crate has no process, filesystem, network, environment or
//! thread access in production code, and its production dependencies are
//! exactly the three the design names.
use std::fs;
use std::path::Path;

/// Modules whose use would make the compiler depend on its host.
const FORBIDDEN_MODULES: [&str; 5] = ["process", "fs", "net", "env", "thread"];

/// The production dependencies named by the design.
const ALLOWED_DEPENDENCIES: [&str; 3] = ["shamir-wasm-admission", "wasm-encoder", "thiserror"];

fn production_sources(dir: &Path, out: &mut Vec<String>) {
    for entry in fs::read_dir(dir).expect("source directory is readable") {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "tests") {
                continue;
            }
            production_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(fs::read_to_string(&path).expect("source file is readable"));
        }
    }
}

/// Source text without `//` comments and without whitespace, so a path split
/// over lines still matches.
fn flattened(source: &str) -> String {
    source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<String>()
        .split_whitespace()
        .collect()
}

/// The module names a `use std::{...}` group imports at its top level.
fn grouped_std_modules(flat: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = flat;
    while let Some(start) = rest.find("std::{") {
        let body = &rest[start + "std::{".len()..];
        let mut depth = 1usize;
        let mut item = String::new();
        for ch in body.chars() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            if depth == 1 && ch == ',' {
                found.push(std::mem::take(&mut item));
            } else if depth >= 1 {
                item.push(ch);
            }
        }
        found.push(item);
        rest = body;
    }
    found
}

#[test]
fn production_sources_never_touch_process_fs_net_env_or_thread() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    production_sources(&root, &mut sources);
    assert!(
        sources.len() > 50,
        "the source walk found {}",
        sources.len()
    );
    for source in &sources {
        let flat = flattened(source);
        for module in FORBIDDEN_MODULES {
            assert!(
                !flat.contains(&format!("std::{module}")),
                "production code uses std::{module}"
            );
        }
        for item in grouped_std_modules(&flat) {
            let head = item.split("::").next().unwrap_or("");
            assert!(
                !FORBIDDEN_MODULES.contains(&head),
                "production code imports std::{{{item}}}"
            );
        }
        assert!(
            !flat.contains("Command::new"),
            "production code spawns a command"
        );
    }
}

#[test]
fn production_dependencies_are_exactly_the_designed_three() {
    let manifest = include_str!("../../../Cargo.toml");
    let section = manifest
        .split("[dependencies]")
        .nth(1)
        .expect("manifest has a [dependencies] section")
        .split("\n[")
        .next()
        .expect("section text");
    let mut names: Vec<&str> = section
        .lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name.trim()))
        .collect();
    names.sort_unstable();
    let mut expected = ALLOWED_DEPENDENCIES.to_vec();
    expected.sort_unstable();
    assert_eq!(names, expected);
}
