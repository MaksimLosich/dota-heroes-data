use serde::{Deserialize, Serialize};

// ---- Valve herolist payload: https://www.dota2.com/datafeed/herolist ----

#[derive(Debug, Deserialize)]
pub struct HeroListResponse {
    pub result: HeroListResult,
}

#[derive(Debug, Deserialize)]
pub struct HeroListResult {
    pub data: HeroListData,
}

#[derive(Debug, Deserialize)]
pub struct HeroListData {
    pub heroes: Vec<HeroListEntry>,
}

#[derive(Debug, Deserialize)]
pub struct HeroListEntry {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub name_loc: String,
    #[serde(default)]
    pub name_english_loc: String,
}

// ---- Valve herodata payload: https://www.dota2.com/datafeed/herodata ----

#[derive(Debug, Deserialize)]
pub struct HeroDataResponse {
    pub result: HeroDataResult,
}

#[derive(Debug, Deserialize)]
pub struct HeroDataResult {
    pub data: HeroDataData,
}

#[derive(Debug, Deserialize)]
pub struct HeroDataData {
    pub heroes: Vec<HeroDataEntry>,
}

#[derive(Debug, Deserialize)]
pub struct HeroDataEntry {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub name_loc: String,
    pub primary_attr: i32,
    pub complexity: i32,
    pub attack_capability: i32,
    pub role_levels: Vec<i32>,
    pub abilities: Vec<AbilityEntry>,
}

#[derive(Debug, Deserialize)]
pub struct AbilityEntry {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub name_loc: String,
    #[serde(default)]
    pub is_item: bool,
}

// ---- Output written to heroes.json ----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HeroRecord {
    pub id: u32,
    pub name: String,
    pub internal_name: String,
    pub attribute: String,
    pub attack_type: String,
    pub roles: Vec<String>,
    pub complexity: i32,
    pub power_spike: Option<String>,
    pub signature: Option<String>,
    pub abilities: Vec<AbilityRecord>,
    pub hint_ability_id: Option<u32>,
    pub build_hint: Option<Vec<u32>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AbilityRecord {
    pub id: u32,
    pub name: String,
    pub internal_name: String,
}

// ---- Manual DotaWho fields from overrides.json ----

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OverrideEntry {
    #[serde(default)]
    pub power_spike: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub hint_ability_id: Option<u32>,
    #[serde(default)]
    pub build_hint: Option<Vec<u32>>,
}
