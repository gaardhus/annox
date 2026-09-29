//! Sync (spec §7): a store-and-relay hub and the replica that keeps a
//! workspace's `.annox/` in step with it.

pub mod credentials;
pub mod hub;
pub mod replica;
mod rpc;

/// Storage format spoken (§5.3).
pub const FORMAT: u64 = 1;

/// WebSocket subprotocol (§7.3).
pub const SUBPROTOCOL: &str = "annox-sync";

// Error codes (§7.9).
pub const UNAUTHORIZED: i32 = 2001;
pub const READ_ONLY: i32 = 2002;
pub const UNSUPPORTED_FORMAT: i32 = 2003;
pub const UNKNOWN_HUB: i32 = 2004;
pub const INVALID_ITEM: i32 = 2005;

/// Whether `s` is a canonical lowercase UUID (§2.1).
pub(crate) fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_digit() || ('a'..='f').contains(&c),
        })
}

/// Whether `path` is a valid document path (§3.1): relative, `/`-separated,
/// with no empty, `.` or `..` segments.
pub(crate) fn is_document_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// Whether `item` is a well-formed item carrying a well-formed event (§7.5.1,
/// §7.7).
pub fn is_well_formed(item: &serde_json::Value) -> bool {
    let event = &item["event"];
    let str_uuid = |v: &serde_json::Value| v.as_str().is_some_and(is_uuid);
    str_uuid(&item["document"])
        && str_uuid(&event["id"])
        && event["type"].is_string()
        && event["after"].as_array().is_some_and(|a| a.iter().all(str_uuid))
        && (str_uuid(&event["annotation"]) || str_uuid(&event["document"]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_escaping_paths() {
        assert!(is_document_path("ch2/intro.md"));
        for bad in ["", "/etc/passwd", "../x", "a/../b", "a//b", "a/./b", "a\\b"] {
            assert!(!is_document_path(bad), "{bad}");
        }
    }
}
