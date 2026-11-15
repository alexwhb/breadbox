//! TEST-4 · plugins the way the Plugins screen uses them (PLUG-1…5): a fresh registry shows the
//! catalog (first-party preinstalled, the rest installable, the Pi HAT host planned); installing
//! the 4-channel scope from the catalog and placing it on garden-node measures the night LED's
//! PWM; a `.bbplugin` package installs with its permissions; a local plugin hot-reloads;
//! disable / uninstall take plugins off the bench.

use bb_circuit::project::{Endpoint, PartInstance, Wire};
use bb_plugin::registry::CatalogStatus;
use breadbox_e2e::*;

fn status(reg: &bb_plugin::Registry, id: &str) -> CatalogStatus {
    reg.catalog()
        .into_iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("{id} not in the catalog"))
        .status
}

#[test]
fn install_the_scope_from_the_catalog_and_measure_on_the_bench() {
    let scratch = TempDir::new("plug-scope");
    let mut reg = plugin_registry(scratch.path());
    assert_eq!(
        status(&reg, "community.dmm-handheld"),
        CatalogStatus::Installed
    );
    assert_eq!(
        status(&reg, "breadbox.soil-weather"),
        CatalogStatus::Installed
    );
    assert_eq!(status(&reg, "breadbox.scope-4ch"), CatalogStatus::Available);
    assert!(matches!(
        status(&reg, "breadbox.pi-hat-host"),
        CatalogStatus::Planned { .. }
    ));
    let card = reg
        .catalog()
        .into_iter()
        .find(|c| c.id == "breadbox.scope-4ch")
        .unwrap();
    assert!(
        !card.permissions.is_empty(),
        "the install prompt lists permissions"
    );
    // installing without the permissions it asks for is refused
    assert!(reg.install_bundled(&card.id, &[]).is_err());
    reg.install_bundled(&card.id, &card.permissions).unwrap();
    assert_eq!(status(&reg, &card.id), CatalogStatus::Installed);
    // the state survives reopening the registry (app restart)
    let mut reg = plugin_registry(scratch.path());
    assert_eq!(status(&reg, &card.id), CatalogStatus::Installed);

    let host = bb_plugin::PluginHost::new(&mut reg, None);
    assert!(host.errors.is_empty(), "{:?}", host.errors);
    let scope = host
        .instruments()
        .into_iter()
        .find(|i| i.plugin == card.id)
        .expect("scope instrument");
    assert!(scope.panel.is_some(), "front panel declared");
    // "Place on bench" + clip CH1 to D9 / GND to the ground rail
    let (mut p, dir) = example("garden-node");
    p.parts
        .push(PartInstance::new("scope2", "SC2", &scope.part).at(80.0, 10.0));
    for (id, a, b) in [
        ("ws1", ("scope2", "CH1"), ("u1", "D9")),
        ("ws2", ("scope2", "GND"), ("bb1", "t-1")),
    ] {
        p.wires.push(Wire {
            id: id.into(),
            a: Endpoint::pin(a.0, a.1),
            b: Endpoint::pin(b.0, b.1),
            color: "yellow".into(),
            bends: vec![],
        });
    }
    let mut opts = host.bench_options();
    opts.firmware = bb_sim::firmware::project_images(&p, &dir);
    let mut sim = Sim::spawn(p, host.library(), opts);
    sim.run_secs(3.0);
    let f = sim.reading("scope2", "frequency").unwrap();
    let duty = sim.reading("scope2", "duty").unwrap();
    assert!((f - 490.2).abs() < 2.0, "scope frequency {f}");
    assert!((duty - 0.478).abs() < 0.02, "scope duty {duty}");
    // the bench's own mini scope agrees
    let mini = &sim.tel().scopes[0];
    assert!((mini.duty - duty).abs() < 0.01, "{} vs {duty}", mini.duty);
    let panel = sim.part("scope2").panel.expect("live panel");
    assert!(panel["screens"].is_object(), "{panel}");
    sim.shutdown();

    // disabled → not on the bench any more; uninstall → back to Available
    reg.set_enabled(&card.id, false).unwrap();
    assert_eq!(status(&reg, &card.id), CatalogStatus::Disabled);
    let host = bb_plugin::PluginHost::new(&mut reg, None);
    assert!(host.instruments().iter().all(|i| i.plugin != card.id));
    reg.set_enabled(&card.id, true).unwrap();
    reg.uninstall(&card.id).unwrap();
    assert_eq!(status(&reg, &card.id), CatalogStatus::Available);
}

#[test]
fn bbplugin_package_installs_and_local_plugins_hot_reload() {
    let scratch = TempDir::new("plug-pkg");
    let mut reg = plugin_registry(&scratch.path().join("user"));
    // a .bbplugin built from the SDK's example part
    let src = root().join("plugins/examples");
    let example = std::fs::read_dir(&src)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|d| d.join("plugin.json").exists())
        .find(|d| {
            bb_plugin::manifest::Manifest::load(d)
                .is_ok_and(|m| m.kind.contains(&bb_plugin::Kind::Part))
        })
        .expect("an SDK example part plugin");
    let bytes = bb_plugin::package::pack(&example).unwrap();
    let m = bb_plugin::package::inspect(&bytes).unwrap();
    let rep = reg.install_package(&bytes, &m.permissions, false).unwrap();
    assert_eq!(rep.id, m.id);
    assert_eq!(status(&reg, &m.id), CatalogStatus::Installed);
    let host = bb_plugin::PluginHost::new(&mut reg, None);
    assert!(host.errors.is_empty(), "{:?}", host.errors);
    let parts = host.parts_of(&m.id);
    assert!(!parts.is_empty(), "the package adds parts");
    assert!(host.library().get(&parts[0]).is_some());

    // "Load local plugin…": a copy of the DMM folder, then an edit on disk → hot reload
    let local = scratch.path().join("my-dmm");
    copy_dir(&root().join("plugins/dmm-handheld"), &local);
    let manifest_path = local.join("plugin.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    json["id"] = "local.my-dmm".into();
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    let lm = bb_plugin::manifest::Manifest::load(&local).unwrap();
    reg.load_local(&local, &lm.permissions).unwrap();
    assert_eq!(status(&reg, "local.my-dmm"), CatalogStatus::Installed);
    assert!(reg.check_changes().is_empty(), "nothing changed yet");
    // (the change stamp includes the file size, so no wait for the mtime to tick)
    json["name"] = "My DMM (edited)".into();
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    let changed = reg.check_changes();
    assert_eq!(changed, vec!["local.my-dmm".to_string()]);
    assert_eq!(
        reg.get("local.my-dmm").unwrap().manifest.name,
        "My DMM (edited)"
    );
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        if p.file_name().is_some_and(|n| n == "target" || n == "src") {
            continue;
        }
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()));
        } else {
            std::fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}
