//! Install-tier checks of the asset access every derivation builds on.

use orpheus_assets::Install;
use orpheus_stu::Value;

/// A hero's name string reads as DataTool shows it, through the hero asset.
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn a_hero_name_reads_from_its_hero_asset() {
    let install = Install::open(&orpheus_testkit::game_dir(), "enUS").unwrap();
    let reaper = install.stu(0x02e0_0000_0000_0002).unwrap();
    let Some(Value::Guid(name)) = reaper.root().unwrap().get("m_0EDCE350") else {
        panic!("Reaper's hero asset has no name string");
    };
    assert_eq!(install.string(*name).as_deref(), Some("Reaper"));
    assert_eq!(install.string(0), None);
}
