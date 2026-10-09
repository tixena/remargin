//! Tests for the op-name partitions and the kebab-case wire form.

use super::OpName;

#[test]
fn all_covers_every_variant() {
    assert_eq!(OpName::READ.len() + OpName::WRITE.len(), OpName::ALL.len());
}

#[test]
fn read_and_write_are_disjoint() {
    for read in OpName::READ {
        assert!(
            !OpName::WRITE.contains(read),
            "{read} appears in both READ and WRITE"
        );
    }
}

/// Every member of `ALL` is on exactly one of `READ` / `WRITE`.
#[test]
fn every_op_classified() {
    for op in OpName::ALL {
        let on_read = OpName::READ.contains(op);
        let on_write = OpName::WRITE.contains(op);
        assert!(
            on_read ^ on_write,
            "{op} must appear on exactly one of READ / WRITE"
        );
    }
}

/// Wire form matches the kebab-case rename.
#[test]
fn as_str_matches_kebab_serialisation() {
    for op in OpName::ALL {
        let serialised = serde_yaml::to_string(op).unwrap();
        // serde_yaml renders a bare scalar with a trailing newline.
        let expected = format!("{}\n", op.as_str());
        assert_eq!(serialised, expected, "serialised form for {op}");
    }
}

/// The error names the offending value and lists the valid names.
#[test]
fn unknown_op_rejected_on_deserialise() {
    let result: Result<OpName, _> = serde_yaml::from_str("purg");
    let err = result.unwrap_err().to_string();
    assert!(err.contains("purg"), "error did not name typo: {err}");
}

#[test]
fn valid_names_csv_alphabetical() {
    let csv = OpName::valid_names_csv();
    let names: Vec<&str> = csv.split(", ").collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
    assert_eq!(names.len(), OpName::ALL.len());
    assert!(names.contains(&"purge"));
    assert!(names.contains(&"sandbox-add"));
}
