//! Conservative path-list parsing. Never evaluate pasted shell syntax.

pub fn is_absolute(value: &str) -> bool {
    value.starts_with('/') || is_windows(value)
}

pub fn is_windows(value: &str) -> bool {
    let b = value.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/'))
        || value.starts_with("\\\\")
}

pub fn parse(payload: &[u8]) -> Option<(Vec<String>, bool)> {
    let text = std::str::from_utf8(payload).ok()?;
    if text.chars().any(char::is_control) {
        return None;
    }
    let trailing = text.ends_with(' ');
    let mut rest = text.trim();
    let mut paths = Vec::new();
    while !rest.is_empty() {
        let quote = rest.chars().next()?;
        let (path, consumed) = if matches!(quote, '\'' | '"') {
            let end = rest[1..].find(quote)? + 1;
            if !rest[end + 1..].is_empty() && !rest[end + 1..].starts_with(' ') {
                return None;
            }
            (rest[1..end].to_owned(), end + 1)
        } else {
            if !is_absolute(rest) {
                return None;
            }
            let windows = is_windows(rest);
            let mut path = String::new();
            let mut chars = rest.char_indices().peekable();
            let mut end = rest.len();
            while let Some((i, ch)) = chars.next() {
                if !windows && ch == '\\' {
                    path.push(chars.next()?.1);
                } else if ch == ' ' {
                    let tail = rest[i..].trim_start();
                    if tail.is_empty() || is_absolute(tail) || tail.starts_with(['\'', '"']) {
                        end = i;
                        break;
                    }
                    path.push(ch);
                } else {
                    path.push(ch);
                }
            }
            (path, end)
        };
        if !is_absolute(&path) {
            return None;
        }
        paths.push(path);
        if paths.len() > 32 {
            return None;
        }
        rest = rest[consumed..].trim_start();
    }
    if paths.is_empty() {
        None
    } else {
        Some((paths, trailing))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognizes_drop_formats_without_shell_evaluation() {
        for (input, expected) in [
            (
                r#""C:\Users\Me\one two.png" "D:\三.png" "#,
                vec![r"C:\Users\Me\one two.png", r"D:\三.png"],
            ),
            (
                r"C:\Users\Me\one two.png ",
                vec![r"C:\Users\Me\one two.png"],
            ),
            (
                r"/Users/me/one\ two.png /Users/me/三.png ",
                vec!["/Users/me/one two.png", "/Users/me/三.png"],
            ),
            ("'/Users/me/one two.png' ", vec!["/Users/me/one two.png"]),
            (r"\\server\share\one.png ", vec![r"\\server\share\one.png"]),
        ] {
            assert_eq!(
                parse(input.as_bytes()).unwrap(),
                (expected.into_iter().map(String::from).collect(), true)
            );
        }
        for text in [
            "hello world",
            "look at /Users/me/a",
            "'/a' prose",
            "/a\n/b",
            "\x16",
            "'/unclosed",
        ] {
            assert!(parse(text.as_bytes()).is_none(), "{text}");
        }
    }
}
