#[derive(Clone, Debug, PartialEq, Eq)]
enum TokenKind {
    Ident { text: String, raw: bool },
    Punct(char),
    Literal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Token {
    kind: TokenKind,
    line: usize,
}

impl Token {
    fn ident(&self) -> Option<(&str, bool)> {
        match &self.kind {
            TokenKind::Ident { text, raw } => Some((text, *raw)),
            _ => None,
        }
    }

    fn punct(&self, punct: char) -> bool {
        self.kind == TokenKind::Punct(punct)
    }

    fn keyword(&self, keyword: &str) -> bool {
        self.ident()
            .is_some_and(|(text, raw)| !raw && text == keyword)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Finding {
    pub(super) line: usize,
    pub(super) reason: &'static str,
}

const ROOT_CRATES: [&str; 3] = ["std", "core", "alloc"];
const FORBIDDEN_MODULES: [&str; 5] = ["process", "fs", "net", "env", "thread"];

pub(super) fn scan_source(source: &str) -> Vec<Finding> {
    let tokens = tokenize(source);
    let mut findings = Vec::new();

    for (index, token) in tokens.iter().enumerate() {
        if token.keyword("extern")
            && tokens
                .get(index + 1)
                .is_some_and(|next| next.keyword("crate"))
        {
            findings.push(Finding {
                line: token.line,
                reason: "extern crate declaration",
            });
        }

        if token
            .ident()
            .is_some_and(|(name, _)| name == "env" || name == "option_env")
            && tokens.get(index + 1).is_some_and(|next| next.punct('!'))
        {
            findings.push(Finding {
                line: token.line,
                reason: "environment macro invocation",
            });
        }

        if token.ident().is_some_and(|(name, _)| name == "Command")
            && tokens.get(index + 1).is_some_and(|next| next.punct(':'))
            && tokens.get(index + 2).is_some_and(|next| next.punct(':'))
            && tokens
                .get(index + 3)
                .is_some_and(|next| next.ident().is_some_and(|(name, _)| name == "new"))
        {
            findings.push(Finding {
                line: token.line,
                reason: "Command::new invocation",
            });
        }

        let Some((root, _)) = token.ident() else {
            continue;
        };
        if is_path_root(&tokens, index)
            && ROOT_CRATES.contains(&root)
            && tokens.get(index + 1).is_some_and(|next| next.punct(':'))
            && tokens.get(index + 2).is_some_and(|next| next.punct(':'))
            && tokens.get(index + 3).is_some_and(|next| {
                next.ident()
                    .is_some_and(|(module, _)| FORBIDDEN_MODULES.contains(&module))
            })
        {
            findings.push(Finding {
                line: token.line,
                reason: "forbidden standard-library module path",
            });
        }
    }

    for index in 0..tokens.len() {
        if tokens[index].keyword("use") {
            let end = use_statement_end(&tokens, index + 1);
            scan_use_tree(&tokens, index + 1, end, &[], &mut findings);
        }
    }

    findings
}

fn is_path_root(tokens: &[Token], index: usize) -> bool {
    let Some(previous) = index
        .checked_sub(1)
        .and_then(|previous| tokens.get(previous))
    else {
        return true;
    };
    if previous.punct(':')
        && index
            .checked_sub(2)
            .and_then(|previous| tokens.get(previous))
            .is_some_and(|previous| previous.punct(':'))
    {
        return index
            .checked_sub(3)
            .and_then(|previous| tokens.get(previous))
            .is_none_or(|before| {
                before.ident().is_none_or(|(name, raw)| {
                    (!raw && is_expression_keyword(name))
                        || (index >= 5
                            && tokens[index - 4].punct('\'')
                            && tokens[index - 5].keyword("break"))
                })
            });
    }
    !previous.punct('#') && (!previous.punct('.') || index >= 2 && tokens[index - 2].punct('.'))
}

fn is_expression_keyword(name: &str) -> bool {
    matches!(
        name,
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "dyn"
            | "else"
            | "extern"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "pub"
            | "return"
            | "static"
            | "trait"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "yield"
    )
}

fn tokenize(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    let mut line = 1;

    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            if bytes[index] == b'\n' {
                line += 1;
            }
            index += 1;
            continue;
        }

        if bytes[index..].starts_with(b"//") {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }

        if bytes[index..].starts_with(b"/*") {
            let start = index;
            index += 2;
            let mut depth = 1usize;
            while index < bytes.len() && depth > 0 {
                if bytes[index..].starts_with(b"/*") {
                    depth += 1;
                    index += 2;
                } else if bytes[index..].starts_with(b"*/") {
                    depth -= 1;
                    index += 2;
                } else {
                    if bytes[index] == b'\n' {
                        line += 1;
                    }
                    index += 1;
                }
            }
            debug_assert!(index > start);
            continue;
        }

        if let Some((quote, hashes)) = raw_string_start(bytes, index) {
            let start = index;
            index = raw_string_end(bytes, quote, hashes);
            line += newline_count(&bytes[start..index]);
            tokens.push(Token {
                kind: TokenKind::Literal,
                line: line - newline_count(&bytes[start..index]),
            });
            continue;
        }

        if let Some(quote) = ordinary_string_quote(bytes, index) {
            let start = index;
            index = ordinary_string_end(bytes, quote);
            let token_line = line;
            line += newline_count(&bytes[start..index]);
            tokens.push(Token {
                kind: TokenKind::Literal,
                line: token_line,
            });
            continue;
        }

        if bytes[index] == b'b' && bytes.get(index + 1) == Some(&b'\'') {
            if let Some(end) = char_literal_end(source, index + 1) {
                let token_line = line;
                line += newline_count(&bytes[index..end]);
                tokens.push(Token {
                    kind: TokenKind::Literal,
                    line: token_line,
                });
                index = end;
                continue;
            }
        }

        if bytes[index] == b'\'' {
            if let Some(end) = char_literal_end(source, index) {
                let token_line = line;
                line += newline_count(&bytes[index..end]);
                tokens.push(Token {
                    kind: TokenKind::Literal,
                    line: token_line,
                });
                index = end;
                continue;
            }
        }

        if bytes[index] == b'r'
            && bytes.get(index + 1) == Some(&b'#')
            && bytes
                .get(index + 2..)
                .and_then(|rest| std::str::from_utf8(rest).ok())
                .and_then(|rest| rest.chars().next())
                .is_some_and(is_identifier_start)
        {
            let token_line = line;
            index += 2;
            let start = index;
            while index < bytes.len() {
                let character = source[index..].chars().next().expect("valid UTF-8 source");
                if !is_identifier_continue(character) {
                    break;
                }
                index += character.len_utf8();
            }
            tokens.push(Token {
                kind: TokenKind::Ident {
                    text: source[start..index].to_owned(),
                    raw: true,
                },
                line: token_line,
            });
            continue;
        }

        let character = source[index..].chars().next().expect("valid UTF-8 source");
        if is_identifier_start(character) {
            let token_line = line;
            let start = index;
            index += character.len_utf8();
            while index < bytes.len() {
                let next = source[index..].chars().next().expect("valid UTF-8 source");
                if !is_identifier_continue(next) {
                    break;
                }
                index += next.len_utf8();
            }
            tokens.push(Token {
                kind: TokenKind::Ident {
                    text: source[start..index].to_owned(),
                    raw: false,
                },
                line: token_line,
            });
            continue;
        }

        if bytes[index].is_ascii_digit() {
            let token_line = line;
            let start = index;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            {
                index += 1;
            }
            tokens.push(Token {
                kind: TokenKind::Literal,
                line: token_line,
            });
            debug_assert!(index > start);
            continue;
        }

        tokens.push(Token {
            kind: TokenKind::Punct(character),
            line,
        });
        index += character.len_utf8();
    }

    tokens
}

fn raw_string_start(bytes: &[u8], index: usize) -> Option<(usize, usize)> {
    let prefix_len = if bytes
        .get(index..index + 2)
        .is_some_and(|prefix| prefix == b"br" || prefix == b"cr")
    {
        2
    } else if bytes.get(index) == Some(&b'r') {
        1
    } else {
        return None;
    };
    let mut quote = index + prefix_len;
    while bytes.get(quote) == Some(&b'#') {
        quote += 1;
    }
    (bytes.get(quote) == Some(&b'"')).then_some((quote, quote - index - prefix_len))
}

fn raw_string_end(bytes: &[u8], quote: usize, hashes: usize) -> usize {
    let mut index = quote + 1;
    while index < bytes.len() {
        if bytes[index] == b'"'
            && bytes
                .get(index + 1..index + 1 + hashes)
                .is_some_and(|ending| ending.iter().all(|byte| *byte == b'#'))
        {
            return index + hashes + 1;
        }
        index += 1;
    }
    bytes.len()
}

fn ordinary_string_quote(bytes: &[u8], index: usize) -> Option<usize> {
    if bytes.get(index) == Some(&b'"') {
        Some(index)
    } else if (bytes.get(index) == Some(&b'b') || bytes.get(index) == Some(&b'c'))
        && bytes.get(index + 1) == Some(&b'"')
    {
        Some(index + 1)
    } else {
        None
    }
}

fn ordinary_string_end(bytes: &[u8], quote: usize) -> usize {
    let mut index = quote + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index = (index + 2).min(bytes.len()),
            b'"' => return index + 1,
            b'\n' => index += 1,
            _ => index += 1,
        }
    }
    bytes.len()
}

fn char_literal_end(source: &str, quote: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut index = quote.checked_add(1)?;
    if bytes.get(index) == Some(&b'\\') {
        index += 1;
        match *bytes.get(index)? {
            b'x' => index += 3,
            b'u' if bytes.get(index + 1) == Some(&b'{') => {
                index += 2;
                while bytes.get(index).is_some_and(|byte| *byte != b'}') {
                    index += 1;
                }
                if bytes.get(index) != Some(&b'}') {
                    return None;
                }
                index += 1;
            }
            _ => index += 1,
        }
    } else {
        let character = source.get(index..)?.chars().next()?;
        if character == '\n' || character == '\r' || character.is_control() {
            return None;
        }
        index += character.len_utf8();
    }
    (bytes.get(index) == Some(&b'\'')).then_some(index + 1)
}

fn is_identifier_start(character: char) -> bool {
    character == '_' || character.is_alphabetic() || !character.is_ascii()
}

fn is_identifier_continue(character: char) -> bool {
    character == '_' || character.is_alphanumeric() || !character.is_ascii()
}

fn newline_count(bytes: &[u8]) -> usize {
    bytes.iter().filter(|byte| **byte == b'\n').count()
}

fn use_statement_end(tokens: &[Token], start: usize) -> usize {
    let mut braces = 0usize;
    let mut brackets = 0usize;
    let mut parentheses = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(start) {
        match &token.kind {
            TokenKind::Punct('{') => braces += 1,
            TokenKind::Punct('}') => braces = braces.saturating_sub(1),
            TokenKind::Punct('[') => brackets += 1,
            TokenKind::Punct(']') => brackets = brackets.saturating_sub(1),
            TokenKind::Punct('(') => parentheses += 1,
            TokenKind::Punct(')') => parentheses = parentheses.saturating_sub(1),
            TokenKind::Punct(';') if braces == 0 && brackets == 0 && parentheses == 0 => {
                return index;
            }
            _ => {}
        }
    }
    tokens.len()
}

fn scan_use_tree(
    tokens: &[Token],
    start: usize,
    end: usize,
    inherited: &[String],
    findings: &mut Vec<Finding>,
) {
    for (item_start, item_end) in split_use_items(tokens, start, end) {
        if item_start >= item_end {
            continue;
        }
        if let Some(open) = (item_start..item_end).find(|index| tokens[*index].punct('{')) {
            let close = matching_brace(tokens, open, item_end).unwrap_or(item_end);
            let (local_path, absolute) = import_path(tokens, item_start, open);
            let path = combine_path(inherited, local_path, absolute);
            inspect_import(&path, false, None, tokens[item_start].line, findings);
            scan_use_tree(tokens, open + 1, close, &path, findings);
        } else {
            let alias_index = (item_start..item_end).find(|index| tokens[*index].keyword("as"));
            let path_end = alias_index.unwrap_or_else(|| {
                (item_start..item_end)
                    .find(|index| tokens[*index].punct('*'))
                    .unwrap_or(item_end)
            });
            let (local_path, absolute) = import_path(tokens, item_start, path_end);
            let path = combine_path(inherited, local_path, absolute);
            let alias = alias_index
                .and_then(|index| tokens.get(index + 1))
                .and_then(Token::ident)
                .map(|(name, _)| name);
            let has_glob = (item_start..item_end).any(|index| tokens[index].punct('*'));
            inspect_import(&path, has_glob, alias, tokens[item_start].line, findings);
        }
    }
}

fn split_use_items(tokens: &[Token], start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut items = Vec::new();
    let mut item_start = start;
    let mut braces = 0usize;
    for (index, token) in tokens.iter().enumerate().take(end).skip(start) {
        if token.punct('{') {
            braces += 1;
        } else if token.punct('}') {
            braces = braces.saturating_sub(1);
        } else if token.punct(',') && braces == 0 {
            items.push((item_start, index));
            item_start = index + 1;
        }
    }
    items.push((item_start, end));
    items
}

fn matching_brace(tokens: &[Token], open: usize, end: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(end).skip(open) {
        if token.punct('{') {
            depth += 1;
        } else if token.punct('}') {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn import_path(tokens: &[Token], start: usize, end: usize) -> (Vec<String>, bool) {
    let absolute = tokens.get(start).is_some_and(|token| token.punct(':'))
        && tokens.get(start + 1).is_some_and(|token| token.punct(':'));
    let mut index = start + usize::from(absolute) * 2;
    let mut path = Vec::new();
    while index < end {
        if tokens[index].keyword("as") || tokens[index].punct('*') {
            break;
        }
        if let Some((name, _)) = tokens[index].ident() {
            path.push(name.to_owned());
            index += 1;
        } else if tokens[index].punct(':')
            && tokens.get(index + 1).is_some_and(|next| next.punct(':'))
        {
            index += 2;
        } else {
            break;
        }
    }
    (path, absolute)
}

fn combine_path(inherited: &[String], local: Vec<String>, absolute: bool) -> Vec<String> {
    if absolute || inherited.is_empty() || local.first().is_some_and(|first| first == "crate") {
        return local;
    }
    if local.first().is_some_and(|first| first == "self") {
        let mut path = inherited.to_vec();
        path.extend(local.into_iter().skip(1));
        return path;
    }
    let mut path = inherited.to_vec();
    path.extend(local);
    path
}

fn inspect_import(
    path: &[String],
    has_glob: bool,
    alias: Option<&str>,
    line: usize,
    findings: &mut Vec<Finding>,
) {
    if path.len() == 1 && ROOT_CRATES.contains(&path[0].as_str()) {
        if alias.is_some() {
            findings.push(Finding {
                line,
                reason: "standard-library root alias",
            });
        }
        if has_glob {
            findings.push(Finding {
                line,
                reason: "standard-library root glob import",
            });
        }
    }
    if path.len() > 1
        && ROOT_CRATES.contains(&path[0].as_str())
        && FORBIDDEN_MODULES.contains(&path[1].as_str())
    {
        findings.push(Finding {
            line,
            reason: "forbidden standard-library module import",
        });
    }
}
