//! Credential completion preserves exact nonce ownership and identity evidence.
use super::*;
use crate::state::{CredentialAttempt, CredentialCompletion};

fn pending(state: &State, binding: &Binding) -> CredentialAttempt {
    let attempt = CredentialAttempt {
        generation: binding.id,
        mutation: Uuid::new_v4(),
        writer: Uuid::new_v4(),
    };
    crate::state::test_support::edit_index(state, |index| {
        let generation = index.generations.get_mut(&binding.id).unwrap();
        generation.credential_mutation = Some(attempt.mutation);
        generation.credential_writer = Some(attempt.writer);
        generation.possibly_authenticated = true;
    });
    attempt
}

#[test]
fn not_started_without_a_mutation_is_an_unchanged_noop_even_for_other_nonces() {
    let (_tmp, state, binding) = fixture();
    let before = fs::read(state.root.join("index.json")).unwrap();
    state
        .finish_credential_attempt(
            CredentialAttempt {
                generation: binding.id,
                mutation: Uuid::new_v4(),
                writer: Uuid::new_v4(),
            },
            CredentialCompletion::NotStarted,
        )
        .unwrap();
    assert_eq!(fs::read(state.root.join("index.json")).unwrap(), before);
}

#[test]
fn matching_not_started_clears_only_pending_nonces() {
    for authenticated in [false, true] {
        let (_tmp, state, binding) = fixture();
        let attempt = pending(&state, &binding);
        crate::state::test_support::edit_index(&state, |index| {
            let generation = index.generations.get_mut(&binding.id).unwrap();
            generation.authenticated = authenticated;
            generation.ever_authenticated = true;
            generation.account_identity = Some("original-account".into());
        });
        let mut expected = state.generations("main").unwrap().remove(0);
        expected.credential_mutation = None;
        expected.credential_writer = None;
        state
            .finish_credential_attempt(attempt, CredentialCompletion::NotStarted)
            .unwrap();
        let actual = state.generations("main").unwrap().remove(0);
        assert_eq!(
            serde_json::to_vec(&actual).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
    }
}

#[test]
fn either_wrong_nonce_refuses_both_completion_kinds_without_saving() {
    for wrong_writer in [false, true] {
        for completion in [
            CredentialCompletion::NotStarted,
            CredentialCompletion::Authenticated(Some("account".into())),
        ] {
            let (_tmp, state, binding) = fixture();
            let mut attempt = pending(&state, &binding);
            if wrong_writer {
                attempt.writer = Uuid::new_v4();
            } else {
                attempt.mutation = Uuid::new_v4();
            }
            let before = fs::read(state.root.join("index.json")).unwrap();
            let error = state
                .finish_credential_attempt(attempt, completion)
                .unwrap_err();
            assert!(matches!(error, Error::RecoveryRequired { .. }));
            assert_eq!(fs::read(state.root.join("index.json")).unwrap(), before);
        }
    }
}

#[test]
fn changed_account_persists_only_the_fence_and_omitted_account_preserves_the_index() {
    for observed in [Some("different".into()), None] {
        let (_tmp, state, binding) = fixture();
        let attempt = pending(&state, &binding);
        crate::state::test_support::edit_index(&state, |index| {
            let generation = index.generations.get_mut(&binding.id).unwrap();
            generation.account_identity = Some("original".into());
            generation.authenticated = true;
            generation.ever_authenticated = true;
        });
        let before = fs::read(state.root.join("index.json")).unwrap();
        let mut expected = state.generations("main").unwrap().remove(0);
        let changed = observed.is_some();
        let error = state
            .finish_credential_attempt(attempt, CredentialCompletion::Authenticated(observed))
            .unwrap_err();
        assert!(matches!(error, Error::RecoveryRequired { .. }));
        if changed {
            expected.credential_fence = true;
            assert_eq!(
                error.message(),
                concat!(
                    "The login account changed. Authenticate an isolated replacement with ",
                    "auth repair --reauthenticate.",
                )
            );
        } else {
            assert_eq!(fs::read(state.root.join("index.json")).unwrap(), before);
            assert_eq!(
                error.message(),
                concat!(
                    "The provider omitted the account identity needed to compare with saved ",
                    "credentials. Retry auth recover; use isolated repair if identity remains",
                    " unavailable.",
                )
            );
        }
        let actual = state.generations("main").unwrap().remove(0);
        assert_eq!(
            serde_json::to_vec(&actual).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
    }
}

#[test]
fn prior_authentication_without_identity_refuses_either_observation_without_saving() {
    for (authenticated, ever_authenticated) in [(true, false), (false, true)] {
        for observed in [Some("observed".into()), None] {
            let (_tmp, state, binding) = fixture();
            let attempt = pending(&state, &binding);
            crate::state::test_support::edit_index(&state, |index| {
                let generation = index.generations.get_mut(&binding.id).unwrap();
                generation.authenticated = authenticated;
                generation.ever_authenticated = ever_authenticated;
                generation.account_identity = None;
            });
            let before = fs::read(state.root.join("index.json")).unwrap();
            let error = state
                .finish_credential_attempt(attempt, CredentialCompletion::Authenticated(observed))
                .unwrap_err();
            assert!(matches!(error, Error::RecoveryRequired { .. }));
            assert_eq!(
                error.message(),
                concat!(
                    "The login account identity could not be verified. Retry auth recover; ",
                    "authenticate an isolated replacement if its identity remains ",
                    "unavailable.",
                )
            );
            assert_eq!(fs::read(state.root.join("index.json")).unwrap(), before);
        }
    }
}
