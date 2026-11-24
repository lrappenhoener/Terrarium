//! Fortress content: the embedded base content plus mod directories, resolved to
//! dense indices. Rules in code refer to content only through these indices and a
//! few roles (soil, rock strata, food category, ...) resolved here while loading.

#![allow(clippy::result_large_err)]

use serde::Deserialize;
use sim_core::content::{Content, ContentError, ContentFile, Registry, read_layer};

pub(crate) const KINDS: [&str; 7] = [
    "category", "material", "item", "workshop", "recipe", "plant", "skill",
];

/// The base content, embedded so that a simulation never depends on the working
/// directory. A test checks that it lists every file of `content/base`.
pub(crate) const BASE: [(&str, &str); 7] = [
    (
        "categories.toml",
        include_str!("../content/base/categories.toml"),
    ),
    ("items.toml", include_str!("../content/base/items.toml")),
    (
        "materials.toml",
        include_str!("../content/base/materials.toml"),
    ),
    ("plants.toml", include_str!("../content/base/plants.toml")),
    ("recipes.toml", include_str!("../content/base/recipes.toml")),
    ("skills.toml", include_str!("../content/base/skills.toml")),
    (
        "workshops.toml",
        include_str!("../content/base/workshops.toml"),
    ),
];

pub const SEASONS: [&str; 4] = ["spring", "summer", "autumn", "winter"];
/// Upper bounds that keep per-item and per-recipe work bounded.
const MAX_CATEGORIES: usize = 64;
pub(crate) const MAX_INPUTS: usize = 4;
pub(crate) const MAX_SKILLS: usize = 16;

fn four() -> i32 {
    4
}

fn one() -> u32 {
    1
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CategoryFile {
    #[serde(default)]
    parent: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MaterialFile {
    class: String,
    #[serde(default)]
    dig_ticks: u32,
    #[serde(default)]
    dig_item: Option<String>,
    #[serde(default)]
    stratum: Option<u32>,
    #[serde(default)]
    vein: Option<u32>,
    #[serde(default)]
    aquifer: bool,
    #[serde(default)]
    magma_product: bool,
    #[serde(default = "four")]
    conductivity: i32,
    #[serde(default = "four")]
    heat_capacity: i32,
    #[serde(default)]
    value: u32,
    #[serde(default)]
    flammable: bool,
    #[serde(default)]
    smelts_into: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemFile {
    category: String,
    /// Material of items spawned without a recipe, for example starting rations.
    #[serde(default)]
    material: Option<String>,
    #[serde(default = "one")]
    stack: u32,
    #[serde(default)]
    capacity: u16,
    #[serde(default)]
    bulk: bool,
    #[serde(default)]
    nutrition: u16,
    #[serde(default)]
    cooked: bool,
    #[serde(default)]
    value: u32,
    #[serde(default)]
    durability: u16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AmountFile {
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    item: Option<String>,
    #[serde(default = "one")]
    amount: u32,
    #[serde(default)]
    raw: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkshopFile {
    size: i32,
    build: Vec<AmountFile>,
    build_ticks: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputFile {
    item: String,
    #[serde(default = "one")]
    amount: u32,
    #[serde(default)]
    material: Option<String>,
    #[serde(default)]
    material_of: Option<u8>,
    #[serde(default)]
    smelted: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeFile {
    workshop: String,
    skill: String,
    work_ticks: u32,
    inputs: Vec<AmountFile>,
    #[serde(default)]
    fuel: u32,
    outputs: Vec<OutputFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct YieldFile {
    item: String,
    #[serde(default = "one")]
    amount: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlantFile {
    #[serde(default)]
    tree: bool,
    #[serde(default)]
    underground: bool,
    material: String,
    seasons: Vec<String>,
    growth_ticks: u32,
    #[serde(default)]
    seed: Option<String>,
    skill: String,
    work_ticks: u32,
    harvest: Vec<YieldFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillFile {
    levels: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialClass {
    Soil,
    Rock,
    Ore,
    Metal,
    Wood,
    Plant,
}

#[derive(Debug, Clone)]
pub struct MaterialDef {
    pub class: MaterialClass,
    /// Zero for materials that do not occur as solid tiles.
    pub dig_ticks: u32,
    pub dig_item: Option<u16>,
    pub aquifer: bool,
    pub conductivity: i32,
    pub heat_capacity: i32,
    pub value: u32,
    pub flammable: bool,
    pub smelts_into: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct CategoryDef {
    pub parent: Option<u8>,
    /// Bit `i` is set if category `i` is this category or one of its ancestors.
    pub ancestors: u64,
}

#[derive(Debug, Clone)]
pub struct ItemDef {
    pub category: u8,
    pub material: Option<u16>,
    pub stack: u32,
    pub capacity: u16,
    pub bulk: bool,
    pub nutrition: u16,
    pub cooked: bool,
    pub value: u32,
    pub durability: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ingredient {
    Category(u8),
    Item(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Input {
    pub what: Ingredient,
    pub amount: u32,
    /// Excludes cooked items.
    pub raw: bool,
}

#[derive(Debug, Clone)]
pub struct WorkshopDef {
    pub size: i32,
    pub build: Vec<Input>,
    pub build_ticks: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMaterial {
    Fixed(u16),
    Of(u8),
    Smelted(u8),
}

#[derive(Debug, Clone, Copy)]
pub struct OutputDef {
    pub item: u16,
    pub amount: u32,
    pub material: OutputMaterial,
}

#[derive(Debug, Clone)]
pub struct RecipeDef {
    pub workshop: u16,
    pub skill: u16,
    pub work_ticks: u32,
    /// Ingredients including the fuel, which comes last.
    pub inputs: Vec<Input>,
    pub outputs: Vec<OutputDef>,
}

#[derive(Debug, Clone)]
pub struct PlantDef {
    pub tree: bool,
    pub underground: bool,
    pub material: u16,
    /// Bit `i` set for growth in season `i`.
    pub seasons: u8,
    pub growth_ticks: u32,
    pub seed: Option<u16>,
    pub skill: u16,
    pub work_ticks: u32,
    pub harvest: Vec<(u16, u32)>,
}

#[derive(Debug, Clone)]
pub struct SkillDef {
    pub levels: Vec<u32>,
}

impl SkillDef {
    pub fn level(&self, experience: u32) -> u32 {
        self.levels
            .iter()
            .filter(|&&threshold| threshold <= experience)
            .count()
            .saturating_sub(1) as u32
    }
}

/// String IDs for display, debug dumps and command input only.
#[derive(Debug, Clone, Default)]
pub struct Ids {
    pub categories: Vec<String>,
    pub materials: Vec<String>,
    pub items: Vec<String>,
    pub workshops: Vec<String>,
    pub recipes: Vec<String>,
    pub plants: Vec<String>,
    pub skills: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GameContent {
    pub fingerprint: u64,
    /// Categories of item kinds that can carry other items while hauling.
    pub bulk_categories: u64,
    pub ids: Ids,
    pub categories: Vec<CategoryDef>,
    pub materials: Vec<MaterialDef>,
    pub items: Vec<ItemDef>,
    pub workshops: Vec<WorkshopDef>,
    pub recipes: Vec<RecipeDef>,
    pub plants: Vec<PlantDef>,
    pub skills: Vec<SkillDef>,
    /// Surface material, then rock layers from the top down.
    pub soil: u16,
    pub rocks: Vec<u16>,
    /// Ore vein materials in vein order.
    pub ores: Vec<u16>,
    pub obsidian: u16,
    pub aquifer: u16,
    pub food: u8,
    pub stone: u8,
    pub seed: u8,
    pub fuel: u8,
    pub log: u8,
    pub ration: u16,
    pub ration_material: u16,
}

fn ingredient<T>(
    registry: &Registry<T>,
    index: u16,
    key: &str,
    amount: &AmountFile,
    categories: &Registry<CategoryFile>,
    items: &Registry<ItemFile>,
) -> Result<Input, ContentError> {
    let what = match (&amount.category, &amount.item) {
        (Some(category), None) => {
            Ingredient::Category(registry.resolve(index, key, categories, category)? as u8)
        }
        (None, Some(item)) => Ingredient::Item(registry.resolve(index, key, items, item)?),
        _ => {
            return Err(registry.error(
                index,
                key,
                "genau eines von `category` und `item` angeben",
            ));
        }
    };
    registry.check(
        index,
        &format!("{key}.amount"),
        (1..=1000).contains(&amount.amount),
        "muss zwischen 1 und 1000 liegen",
    )?;
    Ok(Input {
        what,
        amount: amount.amount,
        raw: amount.raw,
    })
}

impl GameContent {
    /// Base content plus the mod directories in order.
    pub fn load(mods: &[String]) -> Result<Self, ContentError> {
        let base = BASE
            .iter()
            .map(|(path, text)| ContentFile {
                layer: "base".into(),
                path: (*path).into(),
                text: (*text).into(),
            })
            .collect();
        let mut layers = vec![base];
        for dir in mods {
            layers.push(read_layer(dir, std::path::Path::new(dir))?);
        }
        Self::from_layers(&layers)
    }

    pub fn from_layers(layers: &[Vec<ContentFile>]) -> Result<Self, ContentError> {
        let content = Content::merge(layers)?;
        content.expect_kinds(&KINDS)?;
        let categories = content.registry::<CategoryFile>("category")?;
        let materials = content.registry::<MaterialFile>("material")?;
        let items = content.registry::<ItemFile>("item")?;
        let workshops = content.registry::<WorkshopFile>("workshop")?;
        let recipes = content.registry::<RecipeFile>("recipe")?;
        let plants = content.registry::<PlantFile>("plant")?;
        let skills = content.registry::<SkillFile>("skill")?;
        let required = |kind: &str, ids: &[String], id: &str| {
            ids.binary_search_by(|probe| probe.as_str().cmp(id))
                .map(|i| i as u16)
                .map_err(|_| ContentError::Entry {
                    origin: sim_core::content::Origin {
                        layer: "base".into(),
                        path: String::new(),
                    },
                    kind: kind.into(),
                    id: id.into(),
                    key: "id".into(),
                    message: "wird von den Spielregeln benötigt und fehlt".into(),
                })
        };

        if categories.len() > MAX_CATEGORIES {
            return Err(ContentError::TooMany {
                kind: "category".into(),
                max: MAX_CATEGORIES,
            });
        }
        let mut category_defs = Vec::with_capacity(categories.len());
        for (i, c) in categories.iter() {
            let parent = match &c.parent {
                Some(parent) => Some(categories.resolve(i, "parent", &categories, parent)? as u8),
                None => None,
            };
            category_defs.push(CategoryDef {
                parent,
                ancestors: 0,
            });
        }
        categories.reject_cycles(|i| {
            category_defs[usize::from(i)]
                .parent
                .map(u16::from)
                .into_iter()
                .collect()
        })?;
        for i in 0..category_defs.len() {
            let mut mask = 0u64;
            let mut current = Some(i as u8);
            while let Some(c) = current {
                mask |= 1 << c;
                current = category_defs[usize::from(c)].parent;
            }
            category_defs[i].ancestors = mask;
        }

        let mut item_defs = Vec::with_capacity(items.len());
        for (i, item) in items.iter() {
            let category = items.resolve(i, "category", &categories, &item.category)? as u8;
            let material = match &item.material {
                Some(m) => Some(items.resolve(i, "material", &materials, m)?),
                None => None,
            };
            items.check(
                i,
                "stack",
                (1..=10_000).contains(&item.stack),
                "muss zwischen 1 und 10000 liegen",
            )?;
            items.check(i, "capacity", item.capacity <= 64, "höchstens 64")?;
            items.check(
                i,
                "bulk",
                !item.bulk || item.capacity > 0,
                "nur für Behälter mit `capacity`",
            )?;
            items.check(
                i,
                "stack",
                item.capacity == 0 || item.stack == 1,
                "Behälter sind nicht stapelbar",
            )?;
            item_defs.push(ItemDef {
                category,
                material,
                stack: item.stack,
                capacity: item.capacity,
                bulk: item.bulk,
                nutrition: item.nutrition,
                cooked: item.cooked,
                value: item.value,
                durability: item.durability,
            });
        }

        let mut material_defs = Vec::with_capacity(materials.len());
        let mut strata: Vec<(u32, u16)> = Vec::new();
        let mut veins: Vec<(u32, u16)> = Vec::new();
        let mut obsidian = Vec::new();
        let mut aquifer = Vec::new();
        for (i, m) in materials.iter() {
            let class = match m.class.as_str() {
                "soil" => MaterialClass::Soil,
                "rock" => MaterialClass::Rock,
                "ore" => MaterialClass::Ore,
                "metal" => MaterialClass::Metal,
                "wood" => MaterialClass::Wood,
                "plant" => MaterialClass::Plant,
                _ => {
                    return Err(materials.error(
                        i,
                        "class",
                        "erwartet soil, rock, ore, metal, wood oder plant",
                    ));
                }
            };
            materials.check(i, "dig_ticks", m.dig_ticks <= 100_000, "höchstens 100000")?;
            materials.check(
                i,
                "dig_item",
                m.dig_item.is_some() == (m.dig_ticks > 0),
                "abbaubare Materialien (dig_ticks > 0) brauchen genau dann ein dig_item",
            )?;
            materials.check(
                i,
                "conductivity",
                m.conductivity >= 0,
                "darf nicht negativ sein",
            )?;
            materials.check(i, "heat_capacity", m.heat_capacity > 0, "muss positiv sein")?;
            let dig_item = match &m.dig_item {
                Some(item) => Some(materials.resolve(i, "dig_item", &items, item)?),
                None => None,
            };
            let smelts_into = match &m.smelts_into {
                Some(target) => Some(materials.resolve(i, "smelts_into", &materials, target)?),
                None => None,
            };
            if let Some(stratum) = m.stratum {
                materials.check(
                    i,
                    "stratum",
                    m.dig_ticks > 0,
                    "Schichten müssen abbaubar sein",
                )?;
                if strata.iter().any(|&(s, _)| s == stratum) {
                    return Err(materials.error(i, "stratum", "Schicht ist bereits vergeben"));
                }
                strata.push((stratum, i));
            }
            if let Some(vein) = m.vein {
                materials.check(
                    i,
                    "vein",
                    class == MaterialClass::Ore && m.dig_ticks > 0,
                    "nur abbaubare Erze",
                )?;
                if veins.iter().any(|&(v, _)| v == vein) {
                    return Err(materials.error(
                        i,
                        "vein",
                        "Ader-Reihenfolge ist bereits vergeben",
                    ));
                }
                veins.push((vein, i));
            }
            if m.magma_product {
                obsidian.push(i);
            }
            if m.aquifer {
                aquifer.push(i);
            }
            material_defs.push(MaterialDef {
                class,
                dig_ticks: m.dig_ticks,
                dig_item,
                aquifer: m.aquifer,
                conductivity: m.conductivity,
                heat_capacity: m.heat_capacity,
                value: m.value,
                flammable: m.flammable,
                smelts_into,
            });
        }
        materials.reject_cycles(|i| {
            material_defs[usize::from(i)]
                .smelts_into
                .into_iter()
                .collect()
        })?;
        strata.sort_unstable();
        veins.sort_unstable();
        let first = |found: &[u16], key: &str, message: &str| {
            if found.len() == 1 {
                Ok(found[0])
            } else {
                Err(materials.error(found.get(1).copied().unwrap_or(0), key, message))
            }
        };
        let obsidian = first(
            &obsidian,
            "magma_product",
            "genau ein Material mit magma_product nötig",
        )?;
        let aquifer = first(&aquifer, "aquifer", "genau ein Material mit aquifer nötig")?;
        let soil = match strata.first() {
            Some(&(0, soil)) if material_defs[usize::from(soil)].class == MaterialClass::Soil => {
                soil
            }
            _ => {
                return Err(materials.error(
                    0,
                    "stratum",
                    "Schicht 0 muss ein Material der Klasse soil sein",
                ));
            }
        };
        let rocks: Vec<u16> = strata.iter().skip(1).map(|&(_, m)| m).collect();
        if rocks.is_empty() {
            return Err(materials.error(soil, "stratum", "mindestens eine Gesteinsschicht nötig"));
        }

        let mut skill_defs = Vec::with_capacity(skills.len());
        if skills.len() > MAX_SKILLS {
            return Err(ContentError::TooMany {
                kind: "skill".into(),
                max: MAX_SKILLS,
            });
        }
        for (i, s) in skills.iter() {
            skills.check(
                i,
                "levels",
                s.levels.first() == Some(&0) && s.levels.windows(2).all(|w| w[0] < w[1]),
                "beginnt mit 0 und steigt streng",
            )?;
            skill_defs.push(SkillDef {
                levels: s.levels.clone(),
            });
        }

        let mut workshop_defs = Vec::with_capacity(workshops.len());
        for (i, w) in workshops.iter() {
            workshops.check(i, "size", (1..=5).contains(&w.size), "zwischen 1 und 5")?;
            workshops.check(
                i,
                "build_ticks",
                (1..=100_000).contains(&w.build_ticks),
                "zwischen 1 und 100000",
            )?;
            workshops.check(
                i,
                "build",
                (1..=MAX_INPUTS).contains(&w.build.len()),
                "1 bis 4 Baumaterialien",
            )?;
            let build = w
                .build
                .iter()
                .enumerate()
                .map(|(k, a)| {
                    ingredient(
                        &workshops,
                        i,
                        &format!("build[{k}]"),
                        a,
                        &categories,
                        &items,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            workshop_defs.push(WorkshopDef {
                size: w.size,
                build,
                build_ticks: w.build_ticks,
            });
        }

        let fuel = required("category", categories.ids(), "fuel")? as u8;
        let mut recipe_defs = Vec::with_capacity(recipes.len());
        for (i, r) in recipes.iter() {
            let workshop = recipes.resolve(i, "workshop", &workshops, &r.workshop)?;
            let skill = recipes.resolve(i, "skill", &skills, &r.skill)?;
            recipes.check(
                i,
                "work_ticks",
                (1..=100_000).contains(&r.work_ticks),
                "zwischen 1 und 100000",
            )?;
            let count = r.inputs.len() + usize::from(r.fuel > 0);
            recipes.check(
                i,
                "inputs",
                (1..=MAX_INPUTS).contains(&count),
                "1 bis 4 Zutaten einschließlich Brennstoff",
            )?;
            let mut inputs = r
                .inputs
                .iter()
                .enumerate()
                .map(|(k, a)| {
                    ingredient(&recipes, i, &format!("inputs[{k}]"), a, &categories, &items)
                })
                .collect::<Result<Vec<_>, _>>()?;
            if r.fuel > 0 {
                recipes.check(i, "fuel", r.fuel <= 1000, "höchstens 1000")?;
                inputs.push(Input {
                    what: Ingredient::Category(fuel),
                    amount: r.fuel,
                    raw: false,
                });
            }
            recipes.check(
                i,
                "outputs",
                (1..=MAX_INPUTS).contains(&r.outputs.len()),
                "1 bis 4 Produkte",
            )?;
            let mut outputs = Vec::with_capacity(r.outputs.len());
            for (k, o) in r.outputs.iter().enumerate() {
                let key = format!("outputs[{k}]");
                let item = recipes.resolve(i, &key, &items, &o.item)?;
                recipes.check(
                    i,
                    &format!("{key}.amount"),
                    (1..=1000).contains(&o.amount),
                    "zwischen 1 und 1000",
                )?;
                let of = o.material_of.unwrap_or(0);
                recipes.check(
                    i,
                    &format!("{key}.material_of"),
                    usize::from(of) < r.inputs.len(),
                    "verweist auf keine Zutat",
                )?;
                let material = match (&o.material, o.smelted) {
                    (Some(m), false) => OutputMaterial::Fixed(recipes.resolve(
                        i,
                        &format!("{key}.material"),
                        &materials,
                        m,
                    )?),
                    (None, true) => OutputMaterial::Smelted(of),
                    (None, false) => OutputMaterial::Of(of),
                    (Some(_), true) => {
                        return Err(recipes.error(
                            i,
                            &key,
                            "`material` und `smelted` schließen sich aus",
                        ));
                    }
                };
                outputs.push(OutputDef {
                    item,
                    amount: o.amount,
                    material,
                });
            }
            recipe_defs.push(RecipeDef {
                workshop,
                skill,
                work_ticks: r.work_ticks,
                inputs,
                outputs,
            });
        }

        let seed = required("category", categories.ids(), "seed")? as u8;
        let mut plant_defs = Vec::with_capacity(plants.len());
        for (i, p) in plants.iter() {
            let material = plants.resolve(i, "material", &materials, &p.material)?;
            let skill = plants.resolve(i, "skill", &skills, &p.skill)?;
            let mut seasons = 0u8;
            for (k, name) in p.seasons.iter().enumerate() {
                let Some(s) = SEASONS.iter().position(|s| s == name) else {
                    return Err(plants.error(
                        i,
                        &format!("seasons[{k}]"),
                        "erwartet spring, summer, autumn oder winter",
                    ));
                };
                seasons |= 1 << s;
            }
            plants.check(i, "seasons", seasons != 0, "mindestens eine Jahreszeit")?;
            plants.check(
                i,
                "growth_ticks",
                (1..=1_000_000).contains(&p.growth_ticks),
                "zwischen 1 und 1000000",
            )?;
            plants.check(
                i,
                "work_ticks",
                (1..=100_000).contains(&p.work_ticks),
                "zwischen 1 und 100000",
            )?;
            plants.check(
                i,
                "tree",
                !(p.tree && p.underground),
                "Bäume wachsen an der Oberfläche",
            )?;
            plants.check(
                i,
                "seed",
                p.tree || p.seed.is_some(),
                "Feldfrüchte brauchen Samen",
            )?;
            let seed_item = match &p.seed {
                Some(s) => {
                    let item = plants.resolve(i, "seed", &items, s)?;
                    plants.check(
                        i,
                        "seed",
                        item_defs[usize::from(item)].category == seed,
                        "muss zur Kategorie seed gehören",
                    )?;
                    Some(item)
                }
                None => None,
            };
            plants.check(
                i,
                "harvest",
                (1..=MAX_INPUTS).contains(&p.harvest.len()),
                "1 bis 4 Erträge",
            )?;
            let harvest = p
                .harvest
                .iter()
                .enumerate()
                .map(|(k, y)| {
                    let key = format!("harvest[{k}]");
                    plants.check(
                        i,
                        &format!("{key}.amount"),
                        (1..=1000).contains(&y.amount),
                        "zwischen 1 und 1000",
                    )?;
                    Ok((plants.resolve(i, &key, &items, &y.item)?, y.amount))
                })
                .collect::<Result<Vec<_>, ContentError>>()?;
            plant_defs.push(PlantDef {
                tree: p.tree,
                underground: p.underground,
                material,
                seasons,
                growth_ticks: p.growth_ticks,
                seed: seed_item,
                skill,
                work_ticks: p.work_ticks,
                harvest,
            });
        }

        let food = required("category", categories.ids(), "food")? as u8;
        for (i, item) in item_defs.iter().enumerate() {
            items.check(
                i as u16,
                "nutrition",
                (item.category == food) == (item.nutrition > 0),
                "genau Gegenstände der Kategorie food haben nutrition > 0",
            )?;
        }
        let ration = required("item", items.ids(), "ration")?;
        let Some(ration_material) = item_defs[usize::from(ration)].material else {
            return Err(items.error(ration, "material", "Rationen brauchen ein Material"));
        };
        let bulk_categories = item_defs
            .iter()
            .filter(|d| d.bulk)
            .fold(0u64, |mask, d| mask | 1 << d.category);
        Ok(Self {
            fingerprint: content.fingerprint(),
            bulk_categories,
            ration_material,
            stone: required("category", categories.ids(), "stone")? as u8,
            log: required("category", categories.ids(), "log")? as u8,
            ration,
            food,
            seed,
            fuel,
            ids: Ids {
                categories: categories.ids().to_vec(),
                materials: materials.ids().to_vec(),
                items: items.ids().to_vec(),
                workshops: workshops.ids().to_vec(),
                recipes: recipes.ids().to_vec(),
                plants: plants.ids().to_vec(),
                skills: skills.ids().to_vec(),
            },
            categories: category_defs,
            materials: material_defs,
            items: item_defs,
            workshops: workshop_defs,
            recipes: recipe_defs,
            plants: plant_defs,
            skills: skill_defs,
            soil,
            rocks,
            ores: veins.iter().map(|&(_, m)| m).collect(),
            obsidian,
            aquifer,
        })
    }

    pub fn material(&self, index: u16) -> Option<&MaterialDef> {
        self.materials.get(usize::from(index))
    }

    pub fn item(&self, index: u16) -> Option<&ItemDef> {
        self.items.get(usize::from(index))
    }

    pub fn category_of(&self, item: u16) -> u8 {
        self.item(item).map_or(0, |d| d.category)
    }

    /// Bit mask of `category` and all categories below it.
    pub fn descendants(&self, category: u8) -> u64 {
        self.categories
            .iter()
            .enumerate()
            .filter(|(_, c)| c.ancestors & (1 << category) != 0)
            .fold(0, |mask, (i, _)| mask | 1 << i)
    }

    /// Whether item kind `item` matches an ingredient.
    pub fn accepts(&self, input: &Input, item: u16) -> bool {
        let Some(def) = self.item(item) else {
            return false;
        };
        let kind = match input.what {
            Ingredient::Item(wanted) => wanted == item,
            Ingredient::Category(category) => self
                .categories
                .get(usize::from(def.category))
                .is_some_and(|c| c.ancestors & (1 << category) != 0),
        };
        kind && !(input.raw && def.cooked)
    }

    pub fn id_of(ids: &[String], id: &str) -> Option<u16> {
        ids.binary_search_by(|probe| probe.as_str().cmp(id))
            .ok()
            .map(|i| i as u16)
    }
}
