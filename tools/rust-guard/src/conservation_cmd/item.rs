//! Inventory item model for the `conservation` gate: identity,
//! normalised signature, cfg predicate, test attributes and the
//! token-normalised body fingerprint.

use proc_macro2::TokenStream;
use quote::ToTokens;
use std::hash::Hasher;
use syn::spanned::Spanned;
use syn::{ImplItem, Item as SynItem, ItemMacro};

/// What kind of test attribution an item carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestKind {
    None,
    Plain,
    Tokio,
    /// `proptest!` macro invocation.
    PropTest,
    /// A `mod` that is a loom model (`cfg(loom)` or loom-named).
    Loom,
}

/// Test-related attributes of one item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestInfo {
    pub kind: TestKind,
    /// `#[ignore]` present.
    pub ignored: bool,
    /// The `#[ignore = "..."]` reason, when given.
    pub ignore_reason: Option<String>,
}

impl Default for TestInfo {
    fn default() -> Self {
        Self {
            kind: TestKind::None,
            ignored: false,
            ignore_reason: None,
        }
    }
}

impl TestInfo {
    pub fn is_test(&self) -> bool {
        self.kind != TestKind::None
    }
}

/// One tracked item of one revision.
#[derive(Debug, Clone)]
pub struct Item {
    /// Package name (from the target root).
    pub package: String,
    /// fn / struct / enum / union / trait / impl-member / static /
    /// const / type / mod / macro.
    pub kind: &'static str,
    /// Item name (for `proptest!` the macro name).
    pub name: String,
    /// Full logical path: `<pkg>::<target>::<module path>::<name>`
    /// (impl members use `<...>::<Self>::<name>`).
    pub logical: String,
    /// Enclosing type/trait name for associated items (None otherwise).
    pub enclosing: Option<String>,
    /// Normalised `impl` head for impl members ("" otherwise).
    pub impl_head: String,
    /// Normalised signature tokens ("" for non-fn items).
    pub signature: String,
    /// Normalised cfg predicate text, if any.
    pub cfg: Option<String>,
    pub test: TestInfo,
    /// Hex fingerprint of the token-normalised body.
    pub fingerprint: String,
    pub path: String,
    pub line: usize,
}

/// Identity used for the primary (exact) pairing.
pub fn identity(it: &Item) -> String {
    format!(
        "{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}",
        it.package,
        it.kind,
        it.logical,
        it.enclosing.as_deref().unwrap_or(""),
        it.impl_head,
        it.name
    )
}

/// Weaker key for move pairing leftovers.
pub fn weak_key(it: &Item) -> String {
    format!(
        "{}\u{1}{}\u{1}{}\u{1}{}",
        it.package,
        it.kind,
        it.enclosing.as_deref().unwrap_or(""),
        it.name
    )
}

/// Key of the declared-move pass: [`weak_key`] without the package, so a
/// `[[moved]]` entry may relocate an item into another package.
pub fn declared_key(it: &Item) -> String {
    format!(
        "{}\u{1}{}\u{1}{}",
        it.kind,
        it.enclosing.as_deref().unwrap_or(""),
        it.name
    )
}

// ---------- extraction ----------

/// Extracts inventory items from one parsed file.
pub struct Extractor<'a> {
    pub package: &'a str,
    pub path: &'a str,
    pub logical_prefix: String,
    /// cfg predicate inherited from the enclosing modules; items with no
    /// `#[cfg]` of their own take this one, both present compose with
    /// `&&` (finding 7: cfg predicates must propagate down the tree).
    pub inherited_cfg: Option<String>,
}

impl Extractor<'_> {
    /// Records every item in `items` (top level of a file or module body).
    pub fn extract(&self, items: &[SynItem], out: &mut Vec<Item>) {
        for item in items {
            match item {
                SynItem::Mod(m) => {
                    let mod_cfg = combine_cfg(self.inherited_cfg.as_deref(), attr_cfg(&m.attrs));
                    if let Some((_, body)) = &m.content {
                        let inner = Extractor {
                            package: self.package,
                            path: self.path,
                            logical_prefix: format!("{}::{}", self.logical_prefix, m.ident),
                            inherited_cfg: mod_cfg,
                        };
                        inner.extract(body, out);
                    }
                    out.push(self.plain(
                        "mod",
                        &m.ident.to_string(),
                        tokens_of(m.to_token_stream()),
                        attr_cfg(&m.attrs),
                        test_of_mod(m),
                        m.span().start().line,
                    ));
                }
                SynItem::Fn(f) => {
                    out.push(Item {
                        kind: "fn",
                        signature: tokens_of(f.sig.to_token_stream()),
                        fingerprint: hash_tokens(&f.block.to_token_stream()),
                        ..self.plain(
                            "fn",
                            &f.sig.ident.to_string(),
                            tokens_of(f.sig.to_token_stream()),
                            attr_cfg(&f.attrs),
                            test_of_attrs(&f.attrs),
                            f.span().start().line,
                        )
                    });
                }
                SynItem::Struct(i) => self.item_syn("struct", i, &i.attrs, &i.ident, out),
                SynItem::Enum(i) => self.item_syn("enum", i, &i.attrs, &i.ident, out),
                SynItem::Union(i) => self.item_syn("union", i, &i.attrs, &i.ident, out),
                SynItem::Trait(i) => {
                    out.push(self.plain(
                        "trait",
                        &i.ident.to_string(),
                        tokens_of(i.to_token_stream()),
                        attr_cfg(&i.attrs),
                        test_of_attrs(&i.attrs),
                        i.span().start().line,
                    ));
                    self.assoc_items(
                        &i.ident.to_string(),
                        "",
                        i.items.iter().map(trait_member),
                        out,
                    );
                }
                SynItem::Type(i) => self.item_syn("type", i, &i.attrs, &i.ident, out),
                SynItem::Const(i) => self.item_syn("const", i, &i.attrs, &i.ident, out),
                SynItem::Static(i) => self.item_syn("static", i, &i.attrs, &i.ident, out),
                SynItem::Macro(im) if macro_name(im) == "proptest" => {
                    out.push(self.plain(
                        "macro",
                        "proptest!",
                        tokens_of(im.mac.tokens.clone()),
                        attr_cfg(&im.attrs),
                        TestInfo {
                            kind: TestKind::PropTest,
                            ..TestInfo::default()
                        },
                        im.span().start().line,
                    ));
                }
                SynItem::Impl(i) => {
                    let head = impl_head(i);
                    let self_name = impl_self_name(i);
                    self.assoc_items(&self_name, &head, i.items.iter().map(impl_member), out);
                }
                _ => {}
            }
        }
    }

    /// Plain top-level item: identity only, no associated semantics.
    /// The fingerprint keeps identifiers/literals/tokens but not the
    /// visibility, so a widening alone is not a body change.
    fn item_syn<T>(
        &self,
        kind: &'static str,
        node: &T,
        attrs: &[syn::Attribute],
        name: &syn::Ident,
        out: &mut Vec<Item>,
    ) where
        T: ToTokens + syn::spanned::Spanned,
    {
        out.push(self.plain(
            kind,
            &name.to_string(),
            tokens_of(strip_vis(node.to_token_stream())),
            attr_cfg(attrs),
            test_of_attrs(attrs),
            node.span().start().line,
        ));
    }

    /// Builds a non-associated item; the fingerprint hashes the body
    /// with the visibility and module path stripped.
    fn plain(
        &self,
        kind: &'static str,
        name: &str,
        body_tokens: String,
        cfg: Option<String>,
        test: TestInfo,
        line: usize,
    ) -> Item {
        Item {
            package: self.package.to_owned(),
            kind,
            name: name.to_owned(),
            logical: format!("{}::{name}", self.logical_prefix),
            enclosing: None,
            impl_head: String::new(),
            signature: String::new(),
            cfg: combine_cfg(self.inherited_cfg.as_deref(), cfg),
            test,
            fingerprint: hash_text(&body_tokens),
            path: self.path.to_owned(),
            line,
        }
    }

    /// Records associated items (impl or trait members). Duplicate
    /// members across split impl blocks collapse later in `dedup`.
    fn assoc_items<I>(&self, enclosing: &str, impl_head: &str, members: I, out: &mut Vec<Item>)
    where
        I: IntoIterator<Item = Option<Member>>,
    {
        for member in members {
            let Some(m) = member else { continue };
            out.push(Item {
                package: self.package.to_owned(),
                kind: m.kind,
                name: m.name.clone(),
                logical: format!("{}::{}::{}", self.logical_prefix, enclosing, m.name),
                enclosing: Some(enclosing.to_owned()),
                impl_head: impl_head.to_owned(),
                signature: tokens_of(m.sig.clone()),
                cfg: combine_cfg(self.inherited_cfg.as_deref(), m.cfg),
                test: m.test,
                fingerprint: hash_tokens(&m.body),
                path: self.path.to_owned(),
                line: m.line,
            });
        }
    }
}

/// One associated (impl/trait) member, extracted.
pub struct Member {
    pub kind: &'static str,
    pub name: String,
    pub sig: TokenStream,
    pub body: TokenStream,
    pub cfg: Option<String>,
    pub test: TestInfo,
    pub line: usize,
}

fn impl_member(m: &ImplItem) -> Option<Member> {
    let (name, sig, body, attrs) = match m {
        ImplItem::Fn(f) => (
            f.sig.ident.to_string(),
            f.sig.to_token_stream(),
            f.block.to_token_stream(),
            &f.attrs,
        ),
        ImplItem::Const(c) => (
            c.ident.to_string(),
            c.to_token_stream(),
            c.expr.to_token_stream(),
            &c.attrs,
        ),
        ImplItem::Type(t) => (
            t.ident.to_string(),
            t.to_token_stream(),
            // The value of an associated type IS its body: fingerprint the
            // type expression so a changed impl type is a finding even
            // though the method bodies did not move.
            t.ty.to_token_stream(),
            &t.attrs,
        ),
        _ => return None,
    };
    Some(Member {
        kind: "impl-member",
        name,
        sig,
        body,
        cfg: attr_cfg(attrs),
        test: test_of_attrs(attrs),
        line: span_line(m),
    })
}

fn trait_member(m: &syn::TraitItem) -> Option<Member> {
    let (name, sig, body, attrs) = match m {
        syn::TraitItem::Fn(f) => (
            f.sig.ident.to_string(),
            f.sig.to_token_stream(),
            f.default
                .as_ref()
                .map(|b| b.to_token_stream())
                .unwrap_or_default(),
            &f.attrs,
        ),
        syn::TraitItem::Const(c) => (
            c.ident.to_string(),
            c.to_token_stream(),
            c.default
                .as_ref()
                .map(|e| e.1.to_token_stream())
                .unwrap_or_default(),
            &c.attrs,
        ),
        syn::TraitItem::Type(t) => (
            t.ident.to_string(),
            t.to_token_stream(),
            // Same for trait-member type defaults; a bare declaration
            // (no default) has an empty body, bounds live in the sig.
            t.default
                .as_ref()
                .map(|(_, ty)| ty.to_token_stream())
                .unwrap_or_default(),
            &t.attrs,
        ),
        _ => return None,
    };
    Some(Member {
        kind: "impl-member",
        name,
        sig,
        body,
        cfg: attr_cfg(attrs),
        test: test_of_attrs(attrs),
        line: span_line(m),
    })
}

// ---------- attribute helpers ----------

/// Normalised `#[cfg(...)]` predicate text, if present.
pub fn attr_cfg(attrs: &[syn::Attribute]) -> Option<String> {
    attrs.iter().find(|a| a.path().is_ident("cfg")).map(|a| {
        a.parse_args_with(|input: syn::parse::ParseStream| {
            input.parse::<proc_macro2::TokenStream>()
        })
        .map(tokens_of)
        .unwrap_or_default()
    })
}

/// Test attribution from a list of attributes.
pub fn test_of_attrs(attrs: &[syn::Attribute]) -> TestInfo {
    let mut info = TestInfo::default();
    for a in attrs {
        let path = a.path();
        let first = path.segments.first().map(|s| s.ident.to_string());
        let last = path.segments.last().map(|s| s.ident.to_string());
        if path.segments.len() == 1 && last.as_deref() == Some("test") {
            info.kind = TestKind::Plain;
        } else if first.as_deref() == Some("tokio") && last.as_deref() == Some("test") {
            info.kind = TestKind::Tokio;
        }
        if path.is_ident("ignore") {
            info.ignored = true;
            info.ignore_reason = a.parse_args::<syn::LitStr>().ok().map(|l| l.value());
        }
    }
    info
}

/// A loom model: `cfg` mentioning loom, or a loom-named module.
fn test_of_mod(m: &syn::ItemMod) -> TestInfo {
    let cfg_loom = attr_cfg(&m.attrs)
        .map(|c| c.contains("loom"))
        .unwrap_or(false);
    let name_loom = m.ident.to_string().contains("loom");
    if cfg_loom || name_loom {
        TestInfo {
            kind: TestKind::Loom,
            ..TestInfo::default()
        }
    } else {
        TestInfo::default()
    }
}

// ---------- token normalisation ----------

/// Token text with uniform spacing: whitespace and comments are gone,
/// identifiers/literals/punctuation stay.
pub fn tokens_of(ts: TokenStream) -> String {
    ts.to_string()
}

/// Normalised impl head: trait path + `for` + self type. Where clauses
/// and generics are excluded: they do not change member identity.
pub fn impl_head(i: &syn::ItemImpl) -> String {
    let mut ts = TokenStream::new();
    if let Some((_, t, _)) = &i.trait_ {
        ts.extend(t.to_token_stream());
    }
    ts.extend(std::iter::once(proc_macro2::TokenTree::Ident(
        proc_macro2::Ident::new("for", proc_macro2::Span::call_site()),
    )));
    ts.extend(i.self_ty.to_token_stream());
    tokens_of(ts)
}

/// Last path segment of an impl self type ("" when unrecognised).
pub fn impl_self_name(i: &syn::ItemImpl) -> String {
    if let syn::Type::Path(p) = i.self_ty.as_ref() {
        p.path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default()
    } else {
        String::new()
    }
}

fn macro_name(im: &ItemMacro) -> String {
    im.mac
        .path
        .segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_default()
}

/// Strips a leading visibility (`pub`, `pub(...)`) from a token stream.
fn strip_vis(ts: TokenStream) -> TokenStream {
    let mut rest: Vec<proc_macro2::TokenTree> = ts.into_iter().collect();
    if let Some(proc_macro2::TokenTree::Ident(id)) = rest.first() {
        if id == "pub" {
            rest.remove(0);
            if let Some(proc_macro2::TokenTree::Group(g)) = rest.first() {
                if g.delimiter() == proc_macro2::Delimiter::Parenthesis {
                    rest.remove(0);
                }
            }
        }
    }
    rest.into_iter().collect()
}

fn hash_tokens(ts: &TokenStream) -> String {
    hash_text(&ts.to_string())
}

/// Composes an inherited cfg predicate with the item's own: no parent
/// means the child stands alone, no child means inheritance, both chain
/// with `&&` (the composed text is only compared for equality).
pub fn combine_cfg(parent: Option<&str>, child: Option<String>) -> Option<String> {
    match (parent, child) {
        (None, c) => c,
        (Some(p), None) => Some(p.to_owned()),
        (Some(p), Some(c)) => Some(format!("{p} && {c}")),
    }
}

fn hash_text(text: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write(text.as_bytes());
    format!("{:016x}", h.finish())
}

fn span_line<T: syn::spanned::Spanned>(t: &T) -> usize {
    t.span().start().line
}
