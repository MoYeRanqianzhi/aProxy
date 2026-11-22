//! Integrity of the shipped extension bootstrap. Neither the package nor the page supplies a hash.
use sha2::{Digest, Sha256};
/// Fixed embedded document path; no package-controlled URL or directory is served.
pub const PATH: &str = "/extension-frame.html";
/// Hash the full raw asset and the single parser-normalized inline classic script separately.
/// A missing, ambiguous or decorated script element is not a qualified bootstrap.
pub fn fingerprints(bytes: &[u8]) -> Option<([u8; 32], [u8; 32])> {
    let html = std::str::from_utf8(bytes).ok()?;
    if html.starts_with('\u{feff}') || html.contains('\0') {
        return None;
    }
    let lower = html.to_ascii_lowercase();
    if lower.matches("<script").count() != 1 || lower.matches("</script").count() != 1 {
        return None;
    }
    let (_, tail) = html.split_once("<script>")?;
    let (script, _) = tail.split_once("</script>")?;
    if script.trim().is_empty() {
        return None;
    }
    Some((
        Sha256::digest(bytes).into(),
        Sha256::digest(script.replace("\r\n", "\n").replace('\r', "\n").as_bytes()).into(),
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bootstrap_hashes_script_and_document_independently() {
        let (d, s) = fingerprints(b"<script>\r\na\r</script>").unwrap();
        let (d2, s2) = fingerprints(b"<script>\na\n</script>").unwrap();
        assert_eq!(s, s2);
        assert_ne!(d, d2);
        assert_eq!(s, <[u8; 32]>::from(Sha256::digest(b"\na\n")));
        for bad in [
            &b"<script> </script>"[..],
            b"\xef\xbb\xbf<script>x</script>",
            b"<script src=x></script>",
            b"<script>x</script><SCRIPT>x</SCRIPT>",
            b"</script><script>",
            b"\xff",
        ] {
            assert!(fingerprints(bad).is_none());
        }
    }
}
