use super::*;

fn text(value: &str) -> FrontMatterValue {
    FrontMatterValue::Text(value.into())
}

fn list(items: &[&str]) -> FrontMatterValue {
    FrontMatterValue::List(items.iter().map(|item| (*item).to_owned()).collect())
}

fn parse(front: &str) -> FrontMatter {
    FrontMatter::parse(front).unwrap_or_else(|error| panic!("{error}: {front}"))
}

#[test]
fn scalars_quotes_and_comments() {
    let front = parse(concat!(
        "# leading comment\n",
        "name: code-review   # trailing comment\n",
        "description: \"Review \\\"diffs\\\"\\tcarefully\\n\\u00e9 \\x41 # not a comment\"\n",
        "single: 'it''s # fine'\n",
        "colon: Use this: when needed\n",
        "hash: issue#42\n",
        "url: https://example.com/a#b\n",
        "empty:\n",
        "tilde: ~\n",
        "nothing: null\n",
        "\n",
        "version: 1.2\n",
    ));
    assert_eq!(front.get("name"), Some(&text("code-review")));
    assert_eq!(
        front.get("description"),
        Some(&text("Review \"diffs\"\tcarefully\né A # not a comment"))
    );
    assert_eq!(front.get("single"), Some(&text("it's # fine")));
    assert_eq!(front.get("colon"), Some(&text("Use this: when needed")));
    assert_eq!(front.get("hash"), Some(&text("issue#42")));
    assert_eq!(front.get("url"), Some(&text("https://example.com/a#b")));
    assert_eq!(front.get("version"), Some(&text("1.2")));
    for absent in ["empty", "tilde", "nothing"] {
        assert_eq!(front.get(absent), None, "{absent}");
    }
}

#[test]
fn booleans_are_typed_only_when_plain() {
    let front = parse("a: true\nb: False\nc: TRUE\nd: \"true\"\ne: yes\n");
    assert_eq!(front.get("a"), Some(&FrontMatterValue::Bool(true)));
    assert_eq!(front.get("b"), Some(&FrontMatterValue::Bool(false)));
    assert_eq!(front.get("c"), Some(&FrontMatterValue::Bool(true)));
    assert_eq!(front.get("d"), Some(&text("true")));
    assert_eq!(front.get("e"), Some(&text("yes")));
}

#[test]
fn inline_and_block_lists() {
    let front = parse(concat!(
        "inline: [Read, \"Grep\", 'Bash(git status:*)', ]\n",
        "empty: []\n",
        "block:\n",
        "  - Read\n",
        "  - \"Write\"   # comment\n",
        "  -\n",
        "  - true\n",
        "flush:\n",
        "- one\n",
        "- two\n",
        "after: x\n",
    ));
    assert_eq!(
        front.get("inline"),
        Some(&list(&["Read", "Grep", "Bash(git status:*)"]))
    );
    assert_eq!(front.get("empty"), Some(&list(&[])));
    assert_eq!(front.get("block"), Some(&list(&["Read", "Write", "true"])));
    assert_eq!(front.get("flush"), Some(&list(&["one", "two"])));
    assert_eq!(front.get("after"), Some(&text("x")));
}

#[test]
fn nested_mappings_and_flow_maps_are_ignored() {
    let front = parse(concat!(
        "metadata:\n",
        "  owner: me\n",
        "  tags: [a]\n",
        "people:\n",
        "  - name: a\n",
        "  - name: b\n",
        "nested: [[a], b]\n",
        "map: {a: 1,\n",
        "  b: 2}\n",
        "name: kept\n",
    ));
    assert_eq!(front.entries().len(), 1, "{front:?}");
    assert_eq!(front.get("name"), Some(&text("kept")));
}

#[test]
fn block_scalars_fold_and_chomp() {
    let front = parse(concat!(
        "literal: |\n",
        "  line one\n",
        "    indented\n",
        "  line three\n",
        "\n",
        "folded: >\n",
        "  a long\n",
        "  sentence\n",
        "\n",
        "  new paragraph\n",
        "stripped: |-\n",
        "  no newline\n",
        "kept: >+\n",
        "  kept\n",
        "\n",
        "explicit: |2\n",
        "    four spaces\n",
        "  two\n",
        "next: done\n",
    ));
    assert_eq!(
        front.get("literal"),
        Some(&text("line one\n  indented\nline three\n"))
    );
    assert_eq!(
        front.get("folded"),
        Some(&text("a long sentence\nnew paragraph\n"))
    );
    assert_eq!(front.get("stripped"), Some(&text("no newline")));
    assert_eq!(front.get("kept"), Some(&text("kept\n\n")));
    assert_eq!(front.get("explicit"), Some(&text("  four spaces\ntwo\n")));
    assert_eq!(front.get("next"), Some(&text("done")));
}

#[test]
fn multi_line_plain_and_quoted_scalars_fold() {
    let front = parse(concat!(
        "plain: first part\n",
        "  second part\n",
        "\n",
        "  new line\n",
        "quoted: \"first\n",
        "  second\"\n",
        "single: 'a\n",
        "\n",
        "  b'\n",
    ));
    assert_eq!(
        front.get("plain"),
        Some(&text("first part second part\nnew line"))
    );
    assert_eq!(front.get("quoted"), Some(&text("first second")));
    assert_eq!(front.get("single"), Some(&text("a\nb")));
}

#[test]
fn multi_line_quoted_scalars_decode_across_lines() {
    let front = parse(concat!(
        "escapes: \"\\u00e9\\t\n",
        "  \\x41 \\\"b\\\"\" # comment\n",
        "single: 'it''s\n",
        "  ''quoted'''\n",
        "closed: 'a'\n",
    ));
    assert_eq!(front.get("escapes"), Some(&text("é\t A \"b\"")));
    assert_eq!(front.get("single"), Some(&text("it's 'quoted'")));
    assert_eq!(front.get("closed"), Some(&text("a")));
    for (front, line) in [
        ("cut: \"a\\\n  u\"\n", 1),
        ("cut: \"a\\u00\n  e9\"\n", 1),
        ("after: \"a\n  b\" c\n", 1),
    ] {
        let error = FrontMatter::parse(front).unwrap_err();
        assert_eq!(error.line, Some(line), "{front}: {error}");
    }
}

#[test]
fn front_matter_without_keys_keeps_every_other_line_as_written() {
    let file = concat!(
        "\u{feff}---\r\n",
        "# heading comment\r\n",
        "name: deploy\r\n",
        "description: |\r\n",
        "  line one\r\n",
        "\r\n",
        "  line two\r\n",
        "tools: [a, \"b\"]\r\n",
        "version: 'v\r\n",
        "  1'\r\n",
        "metadata:\r\n",
        "  nested: map\r\n",
        "list:\r\n",
        "- x\r\n",
        "---\r\n",
        "Body.\r\n",
    );
    let kept = front_matter_without(file, &["name", "description", "version"])
        .unwrap()
        .unwrap();
    assert_eq!(
        kept,
        "# heading comment\ntools: [a, \"b\"]\nmetadata:\n  nested: map\nlist:\n- x\n"
    );
    let reparsed = parse(&kept);
    assert_eq!(
        reparsed.entries(),
        [
            ("tools".to_owned(), list(&["a", "b"])),
            ("list".to_owned(), list(&["x"])),
        ]
    );
    // Dropping nothing keeps the whole block; an empty block keeps nothing.
    assert_eq!(
        front_matter_without("---\nname: a\n---\n", &[]).unwrap(),
        Some("name: a\n".to_owned())
    );
    assert_eq!(
        front_matter_without("---\n---\nbody\n", &["name"]).unwrap(),
        Some(String::new())
    );
    assert_eq!(
        front_matter_without("no front matter\n", &["name"]).unwrap(),
        None
    );
    assert!(front_matter_without("---\n  bad: indent\n---\n", &["name"]).is_err());
}

#[test]
fn many_line_quoted_scalars_parse_in_linear_time() {
    // Rescanning the value after every appended line made this quadratic.
    let blank_lines = 200_000;
    let front = format!("description: \"x\n{}  y\"\n", "\n".repeat(blank_lines));
    let started = std::time::Instant::now();
    let parsed = parse(&front);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    let expected = format!("x{}y", "\n".repeat(blank_lines));
    assert_eq!(parsed.get("description"), Some(&text(&expected)));
}

#[test]
fn malformed_front_matter_reports_lines_without_values() {
    let secret = "sk-proj-SECRETVALUE";
    for (front, line) in [
        ("name: x\nname: y\n".to_owned(), 2),
        (format!("  indented: {secret}\n"), 1),
        (format!("- {secret}\n"), 1),
        (format!("no colon {secret}\n"), 1),
        (format!("bad key!: {secret}\n"), 1),
        (format!("quote: \"{secret}\nnext: x\n"), 1),
        (format!("quote: \"{secret}\" trailing\n"), 1),
        (format!("escape: \"\\q{secret}\"\n"), 1),
        (format!("list: [{secret}, b\n"), 1),
        (format!("block: |x {secret}\n"), 1),
        (format!("block: |4\n  {secret}\n"), 2),
    ] {
        let error = FrontMatter::parse(&front).unwrap_err();
        assert_eq!(error.line, Some(line), "{front}: {error}");
        assert!(!error.to_string().contains(secret), "{error}");
    }
}

#[test]
fn documents_split_with_crlf_bom_and_missing_front_matter() {
    let document = SkillDocument::parse(
        "\u{feff}---\r\nname: crlf\r\ndescription: \"x\"\r\n---\r\n\r\n# Body\r\nline\r\n\r\n",
    )
    .unwrap();
    let front = document.front_matter.unwrap();
    assert_eq!(front.get("name"), Some(&text("crlf")));
    assert_eq!(front.get("description"), Some(&text("x")));
    assert_eq!(document.body, "# Body\nline");

    let plain = SkillDocument::parse("# Just markdown\n\n---\nnot front matter\n").unwrap();
    assert!(plain.front_matter.is_none());
    assert_eq!(plain.body, "# Just markdown\n\n---\nnot front matter");

    let dots = SkillDocument::parse("---\nname: a\n...\nbody").unwrap();
    assert_eq!(dots.body, "body");
    let empty = SkillDocument::parse("---\n---\n").unwrap();
    assert!(empty.front_matter.unwrap().is_empty());
    assert_eq!(empty.body, "");

    let unclosed = SkillDocument::parse("---\nname: a\ndescription: b\n").unwrap_err();
    assert!(unclosed.message.contains("not closed"), "{unclosed}");
    assert_eq!(unclosed.line, Some(1));
    let lines = SkillDocument::parse("---\nname: a\nname: b\n---\n").unwrap_err();
    assert_eq!(
        lines.line,
        Some(3),
        "line numbers count from the file start"
    );
}

#[test]
fn oversized_front_matter_is_refused() {
    let front = format!("---\nname: a\ndescription: {}\n---\n", "x".repeat(20_000));
    let error = SkillDocument::parse(&front).unwrap_err();
    assert!(error.message.contains("larger than"), "{error}");
}
