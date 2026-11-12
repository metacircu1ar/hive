use super::*;

fn interrupt_upgrade(world: &World, point: &str) {
    let part = fixture(world.temp.path(), "hive-core");
    let mut services = world.services();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        publish::install_with(
            &world.prefix,
            world.temp.path(),
            &[part],
            &mut services,
            |at| {
                assert_ne!(at, point, "simulated process loss");
                Ok(())
            },
        )
    }));
    assert!(result.is_err());
}

#[test]
fn already_restored_pointer_repeats_barrier_without_preparing_a_replacement() {
    let world = World::new();
    let manager = world.prefix.join("lib/hive");
    let old = publish::current(&manager).unwrap();
    let observed = Rc::new(Cell::new(false));
    let created = observed.clone();
    let mut synced = false;
    crate::publication::with_creation_hook(
        move |_| {
            created.set(true);
            Err(std::io::Error::other("pointer preparation must not occur"))
        },
        || {
            publish::rollback(&manager, old.as_deref(), &mut |phase| {
                synced |= phase == "rollback_sync";
                Ok(())
            })
        },
    )
    .unwrap();
    assert!(synced);
    assert!(!observed.get());
    assert_eq!(publish::current(&manager).unwrap(), old);
}

#[test]
fn failed_first_service_restoration_still_starts_later_services_and_retains_journal() {
    let world = World::new();
    world
        .manager
        .loaded
        .borrow_mut()
        .insert(world.stopped.clone());
    let specs = inventory(&world.layout, &world.prefix, &["hive".into()]).unwrap();
    let first = specs[0].scope.label().unwrap();
    let second = specs[1].scope.label().unwrap();
    world
        .manager
        .fail_start_for
        .borrow_mut()
        .insert(first.clone());
    let part = fixture(world.temp.path(), "hive-core");
    let error = publish::install_with(
        &world.prefix,
        world.temp.path(),
        &[part],
        &mut world.services(),
        |_| Ok(()),
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("service restart is incomplete"));
    {
        let attempts = world.manager.start_attempts.borrow();
        assert!(attempts.contains(&first));
        assert!(attempts.contains(&second));
    }
    assert!(!world.manager.loaded.borrow().contains(&first));
    assert!(world.manager.loaded.borrow().contains(&second));
    let manager = world.prefix.join("lib/hive");
    assert!(manager.join(PENDING).is_file());
    world.manager.fail_start_for.borrow_mut().clear();
    world.services().recover(&world.prefix, &manager).unwrap();
    assert!(!manager.join(PENDING).exists());
    assert_eq!(world.manager.loaded.borrow().len(), 2);
}

#[test]
fn foreign_added_link_does_not_strand_safe_previous_services_during_rollback() {
    let world = World::new();
    let manager = world.prefix.join("lib/hive");
    let old = publish::current(&manager).unwrap();
    let parts = [
        fixture(world.temp.path(), "hive-core"),
        fixture(world.temp.path(), "comb"),
    ];
    let foreign = world.prefix.join("bin/comb");
    let error = publish::install_with(
        &world.prefix,
        world.temp.path(),
        &parts,
        &mut world.services(),
        |at| {
            if at == "link_created" {
                fs::remove_file(&foreign)?;
                fs::write(&foreign, b"foreign")?;
                return Err("original publication failure".into());
            }
            Ok(())
        },
    )
    .err()
    .unwrap();
    assert!(
        error
            .to_string()
            .starts_with("original publication failure")
    );
    assert_eq!(publish::current(&manager).unwrap(), old);
    assert!(world.manager.loaded.borrow().contains(&world.running));
    assert!(!world.manager.loaded.borrow().contains(&world.stopped));
    assert_eq!(fs::read(&foreign).unwrap(), b"foreign");
    assert!(manager.join(PENDING).is_file());
    assert_eq!(
        publish::usage(&manager.join("generations"))
            .unwrap()
            .generations,
        2
    );
    assert!(world.services().recover(&world.prefix, &manager).is_err());
    assert!(world.manager.loaded.borrow().contains(&world.running));
    fs::remove_file(foreign).unwrap();
    world.services().recover(&world.prefix, &manager).unwrap();
    assert!(!manager.join(PENDING).exists());
}

#[test]
fn independently_uninstalled_service_is_not_recreated_by_upgrade_recovery() {
    for point in ["before_replace", "after_replace"] {
        let world = World::new();
        let spec = inventory(&world.layout, &world.prefix, &["hive".into()])
            .unwrap()
            .into_iter()
            .find(|s| s.scope.label().unwrap() == world.running)
            .unwrap();
        interrupt_upgrade(&world, point);
        Controller {
            layout: &world.layout,
            scope: &spec.scope,
            runner: &world.manager,
        }
        .uninstall(&|| Ok(false))
        .unwrap();
        let manager = world.prefix.join("lib/hive");
        world.services().recover(&world.prefix, &manager).unwrap();
        assert!(!spec.unit.exists());
        assert!(!spec.descriptor.exists());
        assert!(!world.manager.loaded.borrow().contains(&world.running));
        assert!(!manager.join(PENDING).exists());
    }
}

#[test]
fn absent_descriptor_is_not_removal_proof_when_unit_job_or_observation_remains() {
    for obstruction in ["unit", "job", "unavailable", "unknown", "dangling-unit"] {
        let world = World::new();
        let spec = inventory(&world.layout, &world.prefix, &["hive".into()])
            .unwrap()
            .remove(0);
        interrupt_upgrade(&world, "after_replace");
        fs::remove_file(&spec.descriptor).unwrap();
        if obstruction != "unit" {
            fs::remove_file(&spec.unit).unwrap();
        }
        match obstruction {
            "job" => {
                world
                    .manager
                    .loaded
                    .borrow_mut()
                    .insert(spec.scope.label().unwrap());
            }
            "unavailable" => world.manager.unavailable.set(true),
            "unknown" => world.manager.unknown_state.set(true),
            "dangling-unit" => std::os::unix::fs::symlink("absent", &spec.unit).unwrap(),
            "unit" => (),
            _ => unreachable!(),
        }
        let manager = world.prefix.join("lib/hive");
        let before = fs::read(manager.join(PENDING)).unwrap();
        assert!(world.services().recover(&world.prefix, &manager).is_err());
        assert_eq!(fs::read(manager.join(PENDING)).unwrap(), before);
        assert!(!spec.descriptor.exists());
    }
}

#[test]
fn committed_uninstall_never_probes_a_foreign_removed_command() {
    let world = World::new();
    let specs = inventory(&world.layout, &world.prefix, &["hive".into()]).unwrap();
    let mut services = world.services();
    let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::uninstall::remove_with(
            &world.prefix,
            &["hive".into()],
            false,
            &mut services,
            |at| {
                assert_ne!(at, "after_replace", "simulated process loss");
                Ok(())
            },
        )
    }));
    assert!(crash.is_err());
    drop(services);
    // An independent re-registration after process loss must not make a stale
    // transaction execute an edited entrypoint during its health check.
    let spec = &specs[0];
    files::write(&spec.unit, spec.unit_contents.as_bytes(), false).unwrap();
    files::write(&spec.descriptor, &files::encode(spec).unwrap(), false).unwrap();
    let foreign = world.prefix.join("bin/hivectl");
    fs::remove_file(&foreign).unwrap();
    fs::write(&foreign, b"foreign command").unwrap();
    let probes = Rc::new(Cell::new(0));
    let called = probes.clone();
    let mut recovery = world.services();
    recovery.health = Box::new(move |_| {
        called.set(called.get() + 1);
        Err("foreign health callback reached".into())
    });
    let manager = world.prefix.join("lib/hive");
    let before = fs::read(manager.join(PENDING)).unwrap();
    assert!(recovery.recover(&world.prefix, &manager).is_err());
    assert_eq!(probes.get(), 0);
    assert_eq!(fs::read(&foreign).unwrap(), b"foreign command");
    assert_eq!(fs::read(manager.join(PENDING)).unwrap(), before);
    assert!(spec.descriptor.is_file());
    assert!(spec.unit.is_file());
}
