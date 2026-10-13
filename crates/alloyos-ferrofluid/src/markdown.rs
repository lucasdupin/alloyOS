//! Small, allocation-only Markdown block parser for the bare-metal chat UI.
//! Unsupported inline syntax remains visible instead of losing source content.
use alloc::{string::String, vec::Vec};

#[derive(Debug, Default, PartialEq)]
pub(crate) struct Block {
    pub text: String,
    pub heading: usize,
    pub code: bool,
    pub quote: bool,
    pub strong: bool,
}

/// Split inline bold into measured words, keeping spaces with their word.
/// Unclosed delimiters stay literal while a response is streaming.
pub(crate) fn inline(text: &str) -> Vec<(String, bool)> {
    let mut spans = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("**") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("**") else { break };
        if end == 0 {
            break;
        }
        spans.push((&rest[..start], false));
        spans.push((&after[..end], true));
        rest = &after[end + 2..];
    }
    spans.push((rest, false));
    let mut words: Vec<(String, bool)> = Vec::new();
    for (span, strong) in spans {
        for word in span.split_inclusive(char::is_whitespace) {
            if word.chars().all(char::is_whitespace) {
                if let Some((previous, _)) = words.last_mut() {
                    previous.push_str(word);
                    continue;
                }
            }
            words.push((String::from(word), strong));
        }
    }
    words
}

pub(crate) fn parse(source: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut code = String::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        let marker = trimmed.chars().next().unwrap_or(' ');
        let count = trimmed.chars().take_while(|&c| c == marker).count();
        if let Some((open, length)) = fence {
            if marker == open && count >= length && trimmed[count..].trim().is_empty() {
                blocks.push(Block {
                    text: core::mem::take(&mut code),
                    code: true,
                    ..Block::default()
                });
                fence = None;
            } else {
                if !code.is_empty() {
                    code.push('\n');
                }
                code.push_str(line);
            }
            continue;
        }
        if matches!(marker, '`' | '~') && count >= 3 {
            fence = Some((marker, count));
            continue;
        }
        let mut block = Block::default();
        let mut body = line;
        let hashes = trimmed.chars().take_while(|&c| c == '#').count();
        if (1..=6).contains(&hashes) && trimmed.as_bytes().get(hashes) == Some(&b' ') {
            block.heading = hashes;
            body = trimmed[hashes + 1..].trim_end();
        } else if let Some(quote) = trimmed.strip_prefix("> ") {
            block.quote = true;
            body = quote;
        } else if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            block.text.push_str(&line[..line.len() - trimmed.len()]);
            block.text.push_str("• ");
            body = item;
        }
        if let Some(strong) = body.strip_prefix("**").and_then(|s| s.strip_suffix("**")) {
            if !strong.is_empty() && !strong.contains("**") {
                body = strong;
                block.strong = true;
            }
        }
        block.text.push_str(body);
        blocks.push(block);
    }
    if fence.is_some() {
        blocks.push(Block {
            text: code,
            code: true,
            ..Block::default()
        });
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bold_prefix_keeps_following_sentence_normal() {
        let words = inline("**Explore my capabilities:** I can tell you...");
        assert_eq!(
            words.iter().map(|(s, _)| s.as_str()).collect::<String>(),
            "Explore my capabilities: I can tell you..."
        );
        assert!(words[..3].iter().all(|(_, strong)| *strong));
        assert!(words[3..].iter().all(|(_, strong)| !*strong));
        assert_eq!(
            inline("**unfinished"),
            alloc::vec![(String::from("**unfinished"), false)]
        );
    }
    #[test]
    fn renders_blocks_and_preserves_code() {
        let blocks = parse("# Title\n- **Item**\n> Quote\n```rust\n# literal\n```\nplain");
        assert_eq!(blocks.len(), 5);
        assert_eq!(blocks[0].heading, 1);
        assert_eq!(blocks[1].text, "• Item");
        assert!(blocks[1].strong);
        assert!(blocks[2].quote);
        assert!(blocks[3].code);
        assert_eq!(blocks[3].text, "# literal");
        assert_eq!(blocks[4].text, "plain");
    }
    #[test]
    fn streaming_and_unicode_remain_readable() {
        assert_eq!(parse("```\n你好 **partial")[0].text, "你好 **partial");
        assert_eq!(parse("**unfinished")[0].text, "**unfinished");
        assert_eq!(
            parse("你好 `code` [link](url)")[0].text,
            "你好 `code` [link](url)"
        );
        assert_eq!(parse("```\na\n~~~\nb\n```")[0].text, "a\n~~~\nb");
    }
}
