use crate::shared::constants::*;
use crate::shared::models::*;
use chrono::{Local, NaiveDate, TimeZone};
use std::path::PathBuf;

pub fn parse_query(
    input: &str,
    scope: SearchScope,
    directory: PathBuf,
    roots: Vec<PathBuf>,
    limit: usize,
) -> Result<SearchQuery, String> {
    if input.len() > MAX_QUERY_BYTES
        || input.chars().any(char::is_control)
        || limit == 0
        || limit > MAX_RESULTS
    {
        return Err("Query or result limit is invalid".into());
    }
    if scope == SearchScope::SelectedRoots && roots.is_empty() {
        return Err("Choose index roots in Settings › Search".into());
    }
    // Each token remembers whether it began inside quotes: `"12:30"` or `"a:b"` is plain text,
    // while `intitle:"monthly report"` is still an operator with a quoted value.
    let mut tokens: Vec<(String, bool)> = Vec::new();
    let mut token = String::new();
    let mut token_quoted = false;
    let mut quoted = false;
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && quoted && chars.peek() == Some(&'"') {
            token.push('"');
            chars.next();
            continue;
        }
        if ch == '"' {
            if !quoted && token.is_empty() {
                token_quoted = true;
            }
            quoted = !quoted;
            continue;
        }
        if ch.is_whitespace() && !quoted {
            if !token.is_empty() {
                tokens.push((std::mem::take(&mut token), token_quoted));
            }
            token_quoted = false;
        } else {
            token.push(ch);
        }
    }
    if quoted {
        return Err("Unclosed quote".into());
    }
    if !token.is_empty() {
        tokens.push((token, token_quoted));
    }
    let mut text = Vec::new();
    let mut filters = Vec::new();
    let mut tokens = tokens.into_iter();
    while let Some((token, is_quoted)) = tokens.next() {
        let Some((operator, value)) = token.split_once(':').filter(|_| !is_quoted) else {
            text.push(token);
            continue;
        };
        // Drive letters such as C:\Users stay plain text.
        if operator.len() == 1 && operator.chars().all(|ch| ch.is_ascii_alphabetic()) {
            text.push(token);
            continue;
        }
        let operator = operator.to_ascii_lowercase();
        let value = if value.is_empty() {
            tokens
                .next()
                .map(|(value, _)| value)
                .ok_or_else(|| format!("{operator}: needs a value"))?
        } else {
            value.to_string()
        };
        if value.is_empty() {
            return Err("Filter needs a value".into());
        }
        match operator.as_str() {
            "intitle" => filters.push(SearchFilter::Title(value)),
            "type" => {
                let value = value.trim_start_matches('.').to_lowercase();
                filters.push(
                    if matches!(
                        value.as_str(),
                        "document" | "image" | "video" | "audio" | "archive" | "folder"
                    ) {
                        SearchFilter::Category(value)
                    } else {
                        SearchFilter::Extension(value)
                    },
                );
            }
            "tag" => filters.push(SearchFilter::Tag(value)),
            "date" => {
                let (start, end) = value.split_once("..").unwrap_or((&value, &value));
                let start = NaiveDate::parse_from_str(start, "%Y-%m-%d")
                    .map_err(|_| "Dates must be YYYY-MM-DD")?;
                let end = NaiveDate::parse_from_str(end, "%Y-%m-%d")
                    .map_err(|_| "Dates must be YYYY-MM-DD")?;
                if end < start {
                    return Err("End date is before start date".into());
                }
                let end = end.succ_opt().ok_or("Date range is invalid")?;
                let start = Local
                    .from_local_datetime(&start.and_hms_opt(0, 0, 0).ok_or("Date is invalid")?)
                    // A DST gap can skip local midnight; take the earliest valid instant.
                    .earliest()
                    .ok_or("Local date is ambiguous")?
                    .timestamp();
                let end = Local
                    .from_local_datetime(&end.and_hms_opt(0, 0, 0).ok_or("Date is invalid")?)
                    .earliest()
                    .ok_or("Local date is ambiguous")?
                    .timestamp();
                filters.push(SearchFilter::Date(start, end));
            }
            _ => {
                return Err(format!(
                    "Unknown filter {operator}:. Use intitle:, type:, tag: or date: (quote text that contains a colon)"
                ));
            }
        }
    }
    Ok(SearchQuery {
        text: text.join(" "),
        filters,
        ids: Vec::new(),
        scope,
        directory,
        roots,
        limit,
        offset: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Result<SearchQuery, String> {
        parse_query(
            input,
            SearchScope::IndexedFiles,
            PathBuf::from("/"),
            Vec::new(),
            10,
        )
    }

    #[test]
    fn operators_and_text() {
        let query =
            parse(r#"antigravity Type:EXE tag:"AI Agent" intitle:"monthly report""#).unwrap();
        assert_eq!(query.text, "antigravity");
        assert!(matches!(&query.filters[0], SearchFilter::Extension(value) if value == "exe"));
        assert!(matches!(&query.filters[1], SearchFilter::Tag(value) if value == "AI Agent"));
        assert!(
            matches!(&query.filters[2], SearchFilter::Title(value) if value == "monthly report")
        );
    }

    #[test]
    fn quoted_colons_and_drive_letters_are_text() {
        let query = parse(r#""12:30" C:\Users "a:b""#).unwrap();
        assert_eq!(query.text, r"12:30 C:\Users a:b");
        assert!(query.filters.is_empty());
    }

    #[test]
    fn errors_are_reported() {
        assert!(parse(r#""open"#).is_err());
        assert!(parse("size:10").is_err());
        assert!(parse("date:2026-10-09..2026-10-01").is_err());
        assert!(parse("date:2026-02-30").is_err());
    }

    #[test]
    fn date_range_is_half_open() {
        let query = parse("date:2026-10-01..2026-10-01").unwrap();
        let SearchFilter::Date(start, end) = query.filters[0] else {
            panic!("expected date filter");
        };
        assert!(end - start >= 23 * 3600 && end - start <= 25 * 3600);
    }
}
