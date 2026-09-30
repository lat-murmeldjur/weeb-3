pub fn between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    source
        .split_once(start)
        .and_then(|(_, tail)| tail.split_once(end))
        .map(|(body, _)| body)
        .unwrap_or_else(|| panic!("missing source section between {start:?} and {end:?}"))
}

pub fn assert_in_order<'a>(source: &'a str, markers: &[&'a str]) {
    let mut tail = source;
    for marker in markers {
        tail = tail
            .split_once(marker)
            .unwrap_or_else(|| panic!("missing ordered source marker {marker:?}"))
            .1;
    }
}

pub fn compact(source: &str) -> String {
    source.split_whitespace().collect()
}

#[track_caller]
pub fn assert_contains(source: &str, markers: &[&str]) {
    for marker in markers {
        assert!(source.contains(marker), "missing source marker {marker:?}");
    }
}

#[track_caller]
pub fn assert_excludes(source: &str, markers: &[&str]) {
    for marker in markers {
        assert!(!source.contains(marker), "unexpected source marker {marker:?}");
    }
}

#[track_caller]
pub fn assert_first_in_order(source: &str, markers: &[(&str, &str)]) {
    let mut previous = None;
    for &(marker, missing) in markers {
        let position = source.find(marker).unwrap_or_else(|| panic!("{missing}: {marker:?}"));
        assert!(previous.is_none_or(|last| last < position), "out-of-order source marker {marker:?}");
        previous = Some(position);
    }
}
