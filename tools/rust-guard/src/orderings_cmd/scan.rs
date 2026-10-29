use serde::Deserialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Expr, ExprCall, ExprMethodCall, ExprPath, Item};

#[derive(Deserialize)]
struct Pins {
    pin: Vec<Pin>,
}
#[derive(Deserialize)]
struct Pin {
    path: String,
    item: String,
    #[serde(default = "production")]
    branch: String,
    owner: String,
    reason: String,
    calls: Vec<CallPin>,
    require_fence: Option<String>,
}
#[derive(Deserialize)]
struct CallPin {
    method: String,
    min: String,
}
fn production() -> String {
    "production".into()
}
const ATOMIC: &[&str] = &[
    "fetch_add",
    "fetch_sub",
    "fetch_or",
    "fetch_and",
    "fetch_xor",
    "fetch_nand",
    "fetch_max",
    "fetch_min",
    "load",
    "store",
    "swap",
    "compare_exchange",
    "compare_exchange_weak",
    "fetch_update",
];

pub fn run(repo: &Path, pins_path: Option<&str>) -> u8 {
    let config = pins_path
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".rust-guard/atomic_orderings.toml"));
    let config = if config.is_absolute() {
        config
    } else {
        repo.join(config)
    };
    let pins = match fs::read_to_string(&config)
        .and_then(|s| toml::from_str::<Pins>(&s).map_err(std::io::Error::other))
    {
        Ok(v) => v,
        Err(e) => {
            eprintln!("rust-guard: error: cannot load {}: {e}", config.display());
            return 1;
        }
    };
    let mut report = crate::common::finding::Report::new();
    for pin in pins.pin {
        let source = match fs::read_to_string(repo.join(&pin.path)) {
            Ok(s) => s,
            Err(e) => {
                report.add(
                    &pin.path,
                    1,
                    "MISSING_ITEM",
                    format!("cannot read item {} owned by {}: {e}", pin.item, pin.owner),
                );
                continue;
            }
        };
        let file = match syn::parse_file(&source) {
            Ok(f) => f,
            Err(e) => {
                report.add(
                    &pin.path,
                    e.span().start().line,
                    "PARSE_FAILED",
                    e.to_string(),
                );
                continue;
            }
        };
        let mut aliases = BTreeSet::new();
        for item in &file.items {
            if let Item::Use(item_use) = item {
                collect_atomic_aliases(&item_use.tree, &mut aliases);
            }
        }
        let mut scanner = Scanner {
            aliases: &aliases,
            wanted: &pin.item,
            branch: &pin.branch,
            calls: &pin.calls,
            fence: pin.require_fence.as_deref(),
            path: &pin.path,
            report: &mut report,
            matched: false,
        };
        for item in &file.items {
            scanner.item(item);
        }
        if !scanner.matched {
            scanner.report.add(
                &pin.path,
                1,
                "MISSING_ITEM",
                format!(
                    "pinned item {} not found in {} branch: {}",
                    pin.item, pin.branch, pin.reason
                ),
            );
        }
    }
    let code = crate::common::finding::exit_code(&report);
    if code != 0 {
        report.print();
    } else {
        eprintln!("rust-guard: 0 finding(s)");
    }
    code
}

struct Scanner<'a> {
    aliases: &'a BTreeSet<String>,
    wanted: &'a str,
    branch: &'a str,
    calls: &'a [CallPin],
    fence: Option<&'a str>,
    path: &'a str,
    report: &'a mut crate::common::finding::Report,
    matched: bool,
}
impl Scanner<'_> {
    fn item(&mut self, item: &Item) {
        match item {
            Item::Fn(f) if f.sig.ident == self.wanted => self.scan(&f.block, &f.attrs),
            Item::Impl(i) => {
                let ty = type_name(&i.self_ty);
                for it in &i.items {
                    if let syn::ImplItem::Fn(f) = it {
                        if self.wanted == format!("{ty}::{}", f.sig.ident)
                            || (self.wanted == format!("{ty}::drop") && f.sig.ident == "drop")
                        {
                            self.scan(&f.block, &f.attrs);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    fn scan(&mut self, body: &syn::Block, attrs: &[syn::Attribute]) {
        let cfg_attrs: Vec<_> = attrs
            .iter()
            .filter(|attr| attr.path().is_ident("cfg"))
            .collect();
        if cfg_attrs
            .iter()
            .any(|attr| !cfg_selects_branch(&attr.meta, self.branch))
        {
            return;
        }
        if self.branch == "loom" && !cfg_attrs.iter().any(|attr| cfg_requires_loom(&attr.meta)) {
            return;
        }
        if self.branch == "production" && cfg_attrs.iter().any(|attr| cfg_requires_loom(&attr.meta))
        {
            return;
        }
        self.matched = true;
        let mut calls = Calls {
            aliases: self.aliases,
            pins: self.calls,
            path: self.path,
            report: self.report,
            seen: vec![false; self.calls.len()],
            fence: self.fence,
            has_fence: false,
            branch: self.branch,
        };
        calls.visit_block(body);
        for (index, call) in self.calls.iter().enumerate() {
            if !calls.seen[index] {
                calls.report.add(
                    self.path,
                    1,
                    "STALE_PIN",
                    format!("pinned call {} no longer exists", call.method),
                );
            }
        }
        if let Some(required) = self.fence {
            if !calls.has_fence {
                calls.report.add(
                    self.path,
                    1,
                    "MISSING_FENCE",
                    format!("required fence({required}) missing"),
                );
            }
        }
    }
}
fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(n) => &n.attrs,
        syn::Item::Enum(n) => &n.attrs,
        syn::Item::ExternCrate(n) => &n.attrs,
        syn::Item::Fn(n) => &n.attrs,
        syn::Item::ForeignMod(n) => &n.attrs,
        syn::Item::Impl(n) => &n.attrs,
        syn::Item::Macro(n) => &n.attrs,
        syn::Item::Mod(n) => &n.attrs,
        syn::Item::Static(n) => &n.attrs,
        syn::Item::Struct(n) => &n.attrs,
        syn::Item::Trait(n) => &n.attrs,
        syn::Item::TraitAlias(n) => &n.attrs,
        syn::Item::Type(n) => &n.attrs,
        syn::Item::Union(n) => &n.attrs,
        syn::Item::Use(n) => &n.attrs,
        _ => &[],
    }
}
fn type_name(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(p) => p
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default(),
        _ => String::new(),
    }
}
fn meta_list(tokens: &proc_macro2::TokenStream) -> Vec<syn::Meta> {
    syn::parse::Parser::parse2(
        syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
        tokens.clone(),
    )
    .map(|v| v.into_iter().collect())
    .unwrap_or_default()
}
/// Three-valued cfg evaluation with only `loom` known; any other predicate is
/// unknown (`None`), so a cfg is excluded only when it is false for every
/// assignment of the unknowns.
fn cfg_eval(meta: &syn::Meta, loom: bool) -> Option<bool> {
    match meta {
        syn::Meta::Path(path) if path.is_ident("loom") => Some(loom),
        syn::Meta::List(list) if list.path.is_ident("not") => {
            let nested = meta_list(&list.tokens);
            match nested.as_slice() {
                [only] => cfg_eval(only, loom).map(|v| !v),
                _ => None,
            }
        }
        syn::Meta::List(list) if list.path.is_ident("all") || list.path.is_ident("cfg") => {
            let mut out = Some(true);
            for item in meta_list(&list.tokens) {
                match cfg_eval(&item, loom) {
                    Some(false) => return Some(false),
                    None => out = None,
                    Some(true) => {}
                }
            }
            out
        }
        syn::Meta::List(list) if list.path.is_ident("any") => {
            let mut out = Some(false);
            for item in meta_list(&list.tokens) {
                match cfg_eval(&item, loom) {
                    Some(true) => return Some(true),
                    None => out = None,
                    Some(false) => {}
                }
            }
            out
        }
        _ => None,
    }
}
/// Whether code under `meta` can be compiled in `branch`.
fn cfg_selects_branch(meta: &syn::Meta, branch: &str) -> bool {
    cfg_eval(meta, branch == "loom") != Some(false)
}
/// Whether `meta` is compiled only under loom (false whenever `loom` is off).
fn cfg_requires_loom(meta: &syn::Meta) -> bool {
    cfg_eval(meta, false) == Some(false)
}
fn collect_atomic_aliases(tree: &syn::UseTree, aliases: &mut BTreeSet<String>) {
    match tree {
        syn::UseTree::Path(path) => collect_atomic_aliases(&path.tree, aliases),
        syn::UseTree::Group(group) => {
            for item in &group.items {
                collect_atomic_aliases(item, aliases);
            }
        }
        syn::UseTree::Rename(rename) if rename.ident.to_string().starts_with("Atomic") => {
            aliases.insert(rename.rename.to_string());
        }
        _ => {}
    }
}

struct Calls<'a> {
    aliases: &'a BTreeSet<String>,
    pins: &'a [CallPin],
    path: &'a str,
    report: &'a mut crate::common::finding::Report,
    seen: Vec<bool>,
    fence: Option<&'a str>,
    has_fence: bool,
    branch: &'a str,
}
impl Calls<'_> {
    fn check(&mut self, method: &str, args: &[Expr], line: usize) {
        let position_set: &[usize] = match method {
            "compare_exchange" | "compare_exchange_weak" => &[2, 3],
            "fetch_update" => &[0, 1],
            "load" => &[0],
            "store" | "swap" | "fetch_add" | "fetch_sub" | "fetch_or" | "fetch_and"
            | "fetch_xor" | "fetch_nand" | "fetch_max" | "fetch_min" => &[1],
            _ => &[],
        };
        let Some((index, pin)) = self
            .pins
            .iter()
            .enumerate()
            .find(|(_, p)| p.method == method)
        else {
            if position_set
                .iter()
                .any(|&p| args.get(p).is_some_and(|e| ordering(e).is_some()))
            {
                self.report.add(
                    self.path,
                    line,
                    "UNPINNED_CALL",
                    format!("atomic method {method} has no call pin"),
                );
            }
            return;
        };
        self.seen[index] = true;
        for &position in position_set {
            let Some(expr) = args.get(position) else {
                self.report.add(
                    self.path,
                    line,
                    "NON_LITERAL_ORDERING",
                    format!("{method} is missing ordering argument at position {position}"),
                );
                continue;
            };
            match ordering(expr) {
                Some(actual) if !at_least(&actual, &pin.min, method, position) => self.report.add(
                    self.path,
                    expr.span().start().line,
                    "WEAKENED_ORDERING",
                    format!("{method} ordering {actual} is weaker than {}", pin.min),
                ),
                Some(_) => {}
                None => self.report.add(
                    self.path,
                    expr.span().start().line,
                    "NON_LITERAL_ORDERING",
                    format!("{method} ordering must be a literal Ordering::<Variant> path"),
                ),
            }
        }
    }
}
impl<'ast> Visit<'ast> for Calls<'_> {
    fn visit_stmt(&mut self, node: &'ast syn::Stmt) {
        let attrs: &[syn::Attribute] = match node {
            syn::Stmt::Item(item) => item_attrs(item),
            syn::Stmt::Macro(m) => &m.attrs,
            syn::Stmt::Local(local) => &local.attrs,
            _ => &[],
        };
        if !attrs
            .iter()
            .filter(|a| a.path().is_ident("cfg"))
            .all(|a| cfg_selects_branch(&a.meta, self.branch))
        {
            return;
        }
        visit::visit_stmt(self, node);
    }
    fn visit_expr(&mut self, node: &'ast Expr) {
        let attrs: &[syn::Attribute] = match node {
            Expr::Array(n) => &n.attrs,
            Expr::Assign(n) => &n.attrs,
            Expr::Async(n) => &n.attrs,
            Expr::Await(n) => &n.attrs,
            Expr::Binary(n) => &n.attrs,
            Expr::Block(n) => &n.attrs,
            Expr::Break(n) => &n.attrs,
            Expr::Call(n) => &n.attrs,
            Expr::Cast(n) => &n.attrs,
            Expr::Closure(n) => &n.attrs,
            Expr::Const(n) => &n.attrs,
            Expr::Continue(n) => &n.attrs,
            Expr::Field(n) => &n.attrs,
            Expr::ForLoop(n) => &n.attrs,
            Expr::Group(n) => &n.attrs,
            Expr::If(n) => &n.attrs,
            Expr::Index(n) => &n.attrs,
            Expr::Infer(n) => &n.attrs,
            Expr::Let(n) => &n.attrs,
            Expr::Lit(n) => &n.attrs,
            Expr::Loop(n) => &n.attrs,
            Expr::Macro(n) => &n.attrs,
            Expr::Match(n) => &n.attrs,
            Expr::MethodCall(n) => &n.attrs,
            Expr::Paren(n) => &n.attrs,
            Expr::Path(n) => &n.attrs,
            Expr::Range(n) => &n.attrs,
            Expr::Reference(n) => &n.attrs,
            Expr::RawAddr(n) => &n.attrs,
            Expr::Repeat(n) => &n.attrs,
            Expr::Return(n) => &n.attrs,
            Expr::Struct(n) => &n.attrs,
            Expr::Try(n) => &n.attrs,
            Expr::TryBlock(n) => &n.attrs,
            Expr::Tuple(n) => &n.attrs,
            Expr::Unary(n) => &n.attrs,
            Expr::Unsafe(n) => &n.attrs,
            Expr::Verbatim(_) => &[],
            Expr::While(n) => &n.attrs,
            Expr::Yield(n) => &n.attrs,
            _ => &[],
        };
        if !attrs
            .iter()
            .filter(|a| a.path().is_ident("cfg"))
            .all(|a| cfg_selects_branch(&a.meta, self.branch))
        {
            return;
        }
        if let Expr::Block(block) = node {
            if !block
                .attrs
                .iter()
                .filter(|a| a.path().is_ident("cfg"))
                .all(|a| cfg_selects_branch(&a.meta, self.branch))
            {
                return;
            }
        }
        visit::visit_expr(self, node);
    }
    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        let method = node.method.to_string();
        if method == "fence" {
            if node.args.first().and_then(ordering).as_deref() == self.fence {
                self.has_fence = true;
            }
        } else if ATOMIC.contains(&method.as_str()) {
            self.check(
                &method,
                &node.args.iter().cloned().collect::<Vec<_>>(),
                node.span().start().line,
            );
        }
        visit::visit_expr_method_call(self, node);
    }
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        if let Ok(parsed) = syn::parse::Parser::parse2(
            syn::punctuated::Punctuated::<Expr, syn::Token![,]>::parse_terminated,
            node.tokens.clone(),
        ) {
            for expression in parsed {
                self.visit_expr(&expression);
            }
        }
    }
    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        let name = match &*node.func {
            Expr::Path(path) => path
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default(),
            _ => String::new(),
        };
        if name == "fence" && node.args.first().and_then(ordering).as_deref() == self.fence {
            self.has_fence = true;
        } else if let Expr::Path(path) = &*node.func {
            let ty = if let Some(qself) = &path.qself {
                type_name(&qself.ty)
            } else {
                path.path
                    .segments
                    .iter()
                    .rev()
                    .nth(1)
                    .map(|segment| segment.ident.to_string())
                    .unwrap_or_default()
            };
            if (ty.starts_with("Atomic") || self.aliases.contains(&ty))
                && ATOMIC.contains(&name.as_str())
                && !node.args.is_empty()
            {
                self.check(
                    &name,
                    &node.args.iter().skip(1).cloned().collect::<Vec<_>>(),
                    node.span().start().line,
                );
            }
        }
        visit::visit_expr_call(self, node);
    }
}
fn ordering(expr: &Expr) -> Option<String> {
    if let Expr::Path(ExprPath { path, .. }) = expr {
        let segments: Vec<_> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        if segments.len() >= 2 && segments[segments.len() - 2] == "Ordering" {
            return Some(segments[segments.len() - 1].clone());
        }
    }
    None
}
fn at_least(actual: &str, min: &str, method: &str, position: usize) -> bool {
    let rank = |v: &str| match v {
        "Relaxed" => 0,
        "Acquire" | "Release" => 1,
        "AcqRel" => 2,
        "SeqCst" => 3,
        _ => -1,
    };
    let failure = (method.starts_with("compare_exchange") && position == 3)
        || (method == "fetch_update" && position == 1);
    let maximum = min == "SeqCst" || min == "AcqRel";
    if failure {
        if actual == "Release" || actual == "AcqRel" {
            return false;
        }
        let minimum = if maximum { "Acquire" } else { min };
        rank(actual) >= rank(minimum)
    } else {
        rank(actual) >= rank(min)
    }
}
