//! What the dashboard says about each lobby event, in `EVENT_PRESETS` order.
//!
//! The scene statuses and notes are the upstream authors' reports from the
//! Python server, carried over unchanged; see `docs/notes/behavior.md`.

pub(super) struct EventInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub category: &'static str,
    pub scene_status: &'static str,
    pub scene_note: &'static str,
}

const fn info(
    id: &'static str,
    label: &'static str,
    description: &'static str,
    category: &'static str,
    scene_status: &'static str,
    scene_note: &'static str,
) -> EventInfo {
    EventInfo {
        id,
        label,
        description,
        category,
        scene_status,
        scene_note,
    }
}

const WORKED: &str = "Worked in previous checks.";
const SWITCHED: &str = "Scene switching confirmed in game after the update.";

pub(super) const EVENT_INFO: [EventInfo; 15] = [
    info(
        "goodbye",
        "Farewell to Overwatch",
        "General scene with the original Overwatch heroes.",
        "special",
        "verified",
        WORKED,
    ),
    info(
        "lunar",
        "Lunar New Year",
        "Year of the Tiger: festive maps and loot boxes.",
        "seasonal",
        "verified",
        WORKED,
    ),
    info(
        "halloween",
        "Halloween Terror",
        "Festive Eichenwalde and themed loot boxes.",
        "seasonal",
        "verified",
        "Worked in game.",
    ),
    info(
        "winter",
        "Winter Wonderland",
        "Winter scenes and festive loot boxes.",
        "seasonal",
        "verified",
        WORKED,
    ),
    info(
        "anniversary",
        "Anniversary: Remix Vol. 3",
        "Scene with heroes in anniversary skins.",
        "seasonal",
        "verified",
        SWITCHED,
    ),
    info(
        "anniversary_remix_1",
        "Anniversary: Remix Vol. 1",
        "Early version of the event with the same anniversary scene.",
        "seasonal",
        "verified",
        SWITCHED,
    ),
    info(
        "anniversary_remix_2",
        "Anniversary: Remix Vol. 2",
        "Second version of the event with the same anniversary scene.",
        "seasonal",
        "verified",
        SWITCHED,
    ),
    info(
        "summer",
        "Summer Games",
        "The \"Summer Games\" event and themed loot boxes.",
        "seasonal",
        "limited",
        "In this client version the event does not select a separate summer scene.",
    ),
    info(
        "archives",
        "Archives",
        "The \"Archives\" event and themed loot boxes.",
        "seasonal",
        "limited",
        "In this client version the event does not select a separate \"Archives\" scene.",
    ),
    info(
        "reaper",
        "Reaper Challenge",
        "The \"Code of Violence\" challenge scene.",
        "special",
        "verified",
        WORKED,
    ),
    info(
        "cassidy",
        "Cassidy Challenge",
        "The \"New Blood\" challenge scene.",
        "special",
        "verified",
        WORKED,
    ),
    info(
        "malevento",
        "Malevento",
        "Main menu with a Malevento view.",
        "special",
        "verified",
        WORKED,
    ),
    info(
        "owl",
        "Overwatch League: Genji",
        "Overwatch League scene with Genji.",
        "esports",
        "verified",
        WORKED,
    ),
    info(
        "watchpoint",
        "Watchpoint Pack",
        "Overwatch 2 pre-order promo in the What's new box, with the OWL scene.",
        "special",
        "verified",
        "Worked in game. The box's Buy button opens the Battle.net shop page.",
    ),
    info(
        "ow2_credits",
        "Overwatch 2 credits notice",
        "The splash from before Overwatch 2: spend your credits by 2 October.",
        "special",
        "verified",
        "Worked in game. The splash shows once per player, on the main menu.",
    ),
];
