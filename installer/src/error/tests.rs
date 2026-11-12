use super::*;

fn observed(error: &(dyn NativeError + 'static)) -> serde_json::Value {
    serde_json::to_value(hive_log::Causes::classify(error)).unwrap()
}

#[test]
fn concrete_io_and_json_observation_retains_the_original_type_and_metadata() {
    let original = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    let expected = observed(&original);
    let message = original.to_string();
    let error = Error::from(original);
    assert_eq!(error.to_string(), message);
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert_eq!(observed(error.as_ref()), expected);
    assert!(error.source().is_none());

    let original = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
    let expected = observed(&original);
    let message = original.to_string();
    let error = Error::from(original);
    assert_eq!(error.to_string(), message);
    assert!(error.downcast_ref::<serde_json::Error>().is_some());
    assert_eq!(observed(error.as_ref()), expected);
}

#[test]
fn input_metadata_failures_keep_their_separate_argument_classification() {
    let input = hive_input::InputError::new(
        hive_input::Code::InvalidPath,
        "Invalid path.",
        "--prefix",
        "an absolute path",
        vec!["hive-install --help".into()],
    )
    .unwrap();
    let expected = serde_json::to_value(input.wire()).unwrap();
    let error = Error::from(input);
    assert_eq!(
        serde_json::to_value(
            error
                .downcast_ref::<hive_input::InputError>()
                .unwrap()
                .wire()
        )
        .unwrap(),
        expected
    );
    let error = Error::from(hive_input::InvalidMetadata::Message);
    assert!(error.downcast_ref::<hive_input::InputError>().is_none());
    assert!(
        error
            .downcast_ref::<hive_input::InvalidMetadata>()
            .is_some()
    );
    assert_eq!(error.to_string(), "Invalid input-error message.");
}

#[test]
fn std_io_bridge_preserves_native_payload_and_source_classification() {
    let original = std::io::Error::other("inner detail");
    let baseline = std::io::Error::other(Box::new(original) as Box<dyn NativeError + Send + Sync>);
    let error = Error::from(std::io::Error::other("inner detail")).into_io_other();
    assert_eq!(error.to_string(), baseline.to_string());
    assert_eq!(error.kind(), baseline.kind());
    assert_eq!(observed(&error), observed(&baseline));
    assert_eq!(
        error
            .get_ref()
            .map(<dyn NativeError + Send + Sync>::is::<std::io::Error>),
        baseline
            .get_ref()
            .map(<dyn NativeError + Send + Sync>::is::<std::io::Error>)
    );

    let original = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
    let baseline = std::io::Error::other(Box::new(original) as Box<dyn NativeError + Send + Sync>);
    let error =
        Error::from(serde_json::from_str::<serde_json::Value>("{").unwrap_err()).into_io_other();
    assert!(error.get_ref().unwrap().is::<serde_json::Error>());
    assert_eq!(error.to_string(), baseline.to_string());
    assert_eq!(observed(&error), observed(&baseline));
}

#[test]
fn contextual_primary_cleanup_and_asset_rendering_keep_the_original_chain() {
    let primary = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    let native_text = primary.to_string();
    let context = crate::publication::context("Prepare", primary.into());
    assert_eq!(context.to_string(), format!("Prepare: {native_text}"));
    assert!(context.source().unwrap().is::<std::io::Error>());
    let cleanup = crate::publication::failed(context, "Cleanup", Err("secondary".into()));
    assert_eq!(
        cleanup.to_string(),
        format!("Prepare: {native_text}. Cleanup: secondary; recovery evidence retained")
    );
    assert!(
        cleanup
            .source()
            .unwrap()
            .is::<crate::publication::ContextFailure>()
    );
    assert!(
        cleanup
            .source()
            .unwrap()
            .source()
            .unwrap()
            .is::<std::io::Error>()
    );
    let asset = crate::manifest::asset("asset.tar.gz".into(), cleanup);
    assert_eq!(
        asset.to_string(),
        format!(
            "Release asset asset.tar.gz: Prepare: {native_text}. \
            Cleanup: secondary; recovery evidence retained"
        )
    );
    assert!(
        asset
            .source()
            .unwrap()
            .is::<crate::publication::CleanupFailure>()
    );
}

#[test]
fn borrowed_and_owned_human_labels_render_identically_through_the_std_bridge() {
    for error in [
        Error::from("static label"),
        Error::from(String::from("owned label")),
    ] {
        let label = error.to_string();
        let error = error.into_io_other();
        assert_eq!(error.to_string(), label);
        assert!(error.source().is_none());
    }
}
