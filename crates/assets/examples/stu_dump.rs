//! Throwaway: print STU assets by name (`012B.116`) as a tree.
use orpheus_assets::{Install, guid_name, guid_type};
use orpheus_stu::{Asset, Object, Types, Value};

fn main() {
    let install = Install::open(std::path::Path::new("D:/Work/Research/Overwatch/game"), "enUS").unwrap();
    for name in std::env::args().skip(1) {
        let kind = u16::from_str_radix(name.rsplit('.').next().unwrap(), 16).unwrap();
        let all = install.guids(kind);
        let index = u64::from_str_radix(name.split('.').next().unwrap(), 16).unwrap();
        let Some(guid) = all.iter().copied().find(|g| guid_name(*g) == name.to_uppercase() || (*g & 0xFFFF_FFFF) == index) else {
            println!("== {name}: not found among {} of type {kind:03X}; first: {:?}", all.len(), all.iter().take(5).map(|g| guid_name(*g)).collect::<Vec<_>>());
            continue;
        };
        println!("== {name} ({guid:#018X})");
        match install.stu(guid) {
            Ok(asset) => {
                for (i, inst) in asset.instances.iter().enumerate() {
                    match inst {
                        Some(obj) => {
                            println!("[{i}] {}", class(obj.class));
                            object(&asset, obj, 1);
                        }
                        None => println!("[{i}] <unknown class>"),
                    }
                }
            }
            Err(e) => {
                println!("decode error: {e}");
                let raw = install.raw(guid).unwrap();
                println!("raw {} bytes, type {:03X}", raw.len(), guid_type(guid));
            }
        }
    }
}

fn class(hash: u32) -> String {
    Types::get().class(hash).map_or(format!("STU_{hash:08X}"), |c| c.name.to_owned())
}

fn object(asset: &Asset, obj: &Object, depth: usize) {
    for (hash, value) in &obj.fields {
        let name = Types::get().field(obj.class, *hash).map_or(format!("m_{hash:08X}"), |f| f.name.to_owned());
        print!("{}{name} = ", "  ".repeat(depth));
        print_value(asset, value, depth);
    }
}

fn print_value(asset: &Asset, value: &Value, depth: usize) {
    match value {
        Value::Object(o) => {
            println!("{{{}}}", class(o.class));
            object(asset, o, depth + 1);
        }
        Value::Array(items) => {
            println!("[{}]", items.len());
            for (i, item) in items.iter().enumerate() {
                print!("{}#{i}: ", "  ".repeat(depth + 1));
                print_value(asset, item, depth + 1);
            }
        }
        Value::Guid(g) if *g != 0 => println!("{} ({g:#018X})", guid_name(*g)),
        Value::Instance(i) => println!("-> [{i}]"),
        other => println!("{other:?}"),
    }
}
