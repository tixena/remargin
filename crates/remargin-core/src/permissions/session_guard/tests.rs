use std::path::Path;

use os_shim::mock::MockSystem;

use super::{GuardDiagnostic, GuardDiagnosticInner, GuardOutcome, session_guard};

/// A mock whose `PATH` contains a directory holding a `remargin` file, so
/// the on-PATH check passes and only the config check can fail.
fn mock_with_remargin_on_path() -> MockSystem {
    MockSystem::new()
        .with_dir(Path::new("/usr/bin"))
        .unwrap()
        .with_file(Path::new("/usr/bin/remargin"), b"")
        .unwrap()
        .with_env("PATH", "/usr/bin")
        .unwrap()
}

/// Destructure a `Fail` outcome without a `panic!` (denied by clippy). The
/// `matches!` assert carries the diagnostic; the else arm is unreachable.
fn expect_fail(outcome: GuardOutcome) -> GuardDiagnostic {
    assert!(
        matches!(outcome, GuardOutcome::Fail(_)),
        "expected GuardOutcome::Fail, got {outcome:?}",
    );
    let GuardOutcome::Fail(diagnostic) = outcome else {
        return GuardDiagnostic {
            hook_specific_output: GuardDiagnosticInner {
                additional_context: String::new(),
                hook_event_name: "SessionStart",
            },
            system_message: String::new(),
        };
    };
    diagnostic
}

/// Case 4: an unparseable realm `.remargin.yaml` above cwd → the guard
/// fails and surfaces a diagnostic naming the parse failure.
#[test]
fn unparseable_realm_config_fails() {
    let system = mock_with_remargin_on_path()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_file(Path::new("/r/.remargin.yaml"), b": : not valid yaml : :")
        .unwrap();

    let diag = expect_fail(session_guard(&system, Path::new("/r")));
    assert_eq!(diag.hook_specific_output.hook_event_name, "SessionStart");
    assert!(
        diag.hook_specific_output
            .additional_context
            .contains(".remargin.yaml"),
        "diagnostic should name the config: {}",
        diag.hook_specific_output.additional_context,
    );
    assert!(
        diag.system_message.contains("remargin doctor"),
        "system message should point at doctor: {}",
        diag.system_message,
    );
}

/// Binary on PATH + parseable config → the session proceeds clean.
#[test]
fn binary_present_and_config_parses_is_ok() {
    let system = mock_with_remargin_on_path()
        .with_dir(Path::new("/r"))
        .unwrap()
        .with_file(
            Path::new("/r/.remargin.yaml"),
            b"identity: alice\ntype: human\n",
        )
        .unwrap();

    assert_eq!(session_guard(&system, Path::new("/r")), GuardOutcome::Ok);
}

/// No `.remargin.yaml` on the walk is not a failure — an absent realm
/// config parses vacuously.
#[test]
fn no_realm_config_is_ok_when_binary_present() {
    let system = mock_with_remargin_on_path()
        .with_dir(Path::new("/r"))
        .unwrap();

    assert_eq!(session_guard(&system, Path::new("/r")), GuardOutcome::Ok);
}

/// `remargin` absent from every `PATH` entry → the guard fails and the
/// diagnostic explains the fail-open (exit 127, non-blocking) risk.
#[test]
fn binary_not_on_path_fails() {
    let system = MockSystem::new()
        .with_dir(Path::new("/usr/bin"))
        .unwrap()
        .with_env("PATH", "/usr/bin")
        .unwrap()
        .with_dir(Path::new("/r"))
        .unwrap();

    let diag = expect_fail(session_guard(&system, Path::new("/r")));
    assert!(
        diag.hook_specific_output
            .additional_context
            .contains("PATH"),
        "diagnostic should mention PATH: {}",
        diag.hook_specific_output.additional_context,
    );
}

/// A missing `PATH` variable is treated as "not resolvable" → failure.
#[test]
fn missing_path_var_fails() {
    let system = MockSystem::new().with_dir(Path::new("/r")).unwrap();

    assert!(matches!(
        session_guard(&system, Path::new("/r")),
        GuardOutcome::Fail(_)
    ));
}
