//! The asset reader against the real build 104319 install:
//! `just test-game <install>`, and against a DataTool XML export of the same
//! install: `just test-datatool <install> <dump>`.

use std::fs;

use lucy_casc::{Game, guid_name};
use lucy_stu::{Asset, Object, Value, read};

const GRAPH: u16 = 0x01B;
const DEFINITION: u16 = 0x003;
const MODE: u16 = 0x0C5;
const DRAGON: &str = "https://yretenai.com/dragonml/v1";

fn game() -> Game {
    Game::open(&lucy_testkit::game_dir(), "enUS").unwrap()
}

fn stored(game: &Game, kind: u16) -> Vec<u64> {
    game.guids(kind)
        .into_iter()
        .filter(|guid| game.is_stored(*guid))
        .collect()
}

#[test]
#[ignore = "reads an Overwatch 1.74 install in LUCY_GAME_DIR"]
fn every_stored_graph_definition_and_mode_decodes() {
    let game = game();
    // Instances of classes OWLib doesn't know: DataTool exports them as null.
    // With TACTLib's static 1.74 key every graph and definition is stored.
    for (kind, count, unknown) in [(GRAPH, 1587, 152), (DEFINITION, 3528, 0), (MODE, 25, 0)] {
        let guids = stored(&game, kind);
        assert_eq!(guids.len(), count);
        let mut nulls = 0;
        for (guid, bytes) in game.read_all(&guids) {
            let asset = read(&bytes.unwrap())
                .unwrap_or_else(|error| panic!("{}: {error}", guid_name(guid)));
            assert!(asset.root().is_some(), "{} has no root", guid_name(guid));
            nulls += asset.instances.iter().filter(|item| item.is_none()).count();
        }
        assert_eq!(nulls, unknown, "{kind:03X}");
    }
}

#[test]
#[ignore = "compares the install in LUCY_GAME_DIR with the DataTool export in LUCY_DATATOOL_DUMP"]
fn datatool_graph_items_and_definition_roots_match() {
    let game = game();
    let dump = lucy_testkit::datatool_dump();
    let mut compared = 0;
    for (guid, bytes) in game.read_all(&stored(&game, GRAPH)) {
        let asset = read(&bytes.unwrap()).unwrap();
        let path = dump.join("01B").join(format!("{}.xml", guid_name(guid)));
        let xml = fs::read_to_string(&path).unwrap();
        let document = roxmltree::Document::parse(&xml).unwrap();
        let name = guid_name(guid);
        assert_eq!(graph_items(&asset), xml_graph_items(&document), "{name}");
        compared += compare_scalars(
            &asset,
            asset.root().unwrap(),
            document.root_element(),
            &name,
        );
        for (object, element) in item_pairs(&asset, &document) {
            compared += compare_scalars(&asset, object, element, &name);
        }
    }
    for (guid, bytes) in game.read_all(&stored(&game, DEFINITION)) {
        let asset = read(&bytes.unwrap()).unwrap();
        let path = dump.join("003").join(format!("{}.xml", guid_name(guid)));
        let xml = fs::read_to_string(&path).unwrap();
        let document = roxmltree::Document::parse(&xml).unwrap();
        assert_eq!(
            asset.root().and_then(Object::class_name),
            Some(document.root_element().tag_name().name()),
            "{}",
            guid_name(guid)
        );
        compared += compare_scalars(
            &asset,
            asset.root().unwrap(),
            document.root_element(),
            &guid_name(guid),
        );
    }
    // Every number, null and string DataTool printed on those objects.
    assert!(compared > 1_000_000, "only {compared} values compared");
}

/// Compare the scalar attributes DataTool printed for an object with the
/// decoded fields; the number compared. Enum members DataTool prints by name
/// are skipped.
fn compare_scalars(
    decoded: &Asset,
    object: &Object,
    element: roxmltree::Node<'_, '_>,
    asset: &str,
) -> usize {
    let types = lucy_stu::Types::get();
    let mut compared = 0;
    for attribute in element
        .attributes()
        .filter(|attribute| attribute.namespace().is_none())
    {
        let name = attribute.name();
        let text = attribute.value();
        let field = types
            .field_named(object.class, name)
            .unwrap_or_else(|| panic!("{asset}: no field {name} in {:?}", object.class_name()));
        let value = object
            .fields
            .iter()
            .find(|(hash, _)| *hash == field.hash)
            .map(|(_, value)| value)
            .unwrap_or_else(|| panic!("{asset}: {name} missing"));
        let matches = match (text, value) {
            ("{null}", Value::Null) => true,
            // DataTool prints an instance of a class OWLib doesn't know as null.
            ("{null}", Value::Instance(_)) => decoded.resolve(value).is_none(),
            ("{null}", _) | (_, Value::Null) => false,
            (text, Value::Int(value)) => match text.parse::<i64>() {
                Ok(parsed) => parsed == *value,
                Err(_) => continue, // an enum member by name
            },
            (text, Value::UInt(value)) => match text.parse::<u64>() {
                Ok(parsed) => parsed == *value,
                Err(_) => continue,
            },
            (text, Value::Float(value)) => {
                let text = text.replace(',', ".");
                match field.ty.kind {
                    lucy_stu::Kind::F32 => text.parse::<f32>().ok() == Some(*value as f32),
                    _ => text.parse::<f64>().ok() == Some(*value),
                }
            }
            (text, Value::String(value)) => text == value,
            _ => continue,
        };
        assert!(
            matches,
            "{asset}: {:?}.{name} is {value:?}, DataTool printed {text:?}",
            object.class_name()
        );
        compared += 1;
    }
    compared
}

/// Each known graph item with the element DataTool printed for it.
fn item_pairs<'a, 'input>(
    asset: &'a Asset,
    document: &'a roxmltree::Document<'input>,
) -> Vec<(&'a Object, roxmltree::Node<'a, 'input>)> {
    let root = asset.root().unwrap();
    let Some(graph) = root.get("m_graph").and_then(|value| asset.resolve(value)) else {
        return Vec::new();
    };
    let Some(Value::Array(items)) = graph.get("m_items") else {
        return Vec::new();
    };
    let Some(elements) = named(document.root_element(), "m_graph")
        .and_then(|node| resolve(document, node))
        .and_then(|graph| named(graph, "m_items"))
        .and_then(|node| resolve(document, node))
    else {
        return Vec::new();
    };
    items
        .iter()
        .zip(elements.children().filter(roxmltree::Node::is_element))
        .filter_map(|(item, element)| Some((asset.resolve(item)?, resolve(document, element)?)))
        .collect()
}

/// Graph `01CA`'s first item, as DataTool exports it: a server-only message
/// entry (unique ID 6) for game message `00000000003E.025`.
#[test]
#[ignore = "reads an Overwatch 1.74 install in LUCY_GAME_DIR"]
fn a_graph_item_reads_its_scalars_and_asset_references() {
    let game = game();
    let guid = stored(&game, GRAPH)
        .into_iter()
        .find(|guid| guid & 0xFFFF_FFFF_FFFF == 0x01CA)
        .unwrap();
    let asset = read(&game.read(guid).unwrap()).unwrap();
    let root = asset.root().unwrap();
    assert_eq!(root.class_name(), Some("STUStatescriptGraph"));
    assert_eq!(int(root.get("m_nodesBitCount")), Some(8));
    assert_eq!(int(root.get("m_BE8E52CD")), Some(1_845_772_726));
    let graph = asset.resolve(root.get("m_graph").unwrap()).unwrap();
    let Some(Value::Array(items)) = graph.get("m_items") else {
        panic!("graph without items");
    };
    let entry = asset.resolve(&items[0]).unwrap();
    assert_eq!(entry.class_name(), Some("STUStatescriptEntryGameMessage"));
    assert_eq!(int(entry.get("m_serverOnly")), Some(1));
    assert_eq!(int(entry.get("m_uniqueID")), Some(6));
    let message = asset.resolve(entry.get("m_gameMessage").unwrap()).unwrap();
    let Some(Value::Guid(message)) = message.get("m_gameMessage") else {
        panic!("message without a GUID");
    };
    assert_eq!(guid_name(*message), "00000000003E.025");
}

fn int(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Int(value) => Some(*value),
        Value::UInt(value) => i64::try_from(*value).ok(),
        _ => None,
    }
}

/// Each item of the graph's `m_items`: its class and unique ID, `None` for an
/// item of an unknown class.
fn graph_items(asset: &Asset) -> Vec<Option<(String, String)>> {
    let root = asset.root().unwrap();
    let Some(graph) = root.get("m_graph").and_then(|value| asset.resolve(value)) else {
        return Vec::new();
    };
    let Some(Value::Array(items)) = graph.get("m_items") else {
        return Vec::new();
    };
    items
        .iter()
        .map(|item| {
            let object = asset.resolve(item)?;
            let unique = match object.get("m_uniqueID") {
                Some(Value::UInt(value)) => value.to_string(),
                Some(Value::Int(value)) => value.to_string(),
                _ => "{null}".into(),
            };
            Some((object.class_name()?.to_owned(), unique))
        })
        .collect()
}

fn xml_graph_items(document: &roxmltree::Document<'_>) -> Vec<Option<(String, String)>> {
    let root = document.root_element();
    let Some(graph) = named(root, "m_graph").and_then(|node| resolve(document, node)) else {
        return Vec::new();
    };
    let Some(items) = named(graph, "m_items").and_then(|node| resolve(document, node)) else {
        return Vec::new();
    };
    items
        .children()
        .filter(roxmltree::Node::is_element)
        .map(|item| {
            let item = resolve(document, item)?;
            let class = item.tag_name().name();
            if class == "null" {
                return None;
            }
            let unique = item.attribute("m_uniqueID").unwrap_or("{null}");
            Some((class.to_owned(), unique.to_owned()))
        })
        .collect()
}

/// The child that holds the field `name`.
fn named<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    name: &str,
) -> Option<roxmltree::Node<'a, 'input>> {
    node.children()
        .find(|child| child.attribute((DRAGON, "name")) == Some(name))
}

/// An object reference (`dragon:ref` without a GUID) resolved to the element
/// it repeats.
fn resolve<'a, 'input>(
    document: &'a roxmltree::Document<'input>,
    node: roxmltree::Node<'a, 'input>,
) -> Option<roxmltree::Node<'a, 'input>> {
    if node.tag_name().name() != "ref" || node.attribute("GUID").is_some() {
        return Some(node);
    }
    let id = node.attribute((DRAGON, "id"))?;
    document.descendants().find(|candidate| {
        candidate.attribute((DRAGON, "id")) == Some(id) && candidate.tag_name().name() != "ref"
    })
}
