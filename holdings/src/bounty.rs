//! The bounty claim, `BountyClaim(state_header, recipient, threshold)`
//! (bounty.podlang): unspent Bitcraft items worth at least `threshold`
//! centi-BIT at one state root, for `recipient`, proved as a pod's only public
//! statement so that pod2-onchain's wrapper can turn it into a proof a
//! contract checks.
//!
//! Each item is a `LiveObject` at that root (live.rs): created and unspent
//! there, by proofs from a synchronizer with the holdings witness endpoint. Its
//! value is the Bitcraft app's (apps/bitcraft/src-tauri/src/metrics.rs,
//! `object_value`), from the app's inventory-values.json, in centi-BIT
//! (1/100 BIT) so that every value is a whole number: a Stick is 0.5 BIT, a
//! MiningDrill at level 1 67.5 BIT, an IronAxe at durability 37 3.33 BIT.

use std::{collections::HashSet, sync::Arc};

use anyhow::{Context, Result, bail, ensure};
use pod2::{
    frontend::entry,
    lang::Module,
    middleware::{
        F, Hash, RawValue, Statement, Value,
        containers::{Dictionary, Set},
        hash_values,
    },
};
use pod2utils::{macros::BuildContext, op};
use txlib::StateHeader;

use crate::{
    INVENTORY_VALUES,
    live::{LiveObject, Witness, field, import_holdings, live_objects, prove_live_object, tag_of},
};

const BOUNTY_PODLANG: &str = include_str!("bounty.podlang");

/// The Bitcraft plugin's classes (module 0x54548dac8ef1..., the network's):
/// each class's name and `Is<Class>` predicate hash, as `holdings classes`
/// prints them; `holdings bounty-info --pexe` checks them against the plugin.
pub const CLASSES: [(&str, &str); 27] = [
    ("Amethyst", "0x0e55065947112afd4299b4c50de9df8f39586dc63720b84f17c6019b2296ea2a"),
    ("BarrelOfOil", "0x606ffcef0005367419cddcd225dcd49c3c8d4d432e03265dcafd3d599f331ad5"),
    ("Bit", "0x7e47be67498e8be58f42e86f235f64bf5bbea32afc6f52c5b921a7314bb0ccdc"),
    ("BucketOfBits", "0x6a636fed3daa29444e6f47b7789ed5e52c939857510b55b47420e27a14473a20"),
    ("BucketOfCoal", "0x761f59beca1873881b12f00331ddce36ade4f2990d8a19186b64c2968b0f271e"),
    ("BucketOfIron", "0xc44291495b974f652ddc4bb732d3c52ca9d542f490685b22c135ede2b0a1a4f7"),
    ("BucketOfQuartz", "0x74f0b178a2268f61caab74fbd2ff9f32a4504db687c86977c6161de8002e3b09"),
    ("BucketOfStone", "0x09c04b9377f8c2ccbdb35ed5a7e4948ddc134d5bdffc23cb532fcac83529272f"),
    ("BucketOfWood", "0x401574324f8315bc6829d50dab431f97eab39e81b721b4fcce8e0c7e741df07d"),
    ("Coal", "0x452e6fe635c260ec29443b5c81eeeb438090b26c59b5ee9306a8c6df6d264af2"),
    ("Diamond", "0x7bcbc10f30ddd08caf37b931ba2594799f014ddd38bc8d0e1e92659146bd73de"),
    ("Emerald", "0x87c2c758e8d81ccb603ed25ad7b4c6d4cdea3e4ff014e9765240d9ec945fe227"),
    ("Geode", "0xa071f08736eeaa63bff86feeacadf2e873789004e55c33e7b507628ea2659e16"),
    ("Iron", "0x8988865c6b938bfe74b5c349b14be3eb0011ce1a440e9b772fbef600ce458bd2"),
    ("IronAxe", "0x9bfb93179cdb2e14fc2752a27b20d96c8039d7e2c1906dc03e11ff6318008e14"),
    ("MiningDrill", "0x8775a7e5f813832a03ebc03d0720c575c6458a402022566cbe37cda496d68003"),
    ("Pocketknife", "0xf97d623828244bec6183b3eb28b48bb82084aa4f9d6df7733daa82bd359018fa"),
    ("Pumpjack", "0xb497ee727a7dd057f33dd2aad349c114d48a7abf4c777488b4b9386b14401cc5"),
    ("Quarry", "0x76b361f79a7f4122a201dae52cfaeb246c8f492c791803fdf1011c018ccb4d2f"),
    ("Quartz", "0xf197e1b8e73034cb294828bdf6dc6e98b69b190b511ebef05578ccbb48b8f19d"),
    ("Ruby", "0xd8a37aa915d430a44ab243baf4a8bbabaca126d0d5796fbfd7b0a9469d61e54d"),
    ("Sapphire", "0x46f6cf84cdef15cca4040a0ff4fb9c91744b0a34dbc335738d87d9168c9ffb9b"),
    ("Stick", "0xe2410e4b12bd29e69aa6bad47f4f688cb0736c978fd9d90b3a0167b43f109108"),
    ("Stone", "0x7fbaa9bfba7b9c098398b7cb647c7bc48ee42b91fd8f5693305e0c62d83ef037"),
    ("StoneAxe", "0xeac72fd87f462e410c32257ca34c98f2131543989a98fcd42d8975c45fa12964"),
    ("Wood", "0xd583894285b3ed3630b7bc60fc9499b410b99f9313d9c84acf0985f15aace36a"),
    ("WoodAxe", "0x86165cbd3801da7bf2010488f4ebfa5e8fd65bf4c4527a74a63c5a5bc0ca3988"),
];

/// How the app values an item of one class, in centi-BIT.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rule {
    /// The same for every item (TOTAL_BIT_VALUES).
    Fixed { value: i64 },
    /// `base + per_unit * n` (TOTAL_BIT_VALUES plus BUCKET_CONTENT_BIT_VALUES).
    Bucket { base: i64, per_unit: i64 },
    /// `by_level[level - 1]` for level 1 to `by_level.len()`
    /// (LEVELED_TOTAL_BIT_VALUES); 0 at any other level.
    Leveled { by_level: Vec<i64> },
    /// The full value in BIT times `durability` (TOTAL_BIT_VALUES scaled by
    /// FULL_DURABILITY, which is 100: value * durability / 100 BIT).
    Axe { full_bit: i64 },
}

pub struct ClassRule {
    pub name: &'static str,
    pub class: Hash,
    pub rule: Rule,
}

/// `bit` in centi-BIT, which must be a whole number.
fn centi(bit: f64) -> Result<i64> {
    let c = (bit * 100.0).round();
    ensure!(
        (c - bit * 100.0).abs() < 1e-6,
        "{bit} BIT is not a whole number of centi-BIT"
    );
    Ok(c as i64)
}

/// Each class's rule, read from the app's inventory-values.json the way
/// metrics.rs's `object_value` applies it. Fails on a table entry for a class
/// not in CLASSES, or on a value it can't count exactly.
pub fn rules() -> Result<Vec<ClassRule>> {
    let tables: serde_json::Value = serde_json::from_str(INVENTORY_VALUES)?;
    let table = |name: &str| -> Result<&serde_json::Map<String, serde_json::Value>> {
        tables[name]
            .as_object()
            .with_context(|| format!("inventory-values.json has no {name}"))
    };
    let (total, bucket, leveled, durability) = (
        table("TOTAL_BIT_VALUES")?,
        table("BUCKET_CONTENT_BIT_VALUES")?,
        table("LEVELED_TOTAL_BIT_VALUES")?,
        table("FULL_DURABILITY")?,
    );
    for listed in [total, bucket, leveled, durability] {
        for name in listed.keys() {
            ensure!(
                CLASSES.iter().any(|(class, _)| class == name),
                "inventory-values.json values {name}, which is not in CLASSES"
            );
        }
    }
    let number = |v: &serde_json::Value, what: &str| -> Result<f64> {
        v.as_f64().with_context(|| format!("{what} is not a number"))
    };
    CLASSES
        .iter()
        .map(|(name, hex)| {
            let class = payload::decode_hash_hex(hex)
                .with_context(|| format!("the {name} class hash"))?;
            // metrics.rs: a leveled class's value is its table's alone
            let rule = if let Some(levels) = leveled.get(*name) {
                let by_level = levels
                    .as_array()
                    .with_context(|| format!("{name}'s levels"))?
                    .iter()
                    .map(|v| centi(number(v, name)?))
                    .collect::<Result<_>>()?;
                Rule::Leveled { by_level }
            } else {
                let base = total.get(*name).map(|v| number(v, name)).transpose()?.unwrap_or(0.0);
                match (bucket.get(*name), durability.get(*name)) {
                    (Some(per_unit), None) => Rule::Bucket {
                        base: centi(base)?,
                        per_unit: centi(number(per_unit, name)?)?,
                    },
                    (None, Some(full)) => {
                        ensure!(
                            number(full, name)? == 100.0 && base.fract() == 0.0,
                            "{name}: only a full durability of 100 and a whole BIT value count exactly"
                        );
                        Rule::Axe { full_bit: base as i64 }
                    }
                    (None, None) => Rule::Fixed { value: centi(base)? },
                    (Some(_), Some(_)) => bail!("{name} has both contents and durability"),
                }
            };
            Ok(ClassRule { name, class, rule })
        })
        .collect()
}

/// The hash of a class by name, for the tests' items.
#[cfg(test)]
pub fn class_hash(name: &str) -> Result<Hash> {
    let (_, hex) = CLASSES
        .iter()
        .find(|(class, _)| *class == name)
        .with_context(|| format!("no class {name}"))?;
    payload::decode_hash_hex(hex).with_context(|| format!("the {name} class hash"))
}

/// The four tables of bounty.podlang, built from `rules()`. Each class is in
/// exactly one; classes worth nothing (Pocketknife, Geode) are in none.
pub struct Tables {
    pub fixed: Set,
    pub bucket: Set,
    pub leveled: Set,
    pub axe: Set,
}

fn h(a: impl Into<Value>, b: impl Into<Value>) -> Hash {
    hash_values(&[a.into(), b.into()])
}

impl Tables {
    pub fn new(rules: &[ClassRule]) -> Tables {
        let (mut fixed, mut bucket, mut leveled, mut axe) =
            (HashSet::new(), HashSet::new(), HashSet::new(), HashSet::new());
        for ClassRule { class, rule, .. } in rules {
            match rule {
                Rule::Fixed { value } if *value != 0 => {
                    fixed.insert(Value::from(h(*class, *value)));
                }
                Rule::Fixed { .. } => {}
                Rule::Bucket { base, per_unit } => {
                    bucket.insert(Value::from(h(h(*class, *base), *per_unit)));
                }
                Rule::Leveled { by_level } => {
                    for (i, value) in by_level.iter().enumerate() {
                        leveled.insert(Value::from(h(h(*class, i as i64 + 1), *value)));
                    }
                }
                Rule::Axe { full_bit } => {
                    axe.insert(Value::from(h(*class, *full_bit)));
                }
            }
        }
        Tables {
            fixed: Set::new(fixed),
            bucket: Set::new(bucket),
            leveled: Set::new(leveled),
            axe: Set::new(axe),
        }
    }

    fn literal(set: &Set) -> Result<String> {
        let mut entries = set
            .iter()
            .map(|v| v.map(|v| format!("Untyped({:#})", Hash::from(v.raw()))))
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort();
        Ok(format!("#[{}]", entries.join(", ")))
    }
}

/// bounty.podlang with its tables written in as set literals.
pub fn bounty_module() -> Result<Arc<Module>> {
    let tables = Tables::new(&rules()?);
    let source = BOUNTY_PODLANG
        .replace("FIXED_VALUES", &Tables::literal(&tables.fixed)?)
        .replace("BUCKET_VALUES", &Tables::literal(&tables.bucket)?)
        .replace("LEVELED_VALUES", &Tables::literal(&tables.leveled)?)
        .replace("AXE_VALUES", &Tables::literal(&tables.axe)?);
    import_holdings(&source, "bounty")
}

/// An Ethereum address as a pod2 value: its 160 bits in 4 limbs of 40 bits,
/// least significant first, so each limb is far below the Goldilocks modulus
/// and a contract reads it back as l0 | l1 << 40 | l2 << 80 | l3 << 120.
pub fn recipient_value(address: &str) -> Result<Value> {
    let hex = address.strip_prefix("0x").unwrap_or(address);
    ensure!(
        hex.len() == 40 && hex.chars().all(|c| c.is_ascii_hexdigit()),
        "{address} is not an Ethereum address"
    );
    let bits = u128::from_str_radix(&hex[8..], 16)?; // the low 128 bits
    let top = u64::from_str_radix(&hex[..8], 16)? as u128; // the high 32 bits
    let mask = (1u128 << 40) - 1;
    let limbs = [
        bits & mask,
        (bits >> 40) & mask,
        (bits >> 80) & mask,
        (bits >> 120) | (top << 8),
    ];
    Ok(Value::from(RawValue(limbs.map(|l| F(l as u64)))))
}

// ============================================================================
// An item's value
// ============================================================================

/// An item's value and the fields it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Valued {
    Fixed { value: i64 },
    Bucket { base: i64, per_unit: i64, n: i64 },
    Leveled { level: i64, value: i64 },
    Axe { full_bit: i64, durability: i64 },
}

impl Valued {
    /// In centi-BIT.
    pub fn value(&self) -> i64 {
        match self {
            Valued::Fixed { value } | Valued::Leveled { value, .. } => *value,
            Valued::Bucket { base, per_unit, n } => base + per_unit * n,
            Valued::Axe { full_bit, durability } => full_bit * durability,
        }
    }
}

fn int_field(obj: &Dictionary, name: &str) -> Result<i64> {
    field(obj, name)?
        .as_int()
        .with_context(|| format!("the item's {name} is not an integer"))
}

/// The class name and the value of `obj` as the app counts it, or why it
/// can't count: a class worth nothing, a level outside the table, or a field
/// the value needs and the item lacks.
pub fn item_value(rules: &[ClassRule], obj: &Dictionary) -> Result<(&'static str, Result<Valued, String>)> {
    let class = Hash::from(field(obj, "type")?.raw());
    let Some(ClassRule { name, rule, .. }) = rules.iter().find(|r| r.class == class) else {
        return Ok(("unknown class", Err("not a Bitcraft class".into())));
    };
    let valued = match rule {
        Rule::Fixed { value: 0 } => Err("worth nothing".into()),
        Rule::Fixed { value } => Ok(Valued::Fixed { value: *value }),
        Rule::Bucket { base, per_unit } => int_field(obj, "n")
            .map(|n| Valued::Bucket { base: *base, per_unit: *per_unit, n })
            .map_err(|e| e.to_string()),
        Rule::Leveled { by_level } => match int_field(obj, "level") {
            Ok(level) if level >= 1 && level as usize <= by_level.len() => Ok(Valued::Leveled {
                level,
                value: by_level[level as usize - 1],
            }),
            Ok(level) => Err(format!("level {level} has no value")),
            Err(e) => Err(e.to_string()),
        },
        Rule::Axe { full_bit } => int_field(obj, "durability")
            .map(|durability| Valued::Axe { full_bit: *full_bit, durability })
            .map_err(|e| e.to_string()),
    };
    Ok((name, valued))
}

// ============================================================================
// The claim
// ============================================================================

/// What the claim needs for one item: a live object and its value.
pub struct LiveItem {
    pub name: &'static str,
    pub live: LiveObject,
    pub valued: Valued,
}

/// The items of `objects` that are live at the witness's state and worth
/// something, with their proofs, in order; and, for each other one, its index
/// in `objects`, class name and why it doesn't count.
#[allow(clippy::type_complexity)]
pub fn live_items(
    objects: Vec<Dictionary>,
    witness: &Witness,
) -> Result<(Vec<LiveItem>, Vec<(usize, &'static str, String)>)> {
    let rules = rules()?;
    let mut items = Vec::new();
    let mut skipped = Vec::new();
    for (i, live) in live_objects(objects.clone(), witness)?.into_iter().enumerate() {
        let (name, valued) = item_value(&rules, &objects[i])?;
        match (live, valued) {
            (Ok(live), Ok(valued)) => items.push(LiveItem { name, live, valued }),
            (Err(why), _) => skipped.push((i, name, why.to_string())),
            (Ok(_), Err(why)) => skipped.push((i, name, why)),
        }
    }
    Ok((items, skipped))
}

/// What a claim of `items` would use: one file per object (copies of one
/// object share its tag, and a claim counts each object once), then the
/// fewest items that reach `threshold`, most valuable first. Returns the
/// chosen items, how many copies were dropped, and the value of every
/// distinct item, all in centi-BIT. Fails when even every item is below the
/// threshold.
///
/// pod2 plans a claim's pods in time that grows faster than its item count
/// (100 items took 6 s on 2026-11-09, 300 items 52 s), so a claim names as
/// few items as it can. Taking the largest values first gives the fewest:
/// no k items are worth more than the k most valuable.
pub fn select(items: Vec<LiveItem>, threshold: i64) -> Result<(Vec<LiveItem>, usize, i64)> {
    let given = items.len();
    let mut seen = HashSet::new();
    let mut distinct = Vec::with_capacity(given);
    for item in items {
        if seen.insert(tag_of(&item.live.obj)?) {
            distinct.push(item);
        }
    }
    let copies = given - distinct.len();
    let available: i64 = distinct.iter().map(|item| item.valued.value()).sum();
    ensure!(
        available >= threshold,
        "the live items are worth {} BIT in all, below the threshold of {} BIT",
        available as f64 / 100.0,
        threshold as f64 / 100.0
    );
    distinct.sort_by_key(|item| std::cmp::Reverse(item.valued.value()));
    let mut sum = 0;
    let keep = distinct
        .iter()
        .take_while(|item| {
            let more = sum < threshold;
            sum += item.valued.value();
            more
        })
        .count();
    distinct.truncate(keep);
    Ok((distinct, copies, available))
}

/// Proves `ItemValue(obj, value)` for one item.
fn prove_item_value(ctx: &mut BuildContext, tables: &Tables, item: &LiveItem) -> Result<Statement> {
    let obj = &item.live.obj;
    let class = field(obj, "type")?;
    let value = item.valued.value();
    let (branch, sts) = match &item.valued {
        Valued::Fixed { value } => {
            let e = h(class, *value);
            (0, vec![
                ctx.builder.priv_op(op!(Hash(entry(obj, "type")?, *value, e)))?,
                ctx.builder.priv_op(op!(SetContains(&tables.fixed, e)))?,
            ])
        }
        Valued::Bucket { base, per_unit, n } => {
            let key = h(class, *base);
            let e = h(key, *per_unit);
            let contents = per_unit * n;
            (1, vec![
                ctx.builder.priv_op(op!(Hash(entry(obj, "type")?, *base, key)))?,
                ctx.builder.priv_op(op!(Hash(key, *per_unit, e)))?,
                ctx.builder.priv_op(op!(SetContains(&tables.bucket, e)))?,
                ctx.builder.priv_op(op!(Product(*per_unit, entry(obj, "n")?, contents)))?,
                ctx.builder.priv_op(op!(Sum(*base, contents, value)))?,
            ])
        }
        Valued::Leveled { level, value } => {
            let key = h(class, *level);
            let e = h(key, *value);
            (2, vec![
                ctx.builder.priv_op(op!(Hash(entry(obj, "type")?, entry(obj, "level")?, key)))?,
                ctx.builder.priv_op(op!(Hash(key, *value, e)))?,
                ctx.builder.priv_op(op!(SetContains(&tables.leveled, e)))?,
            ])
        }
        Valued::Axe { full_bit, .. } => {
            let e = h(class, *full_bit);
            (3, vec![
                ctx.builder.priv_op(op!(Hash(entry(obj, "type")?, *full_bit, e)))?,
                ctx.builder.priv_op(op!(SetContains(&tables.axe, e)))?,
                ctx.builder.priv_op(op!(Product(*full_bit, entry(obj, "durability")?, value)))?,
            ])
        }
    };
    let names = ["FixedValue", "BucketValue", "LeveledValue", "AxeValue"];
    let st_branch = ctx.apply_custom_pred_simple(false, names[branch], sts)?;
    let mut or = vec![Statement::None; 4];
    or[branch] = st_branch;
    ctx.apply_custom_pred_simple(false, "ItemValue", or)
}

/// Proves `LiveItem(state_header, value, tag)` for one item; returns the
/// statement and the item's tag.
fn prove_live_item(
    ctx: &mut BuildContext,
    header: &StateHeader,
    tables: &Tables,
    item: &LiveItem,
) -> Result<(Statement, Hash)> {
    let (st_live, tag) = prove_live_object(ctx, header, &item.live)?;
    let st_value = prove_item_value(ctx, tables, item)?;
    let st_item = ctx.apply_custom_pred_simple(false, "LiveItem", vec![st_live, st_value])?;
    Ok((st_item, tag))
}

/// Proves `BountyClaim(state_header, recipient, threshold)` as a public
/// statement: the items' total, each counted once, is at least `threshold`
/// centi-BIT. Returns the statement and the total in centi-BIT.
pub fn prove_claim(
    ctx: &mut BuildContext,
    header: &StateHeader,
    items: &[LiveItem],
    recipient: &Value,
    threshold: i64,
) -> Result<(Statement, i64)> {
    ensure!(!items.is_empty(), "a claim needs at least one item");
    let tables = Tables::new(&rules()?);
    let mut total_statement: Option<(Statement, i64, Set)> = None;
    for item in items {
        let value = item.valued.value();
        let (st_item, tag) = prove_live_item(ctx, header, &tables, item)?;
        total_statement = Some(match total_statement {
            None => {
                let empty = Set::new(HashSet::new());
                let mut tags = empty.clone();
                tags.insert(&Value::from(tag))?;
                let st_insert = ctx.builder.priv_op(op!(SetInsert(empty, tag, tags)))?;
                let st_one =
                    ctx.apply_custom_pred_simple(false, "ItemsTotalOne", vec![st_item, st_insert])?;
                let st_total = ctx.apply_custom_pred_simple(
                    false,
                    "ItemsTotal",
                    vec![st_one, Statement::None],
                )?;
                (st_total, value, tags)
            }
            Some((st_prev, prev_total, prev_tags)) => {
                let mut tags = prev_tags.clone();
                if tags.contains(&Value::from(tag))? {
                    bail!("item {} is the same object as an earlier one", item.live.obj.commitment());
                }
                tags.insert(&Value::from(tag))?;
                let total = prev_total + value;
                let st_sum = ctx.builder.priv_op(op!(Sum(prev_total, value, total)))?;
                let st_insert = ctx
                    .builder
                    .priv_op(op!(SetInsert(prev_tags, tag, tags)))?;
                let st_more = ctx.apply_custom_pred_simple(
                    false,
                    "ItemsTotalMore",
                    vec![st_prev, st_item, st_sum, st_insert],
                )?;
                let st_total = ctx.apply_custom_pred_simple(
                    false,
                    "ItemsTotal",
                    vec![Statement::None, st_more],
                )?;
                (st_total, total, tags)
            }
        });
    }
    let (st_total, total, _) = total_statement.expect("at least one item");
    ensure!(
        total >= threshold,
        "the items are worth {} BIT, below the threshold of {} BIT",
        total as f64 / 100.0,
        threshold as f64 / 100.0
    );
    let st_enough = ctx.builder.priv_op(op!(GtEq(total, threshold)))?;
    let st_recipient = ctx.builder.priv_op(op!(Equal(recipient, recipient)))?;
    let st_claim = ctx.apply_custom_pred_simple(
        true,
        "BountyClaim",
        vec![st_total, st_enough, st_recipient],
    )?;
    Ok((st_claim, total))
}

#[cfg(test)]
mod tests {
    use pod2::{
        backends::plonky2::mock::mainpod::MockProver,
        frontend::MultiPodBuilder,
        middleware::{Params, StrKey, VDSet},
    };

    use anyhow::anyhow;

    use super::*;
    use crate::live::{context, testing::State};

    /// An item of class `name` with key `key` and the given integer fields,
    /// its stable identifier stamped as TxInsert does.
    fn item_obj(name: &str, key: i64, fields: &[(&str, i64)]) -> Dictionary {
        let mut kvs = vec![
            (StrKey::from("type"), Value::from(class_hash(name).unwrap())),
            (StrKey::from("key"), Value::from(key)),
        ];
        for (field, value) in fields {
            kvs.push((StrKey::from(*field), Value::from(*value)));
        }
        txlib::with_stable_identifier(&Dictionary::new(kvs.into_iter().collect()))
    }

    fn valued(obj: &Dictionary) -> Valued {
        item_value(&rules().unwrap(), obj).unwrap().1.unwrap()
    }

    fn item(obj: &Dictionary, state: &State) -> LiveItem {
        LiveItem {
            name: "",
            live: state.live(obj),
            valued: valued(obj),
        }
    }

    fn claim(header: &StateHeader, items: &[LiveItem], threshold: i64) -> Result<i64> {
        let params = Params::default();
        let mut ctx = context(MultiPodBuilder::new(&params, &VDSet::new(&[])), bounty_module()?)?;
        let recipient = recipient_value("0x00000000000000000000000000000000000000ee")?;
        let (_, total) = prove_claim(&mut ctx, header, items, &recipient, threshold)?;
        let pod = ctx
            .builder
            .solve()
            .map_err(|err| anyhow!(err))?
            .prove(&MockProver {})
            .map_err(|err| anyhow!(err))?
            .output_pod()
            .clone();
        pod.pod.verify().map_err(|err| anyhow!("{err}"))?;
        anyhow::ensure!(pod.pod.pub_statements().len() == 1);
        Ok(total)
    }

    #[test]
    fn values_match_the_apps_rules() {
        // metrics.rs's own cases (shared_rules_value_levels_contents_and_wear),
        // in centi-BIT, and a few more
        for (name, fields, centi_bit) in [
            ("Quarry", vec![("level", 3)], 14_000),
            ("BucketOfIron", vec![("n", 10)], 5_500),
            ("IronAxe", vec![("durability", 50)], 450),
            ("IronAxe", vec![("durability", 37)], 333),
            ("MiningDrill", vec![("level", 1)], 6_750),
            ("Stick", vec![], 50),
            ("Emerald", vec![], 80_000_000),
            ("BarrelOfOil", vec![("n", 3)], 2_500 + 3 * 5_000),
        ] {
            assert_eq!(valued(&item_obj(name, 1, &fields)).value(), centi_bit, "{name} {fields:?}");
        }
        let rules = rules().unwrap();
        for (name, fields) in [("Quarry", vec![("level", 11)]), ("Pocketknife", vec![]), ("IronAxe", vec![])] {
            assert!(item_value(&rules, &item_obj(name, 1, &fields)).unwrap().1.is_err(), "{name}");
        }
    }

    #[test]
    fn every_class_is_in_one_table() {
        let rules = rules().unwrap();
        assert_eq!(rules.len(), CLASSES.len());
        let kinds = |pred: fn(&Rule) -> bool| rules.iter().filter(|r| pred(&r.rule)).count();
        assert_eq!(kinds(|r| matches!(r, Rule::Leveled { .. })), 3);
        assert_eq!(kinds(|r| matches!(r, Rule::Bucket { .. })), 7);
        assert_eq!(kinds(|r| matches!(r, Rule::Axe { .. })), 3);
        assert_eq!(kinds(|r| matches!(r, Rule::Fixed { .. })), 14);
    }

    #[test]
    fn items_of_every_kind_reach_the_threshold() -> Result<()> {
        let objs = vec![
            item_obj("Emerald", 1, &[]),
            item_obj("BucketOfIron", 2, &[("n", 10)]),
            item_obj("Quarry", 3, &[("level", 3)]),
            item_obj("IronAxe", 4, &[("durability", 37)]),
            item_obj("Stick", 5, &[]),
        ];
        let state = State::new(&objs, &[]);
        let items: Vec<LiveItem> = objs.iter().map(|obj| item(obj, &state)).collect();
        let total = 80_000_000 + 5_500 + 14_000 + 333 + 50;
        assert_eq!(claim(&state.header, &items, total)?, total);
        assert!(claim(&state.header, &items, total + 1).is_err(), "below the threshold");
        Ok(())
    }

    #[test]
    fn a_wrong_value_is_refused() -> Result<()> {
        let emerald = item_obj("Emerald", 1, &[]);
        let quarry = item_obj("Quarry", 2, &[("level", 3)]);
        let bucket = item_obj("BucketOfIron", 3, &[("n", 10)]);
        let axe = item_obj("IronAxe", 4, &[("durability", 37)]);
        let state = State::new(&[emerald.clone(), quarry.clone(), bucket.clone(), axe.clone()], &[]);
        let with = |obj: &Dictionary, valued: Valued| LiveItem { name: "", live: state.live(obj), valued };
        for (obj, wrong) in [
            (&emerald, Valued::Fixed { value: 1_600_000_000 }), // a Ruby's value
            (&quarry, Valued::Leveled { level: 4, value: 30_000 }), // level 4's value
            (&bucket, Valued::Bucket { base: 500, per_unit: 500, n: 20 }), // more contents
            (&axe, Valued::Axe { full_bit: 9, durability: 100 }), // unworn
            (&emerald, Valued::Axe { full_bit: 9, durability: 100 }), // another table
        ] {
            assert!(claim(&state.header, &[with(obj, wrong.clone())], 1).is_err(), "{wrong:?}");
        }
        Ok(())
    }

    #[test]
    fn a_spent_item_cannot_be_claimed() -> Result<()> {
        let emerald = item_obj("Emerald", 1, &[]);
        let spent = item_obj("Ruby", 2, &[]);
        let created = vec![emerald.clone(), spent.clone()];
        let state = State::new(&created, std::slice::from_ref(&spent));
        // its nullifier is in the set, so there is no proof of its absence;
        // with the proof of presence instead, the claim fails
        assert!(state.is_spent(&spent));
        assert!(claim(&state.header, &[item(&spent, &state)], 1).is_err());
        assert_eq!(claim(&state.header, &[item(&emerald, &state)], 80_000_000)?, 80_000_000);
        Ok(())
    }

    #[test]
    fn the_same_object_counts_once() -> Result<()> {
        let emerald = item_obj("Emerald", 1, &[]);
        let state = State::new(std::slice::from_ref(&emerald), &[]);
        assert!(claim(&state.header, &[item(&emerald, &state), item(&emerald, &state)], 1).is_err());
        Ok(())
    }

    #[test]
    fn a_claim_takes_the_fewest_most_valuable_items() -> Result<()> {
        let objs = vec![
            item_obj("Stone", 1, &[]),
            item_obj("Sapphire", 2, &[]),
            item_obj("Emerald", 3, &[]),
            item_obj("Sapphire", 4, &[]),
            item_obj("Quarry", 5, &[("level", 10)]),
        ];
        let state = State::new(&objs, &[]);
        let items = || objs.iter().map(|obj| item(obj, &state)).collect::<Vec<_>>();
        let names = |chosen: &[LiveItem]| -> Vec<i64> { chosen.iter().map(|i| i.valued.value()).collect() };
        // 800,000 + 40,000 BIT needs the Emerald and one Sapphire
        let (chosen, copies, available) = select(items(), 84_000_000)?;
        assert_eq!(names(&chosen), vec![80_000_000, 4_000_000]);
        assert_eq!((copies, available), (0, 80_000_000 + 2 * 4_000_000 + 2_046_000 + 200));
        // one more centi-BIT needs the next most valuable item too
        assert_eq!(select(items(), 84_000_001)?.0.len(), 3);
        // a copy of the Emerald's file counts once
        let mut doubled = items();
        doubled.push(item(&objs[2], &state));
        let (chosen, copies, _) = select(doubled, 80_000_000)?;
        assert_eq!((chosen.len(), copies), (1, 1));
        // everything together is not enough
        assert!(select(items(), 100_000_000).is_err());
        // and the chosen items prove the claim
        let (chosen, _, _) = select(items(), 84_000_000)?;
        assert_eq!(claim(&state.header, &chosen, 84_000_000)?, 84_000_000);
        Ok(())
    }

    #[test]
    fn a_recipient_packs_into_40_bit_limbs() -> Result<()> {
        let value = recipient_value("0xfedcba9876543210fedcba9876543210fedcba98")?;
        let limbs = value.raw().0.map(|f| f.0 as u128);
        assert!(limbs.iter().all(|&l| l < 1 << 40));
        // l0 | l1 << 40 | l2 << 80 | l3 << 120, in two parts: 160 bits overflow a u128
        let low = limbs[0] | limbs[1] << 40 | limbs[2] << 80 | (limbs[3] & 0xff) << 120;
        let top = limbs[3] >> 8;
        assert_eq!(format!("{top:08x}{low:032x}"), "fedcba9876543210fedcba9876543210fedcba98");
        Ok(())
    }
}
