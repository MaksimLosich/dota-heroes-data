use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

mod models;

use models::{
    AbilityEntry, AbilityRecord, HeroDataEntry, HeroDataResponse, HeroListEntry, HeroListResponse,
    HeroRecord, OverrideEntry,
};

const HERO_LIST_URL: &str = "https://www.dota2.com/datafeed/herolist?language=english";
const HERO_DATA_URL: &str = "https://www.dota2.com/datafeed/herodata?language=english&hero_id=";
const MAX_CONCURRENCY: usize = 8;
const MAX_ATTEMPTS: usize = 3;
const RETRY_STATUSES: [u16; 5] = [429, 500, 502, 503, 504];

type Overrides = BTreeMap<String, OverrideEntry>;

// ---- Field mapping ----

fn attribute_from(primary_attr: i32) -> Result<&'static str> {
    match primary_attr {
        0 => Ok("strength"),
        1 => Ok("agility"),
        2 => Ok("intelligence"),
        3 => Ok("universal"),
        other => Err(anyhow!("unknown primary_attr: {other}")),
    }
}

fn attack_type_from(attack_capability: i32) -> Result<&'static str> {
    match attack_capability {
        1 => Ok("melee"),
        2 => Ok("ranged"),
        other => Err(anyhow!("unknown attack_capability: {other}")),
    }
}

fn roles_from(role_levels: &[i32]) -> Result<Vec<String>> {
    const ROLE_NAMES: [&str; 9] = [
        "carry",
        "support",
        "nuker",
        "disabler",
        "jungler",
        "durable",
        "escape",
        "pusher",
        "initiator",
    ];

    if role_levels.len() > ROLE_NAMES.len() {
        bail!(
            "role_levels has {} entries, expected at most 9",
            role_levels.len()
        );
    }

    let mut roles = Vec::new();
    for (index, level) in role_levels.iter().enumerate() {
        if *level > 0 {
            roles.push(ROLE_NAMES[index].to_string());
        }
    }
    Ok(roles)
}

fn is_talent(name: &str) -> bool {
    name.starts_with("special_bonus_")
}

fn abilities_from(abilities: &[AbilityEntry]) -> Vec<AbilityRecord> {
    abilities
        .iter()
        .filter(|ability| !ability.is_item)
        .filter(|ability| !is_talent(&ability.name))
        .filter(|ability| !ability.name_loc.trim().is_empty())
        .map(|ability| AbilityRecord {
            id: ability.id,
            name: ability.name_loc.trim().to_string(),
            internal_name: ability.name.clone(),
        })
        .collect()
}

fn display_name(list: &HeroListEntry, data: &HeroDataEntry) -> String {
    if !list.name_english_loc.trim().is_empty() {
        return list.name_english_loc.trim().to_string();
    }
    if !list.name_loc.trim().is_empty() {
        return list.name_loc.trim().to_string();
    }
    data.name_loc.trim().to_string()
}

fn normalize(list: &HeroListEntry, data: &HeroDataEntry) -> Result<HeroRecord> {
    if data.id != list.id {
        bail!(
            "hero id mismatch: list says {} but herodata says {}",
            list.id,
            data.id
        );
    }
    if data.name != list.name {
        bail!(
            "hero {} internal name mismatch: list says {} but herodata says {}",
            list.id,
            list.name,
            data.name
        );
    }

    let name = display_name(list, data);
    if name.is_empty() {
        bail!("hero {} has an empty name", list.id);
    }

    Ok(HeroRecord {
        id: list.id,
        name,
        internal_name: list.name.clone(),
        attribute: attribute_from(data.primary_attr)?.to_string(),
        attack_type: attack_type_from(data.attack_capability)?.to_string(),
        roles: roles_from(&data.role_levels)?,
        complexity: data.complexity,
        power_spike: None,
        signature: None,
        abilities: abilities_from(&data.abilities),
        hint_ability_id: None,
        build_hint: None,
    })
}

// ---- Overrides ----

fn apply_override(hero: &mut HeroRecord, entry: &OverrideEntry) {
    if let Some(power_spike) = entry.power_spike.clone() {
        hero.power_spike = Some(power_spike);
    }
    if let Some(signature) = entry.signature.clone() {
        hero.signature = Some(signature);
    }
    if entry.hint_ability_id.is_some() {
        hero.hint_ability_id = entry.hint_ability_id;
    }
    if entry.build_hint.is_some() {
        hero.build_hint = entry.build_hint.clone();
    }
}

fn load_overrides(path: &Path) -> Result<Overrides> {
    if !path.exists() {
        return Ok(Overrides::new());
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(Overrides::new());
    }
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

// ---- HTTP ----

fn is_retryable_status(status: u16) -> bool {
    RETRY_STATUSES.contains(&status)
}

async fn get_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> Result<T> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let result = client.get(url).send().await;
        match result {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    return response
                        .json::<T>()
                        .await
                        .with_context(|| format!("decoding JSON from {url}"));
                }
                if is_retryable_status(status.as_u16()) && attempt < MAX_ATTEMPTS {
                    tokio::time::sleep(Duration::from_millis(400 * attempt as u64)).await;
                    continue;
                }
                bail!("HTTP {} from {url}", status.as_u16());
            }
            Err(error) => {
                if attempt < MAX_ATTEMPTS {
                    tokio::time::sleep(Duration::from_millis(400 * attempt as u64)).await;
                    continue;
                }
                return Err(error).with_context(|| format!("requesting {url}"));
            }
        }
    }
}

async fn fetch_hero_list(client: &reqwest::Client) -> Result<Vec<HeroListEntry>> {
    let payload: HeroListResponse = get_json(client, HERO_LIST_URL).await?;
    Ok(payload.result.data.heroes)
}

async fn fetch_hero_data(client: &reqwest::Client, hero_id: u32) -> Result<HeroDataEntry> {
    let url = format!("{HERO_DATA_URL}{hero_id}");
    let payload: HeroDataResponse = get_json(client, &url).await?;
    payload
        .result
        .data
        .heroes
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no herodata returned for hero {hero_id}"))
}

// ---- Validation ----

fn validate(heroes: &[HeroRecord], overrides: &Overrides) -> Result<()> {
    if heroes.is_empty() {
        bail!("no heroes generated");
    }

    let mut ids = BTreeSet::new();
    let mut internal_names = BTreeSet::new();

    for hero in heroes {
        if !ids.insert(hero.id) {
            bail!("duplicate hero id {}", hero.id);
        }
        if !internal_names.insert(hero.internal_name.clone()) {
            bail!("duplicate internal name {}", hero.internal_name);
        }
        if hero.name.trim().is_empty() {
            bail!("hero {} has an empty name", hero.id);
        }
        if attribute_from(hero_attribute_code(&hero.attribute))? != hero.attribute {
            bail!(
                "hero {} has an invalid attribute {}",
                hero.id,
                hero.attribute
            );
        }
        if attack_type_from(attack_type_code(&hero.attack_type))? != hero.attack_type {
            bail!(
                "hero {} has an invalid attackType {}",
                hero.id,
                hero.attack_type
            );
        }
        if !(1..=3).contains(&hero.complexity) {
            bail!(
                "hero {} has an invalid complexity {}",
                hero.id,
                hero.complexity
            );
        }
        for role in &hero.roles {
            if !ROLES_VALID.contains(&role.as_str()) {
                bail!("hero {} has an invalid role {}", hero.id, role);
            }
        }
        if let Some(power_spike) = &hero.power_spike {
            if !matches!(power_spike.as_str(), "early" | "mid" | "late") {
                bail!("hero {} has an invalid powerSpike {}", hero.id, power_spike);
            }
        }

        let mut ability_ids = BTreeSet::new();
        for ability in &hero.abilities {
            if !ability_ids.insert(ability.id) {
                bail!("hero {} has duplicate ability id {}", hero.id, ability.id);
            }
        }
    }

    let mut sorted = heroes.to_vec();
    sorted.sort_by_key(|hero| hero.id);
    if sorted != heroes {
        bail!("heroes are not sorted by id ascending");
    }

    for (key, entry) in overrides {
        let hero_id: u32 = key
            .parse()
            .with_context(|| format!("override key {key:?} is not a hero id"))?;
        if key != &hero_id.to_string() {
            bail!("override key {key:?} is not canonical; use {hero_id}");
        }
        let hero = heroes
            .iter()
            .find(|hero| hero.id == hero_id)
            .ok_or_else(|| anyhow!("override references unknown hero id {hero_id}"))?;

        if let Some(power_spike) = &entry.power_spike {
            if !matches!(power_spike.as_str(), "early" | "mid" | "late") {
                bail!("override hero {hero_id} has an invalid powerSpike {power_spike}");
            }
        }
        if let Some(hint) = entry.hint_ability_id {
            if !hero.abilities.iter().any(|ability| ability.id == hint) {
                bail!("override hero {hero_id} hintAbilityId {hint} does not belong to this hero");
            }
        }
    }

    Ok(())
}

const ROLES_VALID: [&str; 9] = [
    "carry",
    "support",
    "nuker",
    "disabler",
    "jungler",
    "durable",
    "escape",
    "pusher",
    "initiator",
];

fn hero_attribute_code(attribute: &str) -> i32 {
    match attribute {
        "strength" => 0,
        "agility" => 1,
        "intelligence" => 2,
        "universal" => 3,
        _ => i32::MIN,
    }
}

fn attack_type_code(attack_type: &str) -> i32 {
    match attack_type {
        "melee" => 1,
        "ranged" => 2,
        _ => i32::MIN,
    }
}

// ---- Images ----

struct ImageReport {
    missing: Vec<(u32, String)>,
    orphans: Vec<String>,
}

fn check_images(heroes: &[HeroRecord], dir: &Path) -> Result<ImageReport> {
    let hero_ids: BTreeSet<u32> = heroes.iter().map(|hero| hero.id).collect();
    let mut missing = Vec::new();
    let mut orphans = Vec::new();

    for hero in heroes {
        if !dir.join(format!("{}.png", hero.id)).is_file() {
            missing.push((hero.id, hero.name.clone()));
        }
    }

    if dir.is_dir() {
        for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
            let path = entry?.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("png") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            // Only numeric file names can name a hero image; anything else is not
            // an orphan hero id.
            let Ok(id) = stem.parse::<u32>() else {
                continue;
            };
            if !hero_ids.contains(&id) {
                orphans.push(path.file_name().unwrap().to_string_lossy().to_string());
            }
        }
    }

    orphans.sort();
    Ok(ImageReport { missing, orphans })
}

// ---- Output ----

fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, contents).with_context(|| format!("writing {}", tmp.display()))?;
    // std::fs::rename replaces an existing destination on both Windows
    // (MoveFileExW with MOVEFILE_REPLACE_EXISTING) and Unix, so a single call is
    // atomic and no destination removal is ever needed.
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("heroes_data/0.1 (DotaWho hero generator)")
        .build()
        .context("building HTTP client")
}

async fn run() -> Result<()> {
    let client = client()?;
    let heroes_path = PathBuf::from("heroes.json");
    let overrides_path = PathBuf::from("overrides.json");
    let images_dir = PathBuf::from("heroes");

    let list = fetch_hero_list(&client)
        .await
        .context("fetching hero list")?;
    println!("Valve returned {} heroes", list.len());

    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENCY));
    let mut tasks = JoinSet::new();
    for entry in &list {
        let client = client.clone();
        let semaphore = semaphore.clone();
        let id = entry.id;
        tasks.spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .expect("semaphore is never closed");
            (id, fetch_hero_data(&client, id).await)
        });
    }

    let mut details: BTreeMap<u32, HeroDataEntry> = BTreeMap::new();
    let mut failures = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        let (id, result) = joined.context("hero fetch task panicked")?;
        match result {
            Ok(entry) => {
                details.insert(id, entry);
            }
            Err(error) => failures.push((id, error)),
        }
    }

    if !failures.is_empty() {
        for (id, error) in &failures {
            eprintln!("Failed to fetch hero {id}: {error:#}");
        }
        bail!(
            "{} hero fetches failed; not writing heroes.json",
            failures.len()
        );
    }

    let mut generated = Vec::with_capacity(list.len());
    for entry in &list {
        let detail = details
            .get(&entry.id)
            .ok_or_else(|| anyhow!("missing herodata for hero {}", entry.id))?;
        generated.push(normalize(entry, detail)?);
    }
    generated.sort_by_key(|hero| hero.id);

    let overrides = load_overrides(&overrides_path)?;
    for hero in &mut generated {
        if let Some(entry) = overrides.get(&hero.id.to_string()) {
            apply_override(hero, entry);
        }
    }

    validate(&generated, &overrides)?;
    let images = check_images(&generated, &images_dir)?;

    let json = serde_json::to_string_pretty(&generated).context("serializing heroes.json")?;
    write_atomic(&heroes_path, &format!("{json}\n"))?;

    println!(
        "Generated {} heroes into {}",
        generated.len(),
        heroes_path.display()
    );
    if images.missing.is_empty() {
        println!("All hero images present in {}", images_dir.display());
    } else {
        println!("Missing {} hero images:", images.missing.len());
        for (id, name) in &images.missing {
            println!("Missing image:");
            println!("{id} - {name}");
            println!("expected: heroes/{id}.png");
        }
    }
    if images.orphans.is_empty() {
        println!("No orphan images in {}", images_dir.display());
    } else {
        println!("Orphan images (no matching current hero):");
        for name in &images.orphans {
            println!("  heroes/{name}");
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

// ---- Tests ----

#[cfg(test)]
mod tests {
    use super::*;

    fn list_entry(id: u32, name: &str, english: &str) -> HeroListEntry {
        HeroListEntry {
            id,
            name: name.to_string(),
            name_loc: english.to_string(),
            name_english_loc: english.to_string(),
        }
    }

    fn data_entry(id: u32, name: &str, attr: i32, attack: i32, roles: Vec<i32>) -> HeroDataEntry {
        HeroDataEntry {
            id,
            name: name.to_string(),
            name_loc: name.to_string(),
            primary_attr: attr,
            complexity: 1,
            attack_capability: attack,
            role_levels: roles,
            abilities: vec![],
        }
    }

    #[test]
    fn maps_attributes() {
        assert_eq!(attribute_from(0).unwrap(), "strength");
        assert_eq!(attribute_from(1).unwrap(), "agility");
        assert_eq!(attribute_from(2).unwrap(), "intelligence");
        assert_eq!(attribute_from(3).unwrap(), "universal");
    }

    #[test]
    fn rejects_unknown_attribute() {
        assert!(attribute_from(4).is_err());
        assert!(attribute_from(-1).is_err());
    }

    #[test]
    fn maps_attack_types() {
        assert_eq!(attack_type_from(1).unwrap(), "melee");
        assert_eq!(attack_type_from(2).unwrap(), "ranged");
    }

    #[test]
    fn rejects_unknown_attack_type() {
        assert!(attack_type_from(0).is_err());
        assert!(attack_type_from(3).is_err());
    }

    #[test]
    fn normalizes_roles_in_valve_order() {
        let roles = roles_from(&[3, 0, 1, 0, 0, 0, 3, 0, 0]).unwrap();
        assert_eq!(roles, vec!["carry", "nuker", "escape"]);
    }

    #[test]
    fn drops_roles_with_zero_level() {
        let roles = roles_from(&[0, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        assert!(roles.is_empty());
    }

    #[test]
    fn filters_talents_items_and_empty_names() {
        let abilities = vec![
            AbilityEntry {
                id: 1,
                name: "hero_spell".to_string(),
                name_loc: "Spell".to_string(),
                is_item: false,
            },
            AbilityEntry {
                id: 2,
                name: "special_bonus_attack_speed".to_string(),
                name_loc: "+20 Attack Speed".to_string(),
                is_item: false,
            },
            AbilityEntry {
                id: 3,
                name: "item_ability".to_string(),
                name_loc: "Item".to_string(),
                is_item: true,
            },
            AbilityEntry {
                id: 4,
                name: "hidden".to_string(),
                name_loc: "  ".to_string(),
                is_item: false,
            },
        ];
        let records = abilities_from(&abilities);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, 1);
        assert_eq!(records[0].name, "Spell");
        assert_eq!(records[0].internal_name, "hero_spell");
    }

    #[test]
    fn prefers_english_name_then_falls_back() {
        let data = data_entry(45, "npc_dota_hero_pugna", 2, 2, vec![]);
        let english = list_entry(45, "npc_dota_hero_pugna", "Pugna");
        assert_eq!(display_name(&english, &data), "Pugna");

        let mut localized_only = list_entry(45, "npc_dota_hero_pugna", "");
        localized_only.name_loc = "Pugna RU".to_string();
        assert_eq!(display_name(&localized_only, &data), "Pugna RU");
    }

    #[test]
    fn normalizes_a_hero() {
        let list = list_entry(1, "npc_dota_hero_antimage", "Anti-Mage");
        let mut data = data_entry(
            1,
            "npc_dota_hero_antimage",
            1,
            1,
            vec![3, 0, 1, 0, 0, 0, 3, 0, 0],
        );
        data.complexity = 1;
        data.abilities = vec![AbilityEntry {
            id: 5003,
            name: "antimage_mana_break".to_string(),
            name_loc: "Mana Break".to_string(),
            is_item: false,
        }];
        let hero = normalize(&list, &data).unwrap();
        assert_eq!(hero.id, 1);
        assert_eq!(hero.name, "Anti-Mage");
        assert_eq!(hero.internal_name, "npc_dota_hero_antimage");
        assert_eq!(hero.attribute, "agility");
        assert_eq!(hero.attack_type, "melee");
        assert_eq!(hero.roles, vec!["carry", "nuker", "escape"]);
        assert_eq!(hero.power_spike, None);
        assert_eq!(hero.abilities.len(), 1);
        assert_eq!(hero.abilities[0].id, 5003);
    }

    #[test]
    fn merges_overrides_without_destroying_defaults() {
        let list = list_entry(45, "npc_dota_hero_pugna", "Pugna");
        let mut data = data_entry(
            45,
            "npc_dota_hero_pugna",
            2,
            2,
            vec![0, 0, 2, 0, 0, 0, 0, 2, 0],
        );
        data.abilities = vec![AbilityEntry {
            id: 5186,
            name: "pugna_nether_blast".to_string(),
            name_loc: "Nether Blast".to_string(),
            is_item: false,
        }];
        let mut hero = normalize(&list, &data).unwrap();

        let entry = OverrideEntry {
            power_spike: Some("mid".to_string()),
            signature: Some("channeling".to_string()),
            hint_ability_id: Some(5186),
            build_hint: None,
        };
        apply_override(&mut hero, &entry);

        assert_eq!(hero.power_spike.as_deref(), Some("mid"));
        assert_eq!(hero.signature.as_deref(), Some("channeling"));
        assert_eq!(hero.hint_ability_id, Some(5186));
        assert_eq!(hero.build_hint, None);
        assert_eq!(hero.name, "Pugna");
    }

    #[test]
    fn override_validation_rejects_unknown_hint_ability() {
        let list = list_entry(45, "npc_dota_hero_pugna", "Pugna");
        let data = data_entry(
            45,
            "npc_dota_hero_pugna",
            2,
            2,
            vec![0, 0, 2, 0, 0, 0, 0, 2, 0],
        );
        let hero = normalize(&list, &data).unwrap();
        let mut overrides: Overrides = BTreeMap::new();
        overrides.insert(
            "45".to_string(),
            OverrideEntry {
                hint_ability_id: Some(9999),
                ..OverrideEntry::default()
            },
        );
        assert!(validate(&[hero], &overrides).is_err());
    }

    #[test]
    fn override_validation_rejects_unknown_hero() {
        let list = list_entry(45, "npc_dota_hero_pugna", "Pugna");
        let data = data_entry(45, "npc_dota_hero_pugna", 2, 2, vec![]);
        let hero = normalize(&list, &data).unwrap();
        let mut overrides: Overrides = BTreeMap::new();
        overrides.insert("999".to_string(), OverrideEntry::default());
        assert!(validate(&[hero], &overrides).is_err());
    }

    #[test]
    fn parses_overrides_json() {
        let parsed: Overrides =
            serde_json::from_str(r#"{"45":{"powerSpike":"mid","signature":"channeling"}}"#)
                .unwrap();
        let entry = parsed.get("45").unwrap();
        assert_eq!(entry.power_spike.as_deref(), Some("mid"));
        assert_eq!(entry.signature.as_deref(), Some("channeling"));
        assert_eq!(entry.hint_ability_id, None);
    }

    #[test]
    fn serializes_output_with_camel_case() {
        let hero = HeroRecord {
            id: 1,
            name: "Anti-Mage".to_string(),
            internal_name: "npc_dota_hero_antimage".to_string(),
            attribute: "agility".to_string(),
            attack_type: "melee".to_string(),
            roles: vec!["carry".to_string()],
            complexity: 1,
            power_spike: None,
            signature: None,
            abilities: vec![AbilityRecord {
                id: 5003,
                name: "Mana Break".to_string(),
                internal_name: "antimage_mana_break".to_string(),
            }],
            hint_ability_id: None,
            build_hint: None,
        };
        let json = serde_json::to_string(&hero).unwrap();
        assert!(json.contains("\"internalName\":\"npc_dota_hero_antimage\""));
        assert!(json.contains("\"attackType\":\"melee\""));
        assert!(json.contains("\"powerSpike\":null"));
        assert!(json.contains("\"hintAbilityId\":null"));
        assert!(json.contains("\"buildHint\":null"));
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("heroes_data_test_{}_{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn hero_record(id: u32, name: &str) -> HeroRecord {
        HeroRecord {
            id,
            name: name.to_string(),
            internal_name: format!("npc_dota_hero_{id}"),
            attribute: "agility".to_string(),
            attack_type: "melee".to_string(),
            roles: vec!["carry".to_string()],
            complexity: 1,
            power_spike: None,
            signature: None,
            abilities: vec![],
            hint_ability_id: None,
            build_hint: None,
        }
    }

    #[test]
    fn write_atomic_overwrites_existing_file() {
        let dir = temp_dir("overwrite");
        let path = dir.join("heroes.json");
        std::fs::write(&path, "old").unwrap();

        write_atomic(&path, "new").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert!(!dir.join("heroes.json.tmp").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_atomic_preserves_destination_on_failure() {
        let dir = temp_dir("failure");
        let path = dir.join("heroes.json");
        // A non-empty directory at the destination makes the rename fail.
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("keep.txt"), "keep").unwrap();

        let result = write_atomic(&path, "new");

        assert!(result.is_err());
        assert!(path.is_dir());
        assert_eq!(
            std::fs::read_to_string(path.join("keep.txt")).unwrap(),
            "keep"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn check_images_uses_exact_paths_and_numeric_orphans_only() {
        let dir = temp_dir("images");
        // A directory named 1.png must not count as hero 1's image.
        std::fs::create_dir_all(dir.join("1.png")).unwrap();
        // 001.png parses to a current hero id, so it is not an orphan.
        std::fs::write(dir.join("001.png"), b"x").unwrap();
        std::fs::write(dir.join("2.png"), b"x").unwrap();
        // Numeric name for a hero that does not exist.
        std::fs::write(dir.join("9999.png"), b"x").unwrap();
        // Non-numeric file names are never orphan hero ids.
        std::fs::write(dir.join("notnumeric.png"), b"x").unwrap();

        let heroes = vec![hero_record(1, "One"), hero_record(2, "Two")];
        let report = check_images(&heroes, &dir).unwrap();

        assert_eq!(report.missing, vec![(1, "One".to_string())]);
        assert_eq!(report.orphans, vec!["9999.png".to_string()]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn override_validation_rejects_noncanonical_key() {
        let list = list_entry(45, "npc_dota_hero_pugna", "Pugna");
        let data = data_entry(45, "npc_dota_hero_pugna", 2, 2, vec![]);
        let hero = normalize(&list, &data).unwrap();
        let mut overrides: Overrides = BTreeMap::new();
        overrides.insert("045".to_string(), OverrideEntry::default());
        assert!(validate(&[hero], &overrides).is_err());
    }

    #[test]
    fn hero_data_requires_role_levels_and_abilities() {
        let missing_roles = r#"{"result":{"data":{"heroes":[{"id":1,"name":"npc_dota_hero_antimage",
            "primary_attr":1,"complexity":1,"attack_capability":1,"abilities":[]}]}}}"#;
        let missing_abilities = r#"{"result":{"data":{"heroes":[{"id":1,"name":"npc_dota_hero_antimage",
            "primary_attr":1,"complexity":1,"attack_capability":1,"role_levels":[1,0,0,0,0,0,0,0,0]}]}}}"#;

        assert!(serde_json::from_str::<HeroDataResponse>(missing_roles).is_err());
        assert!(serde_json::from_str::<HeroDataResponse>(missing_abilities).is_err());
    }
}
