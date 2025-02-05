use object_store::path::Path;
use std::path::PathBuf;

/// Strips the prefix off the source path and returns a new owned object store [Path].
#[allow(dead_code)]
pub(crate) fn path_tail(source: &Path, prefix: &Path) -> Path {
    match PathBuf::from(source.as_ref()).strip_prefix(std::path::Path::new(prefix.as_ref())) {
        Ok(x) => Path::from(x.to_string_lossy().as_ref()),
        Err(_) => source.to_owned(),
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
    }
}
