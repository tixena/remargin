//! ID generator: short, random, collision-checked identifiers.
//!
//! Every Remargin comment needs a unique short identifier within its document.
//! IDs are random alphanumeric strings that start at 3 characters and grow
//! when the space at the current length becomes more than half full.

use core::iter::repeat_with;
use std::collections::HashSet;

use rand::RngExt as _;

const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

const CHARSET_SIZE: u32 = 36;

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

fn pick_length(existing_ids: &HashSet<&str>) -> u32 {
    let mut length = INITIAL_LENGTH;

    loop {
        let space_size = CHARSET_SIZE.pow(length);
        let ids_at_length = existing_ids
            .iter()
            .filter(|id| id.len() == length as usize)
            .count();

        // More than half full, in integer arithmetic.
        if ids_at_length * 2 <= space_size as usize {
            break;
        }

        length += 1;
    }

    length
}

#[cfg(test)]
mod tests;
