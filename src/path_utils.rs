use object_store::path::Path;

/// Strips the prefix off the source path and returns a new owned object store [Path]. The remaining parts are kept in
/// their already-encoded form so that percent-encoded characters are not encoded a second time.
#[allow(dead_code)]
pub(crate) fn path_tail(source: &Path, prefix: &Path) -> Path {
    match source.prefix_match(prefix) {
        Some(parts) => Path::from_iter(parts),
        None => source.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_tail() {
        assert_eq!(path_tail(&Path::from(""), &Path::from("")).to_string(), "");
        assert_eq!(path_tail(&Path::from("x/y/z"), &Path::from("")).to_string(), "x/y/z");
        assert_eq!(path_tail(&Path::from("x/y/z"), &Path::from("x")).to_string(), "y/z");
        assert_eq!(path_tail(&Path::from("x/y/z"), &Path::from("x/")).to_string(), "y/z");
        assert_eq!(path_tail(&Path::from("x/y/z"), &Path::from("x/y")).to_string(), "z");
        assert_eq!(path_tail(&Path::from("x/y/z"), &Path::from("x/y/")).to_string(), "z");
        assert_eq!(path_tail(&Path::from("x/y/z"), &Path::from("x/y/")).to_string(), "z");
        assert_eq!(path_tail(&Path::from("x/y/z"), &Path::from("x/y/z")).to_string(), "");
        assert_eq!(path_tail(&Path::from("x/y/z"), &Path::from("a/b/c")).to_string(), "x/y/z");
        assert_eq!(path_tail(&Path::from("x/yz/q"), &Path::from("x/y")).to_string(), "x/yz/q");
        let encoded = Path::from("x").join("a/b%");
        assert_eq!(path_tail(&encoded, &Path::from("x")).to_string(), "a%2Fb%25");
    }
}
