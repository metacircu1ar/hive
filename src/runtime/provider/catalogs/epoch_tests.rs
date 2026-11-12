use super::*;

#[test]
fn invalidation_fences_in_flight_publication_without_discarding_its_live_result() {
    let catalogs = Arc::new(Catalogs::new(
        Runtime::default(),
        Arc::default(),
        Arc::default(),
        Arc::default(),
    ));
    let generation = uuid::Uuid::new_v4();
    let unrelated = uuid::Uuid::new_v4();
    let binary = BinaryIdentity::read(&std::env::current_exe().unwrap()).unwrap();
    let (started, entered) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let owner = catalogs.clone();
    let identity = binary.clone();
    let producer = std::thread::spawn(move || {
        let epoch = owner.epoch();
        let models = vec![Model::parse(&serde_json::json!({"model":"live-result"}))];
        started.send(()).unwrap();
        released.recv().unwrap();
        owner.observe(Observation {
            epoch,
            generation,
            key: None,
            binary: identity,
            models: models.clone(),
        });
        models
    });
    entered.recv().unwrap();
    catalogs.invalidate(unrelated);
    release.send(()).unwrap();
    let models = producer.join().unwrap();
    assert_eq!(models[0].name(), Some("live-result"));
    assert!(catalogs.cached(generation, None, &binary).is_none());
    catalogs.observe(Observation {
        epoch: catalogs.epoch(),
        generation,
        key: None,
        binary: binary.clone(),
        models,
    });
    assert_eq!(
        catalogs.cached(generation, None, &binary).unwrap()[0].name(),
        Some("live-result")
    );
    catalogs.invalidate(generation);
    assert!(catalogs.cached(generation, None, &binary).is_none());
}
