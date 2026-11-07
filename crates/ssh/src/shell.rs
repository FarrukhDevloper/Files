//! Building remote shell commands safely.

/// `s` as one POSIX sh word.
pub(crate) fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Run `script` with POSIX sh whatever the login shell is (bash, zsh,
/// fish…): shipped base64-encoded, so only sh ever parses it. sh reads it
/// from stdin, so commands in it that might read stdin need `</dev/null`.
pub(crate) fn sh_command(script: &str) -> String {
    format!("echo {} | base64 -d | sh", base64(script.as_bytes()))
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            out.push(if i <= chunk.len() { T[(n >> (18 - 6 * i)) as usize & 63] as char } else { '=' });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(quote("/a b"), "'/a b'");
        assert_eq!(quote("/it's"), r"'/it'\''s'");
        assert_eq!(quote("/$(x)`y`"), "'/$(x)`y`'");
    }

    #[test]
    fn scripts_run_under_sh() {
        assert_eq!((base64(b"f"), base64(b"fo"), base64(b"foo")), ("Zg==".into(), "Zm8=".into(), "Zm9v".into()));
        let cmd = sh_command("x='a b'; printf '%s|' \"$x\" \"$(echo ok)\"");
        assert!(cmd.bytes().all(|b| b.is_ascii_alphanumeric() || b" |+/=-".contains(&b)), "{cmd}");
        let out = std::process::Command::new("sh").arg("-c").arg(&cmd).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "a b|ok|");
    }
}
