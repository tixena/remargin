//! Lowercase hexadecimal encoding of a byte string.

pub fn encode<T>(bytes: T) -> String
where
    T: AsRef<[u8]>,
{
    use core::fmt::Write as _;
    let mut out = String::with_capacity(bytes.as_ref().len() * 2);
    for byte in bytes.as_ref() {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
