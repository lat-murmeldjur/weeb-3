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
