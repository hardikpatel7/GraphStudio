/// Split a SQL string into top-level statements at unquoted, uncommented `;`.
///
/// Recognizes:
/// - single-quoted strings `'...'` with `''` escape
/// - double-quoted identifiers `"..."` with `""` escape
/// - line comments `-- ...` to end of line
/// - block comments `/* ... */` (non-nesting; matches PG behavior at top level)
/// - PostgreSQL dollar-quoted strings `$tag$ ... $tag$` (and the unnamed `$$...$$`)
///
/// Returns trimmed, non-empty statements (terminator stripped).
pub fn split_statements(sql: &str) -> Vec<String> {
    let bytes = sql.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let n = bytes.len();

    while i < n {
        let c = bytes[i];

        // line comment
        if c == b'-' && i + 1 < n && bytes[i + 1] == b'-' {
            while i < n && bytes[i] != b'\n' { i += 1; }
            continue;
        }
        // block comment
        if c == b'/' && i + 1 < n && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < n && !(bytes[i] == b'*' && bytes[i + 1] == b'/') { i += 1; }
            if i + 1 < n { i += 2; }
            continue;
        }
        // single-quoted string
        if c == b'\'' {
            i += 1;
            while i < n {
                if bytes[i] == b'\'' {
                    if i + 1 < n && bytes[i + 1] == b'\'' { i += 2; continue; }
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        // double-quoted identifier
        if c == b'"' {
            i += 1;
            while i < n {
                if bytes[i] == b'"' {
                    if i + 1 < n && bytes[i + 1] == b'"' { i += 2; continue; }
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        // dollar-quoted string
        if c == b'$' {
            if let Some(tag_end) = find_dollar_tag(bytes, i) {
                let tag = &bytes[i..tag_end + 1]; // includes both `$`s
                let body_start = tag_end + 1;
                if let Some(close) = find_subslice(bytes, body_start, tag) {
                    i = close + tag.len();
                    continue;
                } else {
                    // unterminated — treat rest as one statement
                    i = n;
                    continue;
                }
            }
        }
        // statement terminator
        if c == b';' {
            let stmt = sql[start..i].trim();
            if !stmt.is_empty() { out.push(stmt.to_string()); }
            i += 1;
            start = i;
            continue;
        }
        i += 1;
    }

    let tail = sql[start..].trim();
    if !tail.is_empty() { out.push(tail.to_string()); }
    out
}

/// If `bytes[start]` is `$` and starts a valid dollar-quote opener
/// (`$` + optional identifier + `$`), return the index of the closing `$`.
fn find_dollar_tag(bytes: &[u8], start: usize) -> Option<usize> {
    if start >= bytes.len() || bytes[start] != b'$' { return None; }
    let mut j = start + 1;
    // tag chars: letter | underscore | digit (digit not as first char, but PG is lenient — match PG)
    while j < bytes.len() {
        let c = bytes[j];
        if c == b'$' { return Some(j); }
        let ok = c.is_ascii_alphanumeric() || c == b'_';
        if !ok { return None; }
        j += 1;
    }
    None
}

fn find_subslice(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() { return None; }
    let last = haystack.len().saturating_sub(needle.len());
    let mut k = from;
    while k <= last {
        if &haystack[k..k + needle.len()] == needle { return Some(k); }
        k += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_statement_no_semi() {
        assert_eq!(split_statements("SELECT 1"), vec!["SELECT 1"]);
    }

    #[test]
    fn trailing_semi_dropped() {
        assert_eq!(split_statements("SELECT 1;"), vec!["SELECT 1"]);
    }

    #[test]
    fn two_statements() {
        let s = split_statements("SET x = 1; SELECT 2;");
        assert_eq!(s, vec!["SET x = 1", "SELECT 2"]);
    }

    #[test]
    fn semi_inside_string_ignored() {
        let s = split_statements("SELECT 'a;b'; SELECT 1");
        assert_eq!(s, vec!["SELECT 'a;b'", "SELECT 1"]);
    }

    #[test]
    fn semi_inside_identifier_ignored() {
        let s = split_statements(r#"SELECT "a;b"; SELECT 1"#);
        assert_eq!(s, vec![r#"SELECT "a;b""#, "SELECT 1"]);
    }

    #[test]
    fn line_comment() {
        let s = split_statements("SELECT 1 -- ; not a split\n; SELECT 2");
        assert_eq!(s, vec!["SELECT 1 -- ; not a split", "SELECT 2"]);
    }

    #[test]
    fn block_comment() {
        let s = split_statements("SELECT /* ; */ 1; SELECT 2");
        assert_eq!(s, vec!["SELECT /* ; */ 1", "SELECT 2"]);
    }

    #[test]
    fn dollar_quote() {
        let s = split_statements("DO $$ BEGIN PERFORM 1; END $$; SELECT 2");
        assert_eq!(s, vec!["DO $$ BEGIN PERFORM 1; END $$", "SELECT 2"]);
    }

    #[test]
    fn tagged_dollar_quote() {
        let s = split_statements("SELECT $tag$ a; b $tag$; SELECT 1");
        assert_eq!(s, vec!["SELECT $tag$ a; b $tag$", "SELECT 1"]);
    }

    #[test]
    fn legacy_v2_pattern() {
        let sql = "select * from inventory_smart.article_selection_list_v2(123, 'x'); fetch all from \"352e64c0-aaa\";";
        let s = split_statements(sql);
        assert_eq!(s.len(), 2);
        assert!(s[0].starts_with("select * from"));
        assert!(s[1].starts_with("fetch all"));
    }

    #[test]
    fn escaped_quote_in_string() {
        let s = split_statements("SELECT 'O''Brien'; SELECT 1");
        assert_eq!(s, vec!["SELECT 'O''Brien'", "SELECT 1"]);
    }
}
