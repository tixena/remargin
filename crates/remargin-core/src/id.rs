//! ID generator: short, random, collision-checked identifiers.
//!
//! Every Remargin comment needs a unique short identifier within its document.
//! IDs are random alphanumeric strings that start at 3 characters and grow
//! when the space at the current length becomes more than half full.

use core::iter::repeat_with;
use std::collections::HashSet;

use rand::RngExt as _;

/// Character set for generated IDs: lowercase letters and digits.
const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// Number of distinct characters in the charset (36).
const CHARSET_SIZE: u32 = 36;

/// Minimum and default ID length.
const INITIAL_LENGTH: u32 = 3;

/// Generate a unique ID that does not collide with any existing IDs.
///
/// Starts at 3 characters.  If more than half the ID space at a given length
/// is already occupied, the generator automatically bumps to length + 1.
#[must_use]
pub fn generate(existing_ids: &HashSet<&str>) -> String {
    let length = pick_length(existing_ids);
    let mut rng = rand::rng();

    loop {
        let id: String = repeat_with(|| {
            let idx = rng.random_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .take(length as usize)
        .collect();

        if !existing_ids.contains(id.as_str()) {
            return id;
        }
    }
}

/// Determine the appropriate ID length given the set of existing IDs.
fn pick_length(existing_ids: &HashSet<&str>) -> u32 {
    let mut length = INITIAL_LENGTH;

    loop {
        let space_size = CHARSET_SIZE.pow(length);
        let ids_at_length = existing_ids
            .iter()
            .filter(|id| id.len() == length as usize)
            .count();

        // Check if ids_at_length / space_size > 0.5 using integer arithmetic:
        // ids_at_length * 2 > space_size.
        if ids_at_length * 2 <= space_size as usize {
            break;
        }

        length += 1;
    }

    length
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{CHARSET_SIZE, INITIAL_LENGTH, generate, pick_length};

    #[test]
    fn empty_set_produces_three_char_id() {
        let existing: HashSet<&str> = HashSet::new();
        let id = generate(&existing);
        assert_eq!(id.len(), INITIAL_LENGTH as usize);
    }

    #[test]
    fn small_set_no_collision() {
        let mut existing: HashSet<&str> = HashSet::new();
        existing.insert("abc");
        existing.insert("def");
        existing.insert("123");

        let id = generate(&existing);
        assert_eq!(id.len(), INITIAL_LENGTH as usize);
        assert!(!existing.contains(id.as_str()));
    }

    #[test]
    fn large_set_increases_length() {
        // 36^3 = 46656.  Threshold is 50%, so >23328 IDs at length 3 should bump.
        let half_space = CHARSET_SIZE.pow(INITIAL_LENGTH) as usize / 2_usize;
        let mut existing: HashSet<&str> = HashSet::new();

        let charset = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let mut count = 0_usize;
        'outer: for ch_a in charset {
            for ch_b in charset {
                for ch_c in charset {
                    if count > half_space {
                        break 'outer;
                    }
                    let id_str = String::from_utf8(vec![*ch_a, *ch_b, *ch_c]).unwrap();
                    let leaked: &'static str = Box::leak(id_str.into_boxed_str());
                    existing.insert(leaked);
                    count += 1;
                }
            }
        }

        let length = pick_length(&existing);
        assert_eq!(
            length, 4_u32,
            "expected length 4 when >50% of 3-char space is used"
        );

        let id = generate(&existing);
        assert_eq!(id.len(), 4_usize);
        assert!(!existing.contains(id.as_str()));
    }

    #[test]
    fn character_set_valid() {
        let existing: HashSet<&str> = HashSet::new();
        for _ in 0_i32..1000_i32 {
            let id = generate(&existing);
            for ch in id.chars() {
                assert!(
                    ch.is_ascii_lowercase() || ch.is_ascii_digit(),
                    "unexpected character: {ch}"
                );
            }
        }
    }

    #[test]
    fn no_collision_sequential() {
        let mut existing: HashSet<&str> = HashSet::new();
        let mut generated = Vec::new();

        for _ in 0_i32..100_i32 {
            let id = generate(&existing);
            assert!(!existing.contains(id.as_str()), "collision detected: {id}");
            let leaked: &'static str = Box::leak(id.clone().into_boxed_str());
            existing.insert(leaked);
            generated.push(id);
        }

        let unique: HashSet<&str> = generated.iter().map(String::as_str).collect();
        assert_eq!(unique.len(), 100_usize);
    }
}
