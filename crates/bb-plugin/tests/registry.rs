//! Registry / catalog (PLUG-1, PLUG-4): `.bbplugin` install with the permission grant,
//! version checks, uninstall, enable/disable, state persistence, bundled catalog with
//! preinstalled + planned entries, and hot reload of local plugin folders.

mod common;
use bb_plugin::registry::{CatalogStatus, Registry, Source};
use bb_plugin::{Permission, PluginError, PluginHost, package};
use common::*;

const WAT: &str = r#"(module (memory (export "memory") 1)
  (func (export "bb_abi_version") (result i32) i32.const 1)
  (func (export "bb_alloc") (param i32) (result i32) i32.const 1024))"#;

const PARTS: &str = r#"[{"id": "plugpart", "name": "Plugin part", "category": "sensors",
  "description": "test", "ref_prefix": "X", "pins": ["A:passive", "B:passive"],
  "model": {"spice": ["$(el:R) $(A) $(B) 1k"]},
  "bench": {"layout": "row", "off_board": true, "size": [4, 2]}}]"#;

fn set_version(dir: &std::path::Path, v: &str) {
    let p = dir.join("plugin.json");
    let mut m: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    m["version"] = v.into();
    std::fs::write(p, serde_json::to_string_pretty(&m).unwrap()).unwrap();
}

#[test]
fn package_install_needs_the_grant_refuses_downgrades_and_uninstalls() {
    let tmp = Tmp::new("reg");
    let src = tmp.plugin(
        "acme.widget",
        r#"["part"]"#,
        r#"["read:nets", "world:read"]"#,
        WAT,
        Some(PARTS),
        "",
    );
    let pkg = package::pack(&src).unwrap();
    // inspect for the prompt
    let m = package::inspect(&pkg).unwrap();
    assert_eq!(m.id, "acme.widget");
    assert_eq!(m.permissions, [Permission::ReadNets, Permission::WorldRead]);
    let user = tmp.dir("user");
    let mut reg = Registry::open(None, &user);
    // the user must approve every permission
    let e = reg
        .install_package(&pkg, &[Permission::ReadNets], false)
        .unwrap_err();
    assert!(
        matches!(e, PluginError::NotGranted { ref missing, .. } if missing == &[Permission::WorldRead]),
        "{e}"
    );
    let r = reg
        .install_package(&pkg, &[Permission::ReadNets, Permission::WorldRead], false)
        .unwrap();
    assert_eq!(
        (r.id.as_str(), r.version.as_str(), r.replaced.as_deref()),
        ("acme.widget", "1.0.0", None)
    );
    assert!(user.join("acme.widget/plugin.json").exists());
    assert!(user.join("acme.widget/model.wasm").exists());
    let p = reg.get("acme.widget").unwrap();
    assert_eq!(p.source, Source::User);
    assert!(p.active());
    // the host loads its part
    let host = PluginHost::new(&mut reg, None);
    assert!(host.errors.is_empty(), "{:?}", host.errors);
    assert!(host.library().get("plugpart").is_some());
    // upgrade 1.0.0 → 1.2.0
    set_version(&src, "1.2.0");
    let up = package::pack(&src).unwrap();
    let r = reg
        .install_package(&up, &[Permission::ReadNets, Permission::WorldRead], false)
        .unwrap();
    assert_eq!(r.replaced.as_deref(), Some("1.0.0"));
    assert_eq!(reg.get("acme.widget").unwrap().manifest.version, "1.2.0");
    // downgrade refused
    let e = reg
        .install_package(&pkg, &[Permission::ReadNets, Permission::WorldRead], false)
        .unwrap_err();
    assert!(matches!(e, PluginError::Downgrade { .. }), "{e}");
    // disable / state persists across reopen
    reg.set_enabled("acme.widget", false).unwrap();
    let reg2 = Registry::open(None, &user);
    let p = reg2.get("acme.widget").unwrap();
    assert!(!p.enabled && !p.active());
    assert_eq!(
        reg2.catalog()
            .iter()
            .find(|c| c.id == "acme.widget")
            .unwrap()
            .status,
        CatalogStatus::Disabled
    );
    // uninstall
    let mut reg = reg2;
    reg.uninstall("acme.widget").unwrap();
    assert!(reg.get("acme.widget").is_none());
    assert!(!user.join("acme.widget").exists());
}

#[test]
fn hostile_packages_are_rejected() {
    use std::io::Write;
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    w.start_file("../../evil.sh", o).unwrap();
    w.write_all(b"rm -rf /").unwrap();
    w.start_file("plugin.json", o).unwrap();
    w.write_all(
        br#"{"id":"evil.pkg","version":"1.0.0","kind":["theme"],"provides":{"theme":"t.json"}}"#,
    )
    .unwrap();
    let bytes = w.finish().unwrap().into_inner();
    let e = package::inspect(&bytes).unwrap_err();
    assert!(e.to_string().contains("unsafe path"), "{e}");
    assert!(package::inspect(b"not a zip").is_err());
    // no manifest
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    w.start_file("readme.txt", o).unwrap();
    w.write_all(b"hi").unwrap();
    let bytes = w.finish().unwrap().into_inner();
    assert!(
        package::inspect(&bytes)
            .unwrap_err()
            .to_string()
            .contains("no plugin.json")
    );
}

#[test]
fn local_plugins_hot_reload_on_file_change() {
    let tmp = Tmp::new("hot");
    let src = tmp.plugin("dev.thing", r#"["part"]"#, "[]", WAT, Some(PARTS), "");
    let user = tmp.dir("user");
    let mut reg = Registry::open(None, &user);
    reg.load_local(&src, &[]).unwrap();
    let p = reg.get("dev.thing").unwrap();
    assert_eq!(p.source, Source::Local);
    assert!(reg.check_changes().is_empty());
    // edit the part file and bump the version
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(
        src.join("parts.json"),
        PARTS.replace("Plugin part", "Plugin part v2"),
    )
    .unwrap();
    set_version(&src, "1.0.1");
    let changed = reg.check_changes();
    assert_eq!(changed, ["dev.thing"]);
    assert_eq!(reg.get("dev.thing").unwrap().manifest.version, "1.0.1");
    let host = PluginHost::new(&mut reg, None);
    assert_eq!(
        host.library().get("plugpart").unwrap().name,
        "Plugin part v2"
    );
    assert!(reg.check_changes().is_empty());
    // forgetting it
    reg.uninstall("dev.thing").unwrap();
    assert!(reg.get("dev.thing").is_none());
    assert!(
        src.join("plugin.json").exists(),
        "local sources are never deleted"
    );
}

#[test]
#[ignore = "enabled once every first-party plugin is built"]
fn bundled_catalog_has_the_design_entries() {
    let tmp = Tmp::new("bundled");
    let user = tmp.dir("user");
    let bundled = root().join("plugins");
    let mut reg = Registry::open(Some(&bundled), &user);
    assert!(reg.load_errors.is_empty(), "{:?}", reg.load_errors);
    let cat = reg.catalog();
    let names: Vec<(&str, &CatalogStatus)> =
        cat.iter().map(|c| (c.name.as_str(), &c.status)).collect();
    // preinstalled first (design: Handheld DMM, Soil & weather pack)
    assert_eq!(
        names[0],
        ("Handheld DMM", &CatalogStatus::Installed),
        "{names:#?}"
    );
    assert_eq!(
        names[1],
        ("Soil & weather pack", &CatalogStatus::Installed),
        "{names:#?}"
    );
    for n in [
        "4-channel oscilloscope",
        "Logic analyzer",
        "Bench power supply",
        "Function generator",
        "LoRa + virtual gateway",
    ] {
        let c = cat
            .iter()
            .find(|c| c.name == n)
            .unwrap_or_else(|| panic!("{n} missing"));
        assert_eq!(c.status, CatalogStatus::Available, "{n}");
    }
    let pi = cat.iter().find(|c| c.name == "Pi 40-pin HAT host").unwrap();
    assert!(matches!(pi.status, CatalogStatus::Planned { .. }));
    assert_eq!(pi.chip, "BOARD");
    // install the scope from the catalog with its permissions
    let scope = cat
        .iter()
        .find(|c| c.name == "4-channel oscilloscope")
        .unwrap()
        .clone();
    assert!(reg.install_bundled(&scope.id, &[]).is_err());
    reg.install_bundled(&scope.id, &scope.permissions).unwrap();
    assert!(reg.get(&scope.id).unwrap().active());
    let host = PluginHost::new(&mut reg, None);
    assert!(host.errors.is_empty(), "{:?}", host.errors);
    let ids = host.ids();
    assert!(
        ids.contains(&scope.id) && ids.contains(&"community.dmm-handheld".to_string()),
        "{ids:?}"
    );
    // uninstalling a bundled plugin returns it to "Available"
    reg.uninstall(&scope.id).unwrap();
    assert_eq!(
        reg.catalog()
            .iter()
            .find(|c| c.id == scope.id)
            .unwrap()
            .status,
        CatalogStatus::Available
    );
}
