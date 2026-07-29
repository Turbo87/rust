use super::*;

type ParseResult = Result<(String, Feature), String>;

fn parse(source: &str) -> Vec<ParseResult> {
    let mut results = Vec::new();
    parse_lib_features(Path::new("test.rs"), source, &mut |result, _, _| {
        let result =
            result.map(|(name, feature)| (name.to_owned(), feature)).map_err(str::to_owned);
        results.push(result);
    });
    results
}

#[test]
fn test_parse_stable_feature() {
    let source = r#"#[stable(feature = "test", since = "1.42.0")]"#;
    let results = parse(source);

    assert_eq!(results.len(), 1);
    let (name, feature) = results[0].as_ref().unwrap();
    assert_eq!(name, "test");
    assert_eq!(feature.level, Status::Accepted);
    assert_eq!(feature.since, Some(Version::Explicit { parts: [1, 42, 0] }));
    assert_eq!(feature.tracking_issue, None);
    assert_eq!(feature.line, 1);
}

#[test]
fn test_parse_stable_feature_with_current_version() {
    let source = r#"#[stable(feature = "test", since = "CURRENT_RUSTC_VERSION")]"#;
    let results = parse(source);

    assert_eq!(results.len(), 1);
    let (_, feature) = results[0].as_ref().unwrap();
    assert_eq!(feature.since, Some(Version::CurrentPlaceholder));
}

#[test]
fn test_parse_stable_feature_with_reordered_fields() {
    let source = r#"#[stable(since = "1.42.0", feature = "test")]"#;
    let results = parse(source);

    assert_eq!(results.len(), 1);
    let (name, feature) = results[0].as_ref().unwrap();
    assert_eq!(name, "test");
    assert_eq!(feature.since, Some(Version::Explicit { parts: [1, 42, 0] }));
}

#[test]
fn test_parse_stable_feature_with_whitespace() {
    let source = r#"#[stable( feature="test", since= "1.42.0" )]"#;
    let results = parse(source);

    assert_eq!(results.len(), 1);
    let (name, feature) = results[0].as_ref().unwrap();
    assert_eq!(name, "test");
    assert_eq!(feature.since, Some(Version::Explicit { parts: [1, 42, 0] }));
}

#[test]
fn test_parse_stable_feature_line() {
    let source = r#"pub struct Unrelated;

#[stable(feature = "test", since = "1.42.0")]"#;
    let results = parse(source);

    assert_eq!(results.len(), 1);
    let (_, feature) = results[0].as_ref().unwrap();
    assert_eq!(feature.line, 3);
}

#[test]
fn test_parse_multiple_stable_features() {
    let source = r#"#[stable(feature = "first", since = "1.0.0")]
#[stable(feature = "second", since = "1.1.0")]"#;
    let results = parse(source);

    assert_eq!(results.len(), 2);
    let (name, _) = results[0].as_ref().unwrap();
    assert_eq!(name, "first");
    let (name, _) = results[1].as_ref().unwrap();
    assert_eq!(name, "second");
}

#[test]
fn test_ignore_commented_stable_feature() {
    let source = r#"    // #[stable(feature = "test", since = "1.42.0")]"#;

    assert!(parse(source).is_empty());
}

#[test]
fn test_report_missing_stable_feature_name() {
    let source = r#"#[stable(since = "1.42.0")]"#;
    let results = parse(source);

    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].as_ref().unwrap_err(),
        "malformed stability attribute: missing `feature` key"
    );
}

#[test]
fn test_report_missing_stable_since() {
    let source = r#"#[stable(feature = "test")]"#;
    let results = parse(source);

    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].as_ref().unwrap_err(),
        "malformed stability attribute: missing the `since` key"
    );
}

#[test]
fn test_report_invalid_stable_since() {
    let source = r#"#[stable(feature = "test", since = "1.42")]"#;
    let results = parse(source);

    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].as_ref().unwrap_err(),
        "malformed stability attribute: can't parse `since` key"
    );
}

#[test]
fn test_continue_after_malformed_stable_feature() {
    let source = r#"#[stable(feature = "malformed")]
#[stable(feature = "valid", since = "1.42.0")]"#;
    let results = parse(source);

    assert_eq!(results.len(), 2);
    assert!(results[0].is_err());
    let (name, _) = results[1].as_ref().unwrap();
    assert_eq!(name, "valid");
}

#[test]
fn test_parse_unstable_feature() {
    let source = r#"#[unstable(feature = "test", issue = "12345")]"#;
    let results = parse(source);

    assert_eq!(results.len(), 1);

    let (name, feature) = results[0].as_ref().unwrap();
    assert_eq!(name, "test");
    assert_eq!(feature.level, Status::Unstable);
    assert_eq!(feature.since, None);
    assert_eq!(feature.tracking_issue.map(|issue| issue.get()), Some(12345));
    assert_eq!(feature.line, 1);
}

#[test]
fn test_parse_multiline_unstable_feature() {
    let source = r#"#[unstable(
    feature = "process_exitcode_internals",
    reason = "exposed only for libstd",
    issue = "none"
)]"#;
    let results = parse(source);

    assert_eq!(results.len(), 1);

    let (name, feature) = results[0].as_ref().unwrap();
    assert_eq!(name, "process_exitcode_internals");
    assert_eq!(feature.level, Status::Unstable);
    assert_eq!(feature.since, None);
    assert_eq!(feature.tracking_issue, None);
    assert_eq!(feature.line, 1);
}

#[test]
fn test_parse_rustc_const_unstable_feature() {
    let source = r#"#[rustc_const_unstable(feature = "test", issue = "none")]"#;
    let results = parse(source);

    assert_eq!(results.len(), 1);

    let (name, feature) = results[0].as_ref().unwrap();
    assert_eq!(name, "test");
    assert_eq!(feature.level, Status::Unstable);
    assert_eq!(feature.since, None);
    assert_eq!(feature.tracking_issue, None);
}
