use ssx_assets::career::save::PROFILE_GROUP;
use ssx_formats::t3db::{Database, MetaSchema};

use super::*;

fn schema() -> SaveSchema {
    let tables: [(&str, &[&str]); 6] = [
        ("attribs", &["NAME", "VALUE"]),
        ("character_stats", &["CHARID", "MONEYEARNED", "NEXTLEVELOFFSET", "MAXACHIEVEDLEVEL", "MAXPURCHASEDLEVEL"]),
        ("equipment", &["NAME"]),
        ("equip_unlockables", &["EQUIPID"]),
        ("equip_generated", &["EQUIPID"]),
        ("explore_scores", &["DROPID", "SCORE"]),
    ];
    let mut xml = String::from("<database name=\"x\" shortname=\"y\" version=\"6\">");
    let mut shorts = Vec::new();
    let mut n = 0;
    for (k, (name, fields)) in tables.iter().enumerate() {
        let short = format!("t{k:03}");
        xml += &format!(
            "<table name=\"{name}\" shortname=\"{short}\" maxinsert=\"100\" savegroups=\"{PROFILE_GROUP}\"><fields>"
        );
        for field in *fields {
            n += 1;
            xml += &format!(
                "<field name=\"{field}\" shortname=\"f{n:03}\" type=\"FIELDTYPE_INTEGER\" depth=\"32\" \
                 rangelow=\"-2147483648\" rangehigh=\"2147483647\" />"
            );
        }
        xml += "</fields></table>";
        shorts.push(short);
    }
    xml += "</database>";
    let meta = MetaSchema::parse(&xml).unwrap();
    let order: Vec<&str> = shorts.iter().map(String::as_str).collect();
    let database = Database::from_meta(&meta, &order).unwrap();
    SaveSchema { database, meta, riders: Vec::new(), items: Default::default(), drops: Vec::new(), audio: None }
}

fn written_by_this_thread() -> u64 {
    let io = std::fs::read_to_string("/proc/thread-self/io").expect("/proc/thread-self/io (the task's I/O counts)");
    io.lines()
        .find_map(|l| l.strip_prefix("wchar:"))
        .and_then(|v| v.trim().parse().ok())
        .expect("a wchar line in /proc/thread-self/io")
}

#[test]
fn the_frames_thread_writes_none_of_the_save() {
    let dir = std::env::temp_dir().join(format!("ssx-save-slot-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    let path = dir.join("Saves").join("ssxvprofile");
    let (slot, profile, _) = SaveSlot::load(path.clone(), schema()).unwrap();
    let expected = profile.to_save(&slot.schema, &slot.extras).unwrap().write();
    let before = written_by_this_thread();
    slot.write(&profile, "the test's save");
    let on_this_thread = written_by_this_thread() - before;
    slot.describe();
    assert_eq!(on_this_thread, 0, "the frame's thread wrote {on_this_thread} bytes of the save itself");
    assert_eq!(std::fs::read(&path).unwrap(), expected);
    drop(slot);
    std::fs::remove_dir_all(&dir).unwrap();
}
