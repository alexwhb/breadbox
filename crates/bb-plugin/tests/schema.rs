//! `docs/plugin.schema.json` stays in sync with the manifest types: enums match, every key the
//! bundled manifests use is declared, every bundled manifest validates, and the design's
//! `plugin.json` (Handheld DMM detail page) is the schema's example.

mod common;
use bb_plugin::manifest::{Kind, Manifest, Permission};
use common::*;
use std::collections::BTreeSet;

fn schema() -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(root().join("docs/plugin.schema.json")).unwrap())
        .unwrap()
}

fn manifests() -> Vec<std::path::PathBuf> {
    let mut v = Vec::new();
    for base in ["plugins", "plugins/examples"] {
        for e in std::fs::read_dir(root().join(base)).unwrap().flatten() {
            if e.path().join("plugin.json").exists() {
                v.push(e.path());
            }
        }
    }
    v.sort();
    v
}

#[test]
fn schema_enums_match_the_types() {
    let s = schema();
    let kinds: BTreeSet<String> = s["properties"]["kind"]["items"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect();
    let want: BTreeSet<String> = Kind::ALL.iter().map(|k| k.id().to_string()).collect();
    assert_eq!(kinds, want);
    let perms: BTreeSet<String> = s["properties"]["permissions"]["items"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect();
    let want: BTreeSet<String> = Permission::ALL.iter().map(|p| p.id().to_string()).collect();
    assert_eq!(perms, want);
}

#[test]
fn every_bundled_manifest_validates_and_uses_only_declared_keys() {
    let s = schema();
    let top: BTreeSet<&str> = s["properties"].as_object().unwrap().keys().map(|k| k.as_str()).collect();
    let prov: BTreeSet<&str> = s["properties"]["provides"]["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    let dirs = manifests();
    assert!(dirs.len() >= 12, "{dirs:?}");
    for d in dirs {
        let text = std::fs::read_to_string(d.join("plugin.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        for k in v.as_object().unwrap().keys() {
            assert!(top.contains(k.as_str()), "{}: `{k}` not in the schema", d.display());
        }
        for k in v["provides"].as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default() {
            assert!(prov.contains(k.as_str()), "{}: provides.`{k}` not in the schema", d.display());
        }
        Manifest::load(&d).unwrap_or_else(|e| panic!("{}: {e}", d.display()));
    }
}

#[test]
fn the_design_manifest_is_the_schema_example() {
    let s = schema();
    let dmm: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("plugins/dmm-handheld/plugin.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(s["examples"][0], dmm);
    // the fields the design's read-only pane shows
    assert_eq!(dmm["id"], "community.dmm-handheld");
    assert_eq!(dmm["version"], "1.4.0");
    assert_eq!(dmm["kind"], serde_json::json!(["instrument"]));
    assert_eq!(dmm["provides"]["instrument"], "Handheld DMM");
    assert_eq!(dmm["provides"]["probes"], 2);
    assert_eq!(dmm["spice"]["load"], "R 10Meg ∥ C 100p");
    assert_eq!(dmm["specs"]["counts"], 6000);
    assert_eq!(dmm["specs"]["dcv_ranges"], serde_json::json!(["600m", "6", "60", "600"]));
    assert_eq!(dmm["specs"]["dcv_accuracy"], "0.05% + 1");
    assert_eq!(dmm["specs"]["autorange_ms"], 350);
    assert_eq!(dmm["permissions"], serde_json::json!(["read:nets", "inject:load"]));
}
