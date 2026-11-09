//! Token-aware guard for the runtime-free compiler decision. Production source
//! is scanned for forbidden std/core/alloc paths and imports (including aliases,
//! globs, and re-exports), every extern crate declaration, environment macros,
//! and Command::new calls; strings and comments are ignored. Production
//! dependencies remain pinned to the designed three.
use std::fs;
use std::path::{Path, PathBuf};

use super::purity_scanner::{scan_source, Finding};

struct ProductionSource {
    path: PathBuf,
    source: String,
}

const ALLOWED_DEPENDENCIES: [&str; 3] = ["shamir-wasm-admission", "wasm-encoder", "thiserror"];

fn production_sources(dir: &Path, out: &mut Vec<ProductionSource>) {
    for entry in fs::read_dir(dir).expect("source directory is readable") {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            if path
                .file_name()
                .is_some_and(|name| name == "tests" || name == "testsupport")
            {
                continue;
            }
            production_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(ProductionSource {
                source: fs::read_to_string(&path).expect("source file is readable"),
                path,
            });
        }
    }
}

fn assert_has_finding(source: &str, reason: &str) {
    let findings = scan_source(source);
    assert!(
        findings.iter().any(|finding| finding.reason == reason),
        "scanner did not report {reason}; found {findings:?}"
    );
}

fn assert_clean(source: &str) {
    let findings = scan_source(source);
    assert!(findings.is_empty(), "unexpected findings: {findings:?}");
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
        if let Some(Finding { line, reason }) = scan_source(&source.source).into_iter().next() {
            panic!("{}:{line}: {reason}", source.path.display());
        }
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

#[test]
fn scanner_rejects_grouped_and_brief_import_bypasses() {
    assert_has_finding(
        r#"
use {std as s};
"#,
        "standard-library root alias",
    );
    assert_has_finding(
        r#"
use {std::{self as s}};
"#,
        "standard-library root alias",
    );
    assert_has_finding(
        r#"
use std::{self as s};
"#,
        "standard-library root alias",
    );
    assert_has_finding(
        r#"
use ::std as s;
"#,
        "standard-library root alias",
    );
    assert_has_finding(
        r#"
use {std::*};
"#,
        "standard-library root glob import",
    );
    assert_has_finding(
        r#"
use {std::{process, fs}};
"#,
        "forbidden standard-library module import",
    );
    assert_has_finding(
        r#"
pub use {::std as s};
"#,
        "standard-library root alias",
    );
    assert_has_finding(
        r#"
extern crate std as x;
"#,
        "extern crate declaration",
    );
}

#[test]
fn scanner_rejects_standard_library_root_aliases() {
    assert_has_finding(
        r#"
use std as s;
"#,
        "standard-library root alias",
    );
    assert_has_finding(
        r#"
use core::{self as c};
"#,
        "standard-library root alias",
    );
    assert_has_finding(
        r#"
use alloc::{self as a};
"#,
        "standard-library root alias",
    );
    assert_has_finding(
        r#"
use ::std::{self as absolute};
"#,
        "standard-library root alias",
    );
    assert_has_finding(
        r#"
pub use ::core::{self as exported_core};
"#,
        "standard-library root alias",
    );
}

#[test]
fn scanner_rejects_forbidden_module_import_bypasses() {
    assert_has_finding(
        r#"
use std::process;
"#,
        "forbidden standard-library module import",
    );
    assert_has_finding(
        r#"
use std::{process, fs};
"#,
        "forbidden standard-library module import",
    );
    assert_has_finding(
        r#"
use std::{io, net::{self as network}};
"#,
        "forbidden standard-library module import",
    );
    assert_has_finding(
        r#"
use std::env as environment;
"#,
        "forbidden standard-library module import",
    );
    assert_has_finding(
        r#"
use std::thread::*;
"#,
        "forbidden standard-library module import",
    );
    assert_has_finding(
        r#"
use ::std::process as exported_process;
"#,
        "forbidden standard-library module import",
    );
    assert_has_finding(
        r#"
use core::thread;
"#,
        "forbidden standard-library module import",
    );
    assert_has_finding(
        r#"
use alloc::fs::*;
"#,
        "forbidden standard-library module import",
    );
    assert_has_finding(
        r#"
pub use std::{r#fs as raw_fs};
"#,
        "forbidden standard-library module import",
    );
    assert_has_finding(
        r#"
use ::core::*;
"#,
        "standard-library root glob import",
    );
    assert_has_finding(
        r#"
pub use std as exported_std;
"#,
        "standard-library root alias",
    );
}

#[test]
fn scanner_rejects_all_extern_crate_declarations() {
    assert_has_finding(
        r#"
extern crate std as s;
"#,
        "extern crate declaration",
    );
    assert_has_finding(
        r#"
extern crate anything;
"#,
        "extern crate declaration",
    );
}

#[test]
fn scanner_rejects_environment_macro_bypasses() {
    let source = r#"
fn values() {
    let first = env!("BUILD_SETTING");
    let second = option_env!("OPTIONAL_SETTING");
    let qualified = std::env!("QUALIFIED_SETTING");
    let also_qualified = ::std::option_env!("QUALIFIED_OPTION");
}
"#;
    assert_eq!(
        scan_source(source)
            .iter()
            .filter(|finding| finding.reason == "environment macro invocation")
            .count(),
        4
    );
}

#[test]
fn scanner_rejects_command_constructor_guard() {
    let source = r#"
fn spawn() {
    Command :: /* split tokens */ new("tool");
}
"#;
    assert_has_finding(source, "Command::new invocation");
}

#[test]
fn scanner_handles_lexical_decoys_and_keeps_separated_tokens_visible() {
    let allowed = r####"
// std::process::exit(1); env!("NO");
/* std::fs::read("no"); /* nested std::net::TcpStream */ */
const A: &str = "std::thread::spawn env!(\"NO\")";
const B: &[u8] = br###"std::env! option_env!(\"NO\")"###;
const C: &str = r###"std::process"###;
const D: char = 's';
let byte_string = b"std::fs env!(\"NO\")";
let c_string = c"std::net option_env!(\"NO\")";
let lifetime: &'static str = "allowed";
let raw_ident = r#process;
use ::std::fmt;
use crate::std::process::LocalProcess;
use self::std::thread::LocalThread;
use super::std::fs::LocalFs;
use std::{io::{self as io_self}, fmt::{Debug, Display}};
use core::iter;
use alloc::vec::Vec;
"####;
    assert_clean(allowed);
    assert_clean(
        r#"
use std::{self, io::{Read, Write}};
use ::core::{self, fmt::Debug};
"#,
    );

    assert_has_finding(
        r#"
std /* comment */ ::
process::exit(1);
"#,
        "forbidden standard-library module path",
    );
    assert_has_finding(
        r#"
:: core :: /* comment */ fs :: read("x");
"#,
        "forbidden standard-library module path",
    );
    assert_has_finding(
        r#"
loop { break std::thread::yield_now(); }
"#,
        "forbidden standard-library module path",
    );
    assert_has_finding(
        r#"
loop { break 'leave_now ::std::thread::yield_now(); }
"#,
        "forbidden standard-library module path",
    );
    assert_has_finding(
        r#"
let _ = match ::alloc::env::var("x") { _ => () };
"#,
        "forbidden standard-library module path",
    );
    let following_literals = r#"
let _ = 'x'; std::thread::yield_now();
let _lifetime: &'static str = "x"; core::env::var("x");
let r#process = (); alloc::net::connect();
"#;
    assert_eq!(
        scan_source(following_literals)
            .iter()
            .filter(|finding| finding.reason == "forbidden standard-library module path")
            .count(),
        3
    );
}
