//! The bounty claim, `BountyClaim(state_header, recipient, threshold)`
//! (bounty.podlang): unspent gems worth at least `threshold` BIT at one state
//! root, for `recipient`, proved as a pod's only public statement so that
//! pod2-onchain's wrapper can turn it into a proof a contract checks.
//!
//! Each gem is a `LiveObject` at that root (live.rs): created and unspent
//! there, by proofs from a synchronizer with the holdings witness endpoint.

use std::{collections::HashSet, sync::Arc};

use anyhow::{Context, Result, anyhow, bail, ensure};
use pod2::{
    frontend::entry,
    lang::Module,
    middleware::{
        F, Hash, Params, RawValue, Statement, Value,
        containers::{Dictionary, Set},
        hash_values,
    },
};
use pod2utils::{macros::BuildContext, op};
use txlib::StateHeader;

use crate::live::{LiveObject, Witness, field, import_holdings, live_objects, prove_live_object};

const BOUNTY_PODLANG: &str = include_str!("bounty.podlang");

/// The Bitcraft plugin's gem classes (module 0x54548dac8ef1..., the
/// network's): each class's `Is<Class>` predicate hash, as `holdings classes`
/// prints it, and its value in BIT (the app's inventory-values.json).
pub const GEMS: [(&str, &str, i64); 6] = [
    ("Quartz", "0xf197e1b8e73034cb294828bdf6dc6e98b69b190b511ebef05578ccbb48b8f19d", 100),
    ("Amethyst", "0x0e55065947112afd4299b4c50de9df8f39586dc63720b84f17c6019b2296ea2a", 2_000),
    ("Sapphire", "0x46f6cf84cdef15cca4040a0ff4fb9c91744b0a34dbc335738d87d9168c9ffb9b", 40_000),
    ("Emerald", "0x87c2c758e8d81ccb603ed25ad7b4c6d4cdea3e4ff014e9765240d9ec945fe227", 800_000),
    ("Ruby", "0xd8a37aa915d430a44ab243baf4a8bbabaca126d0d5796fbfd7b0a9469d61e54d", 16_000_000),
    ("Diamond", "0x7bcbc10f30ddd08caf37b931ba2594799f014ddd38bc8d0e1e92659146bd73de", 320_000_000),
];

pub struct Gem {
    pub name: &'static str,
    pub class: Hash,
    pub value: i64,
}

pub fn gems() -> Result<Vec<Gem>> {
    GEMS.iter()
        .map(|(name, hex, value)| {
            Ok(Gem {
                name,
                class: payload::decode_hash_hex(hex)
                    .with_context(|| format!("the {name} class hash"))?,
                value: *value,
            })
        })
        .collect()
}

/// A gem's entry in GEM_VALUES: H(class, value).
fn gem_entry(class: Hash, value: i64) -> Hash {
    hash_values(&[Value::from(class), Value::from(value)])
}

/// GEM_VALUES, the set of every gem's entry.
pub fn gem_values() -> Result<Set> {
    let entries = gems()?
        .iter()
        .map(|gem| Value::from(gem_entry(gem.class, gem.value)))
        .collect();
    Ok(Set::new(entries))
}

/// bounty.podlang with GEM_VALUES written in as a set literal.
pub fn bounty_module() -> Result<Arc<Module>> {
    let entries: Vec<String> = gems()?
        .iter()
        .map(|gem| format!("Untyped({:#})", gem_entry(gem.class, gem.value)))
        .collect();
    let source = BOUNTY_PODLANG.replace("GEM_VALUES", &format!("#[{}]", entries.join(", ")));
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
// The claim
// ============================================================================

/// What the claim needs for one item: a live object and its gem value.
pub struct LiveItem {
    pub live: LiveObject,
    pub value: i64,
}

/// The items of `objects` with their proofs from `witness`, in order; each must
/// be a gem, live at the witness's state.
pub fn live_items(objects: Vec<Dictionary>, witness: &Witness) -> Result<Vec<LiveItem>> {
    let gems = gems()?;
    let commitments: Vec<Hash> = objects.iter().map(|obj| obj.commitment()).collect();
    live_objects(objects, witness)?
        .into_iter()
        .zip(commitments)
        .map(|(live, commitment)| {
            let live = live.map_err(|why| {
                anyhow!("item {commitment} is {why} (block {})", witness.block_number)
            })?;
            let class = Hash::from(field(&live.obj, "type")?.raw());
            let gem = gems
                .iter()
                .find(|gem| gem.class == class)
                .with_context(|| format!("item {commitment} is not a gem"))?;
            Ok(LiveItem {
                value: gem.value,
                live,
            })
        })
        .collect()
}

/// Proves `LiveGem(state_header, value, tag)` for one item; returns the
/// statement and the item's tag.
fn prove_live_gem(
    ctx: &mut BuildContext,
    header: &StateHeader,
    gem_values: &Set,
    item: &LiveItem,
) -> Result<(Statement, Hash)> {
    let obj = &item.live.obj;
    let (st_live, tag) = prove_live_object(ctx, header, &item.live)?;
    let class = field(obj, "type")?;
    let gem_entry = hash_values(&[class, Value::from(item.value)]);
    let st_entry = ctx
        .builder
        .priv_op(op!(Hash(entry(obj, "type")?, item.value, gem_entry)))?;
    let st_table = ctx.builder.priv_op(op!(SetContains(gem_values, gem_entry)))?;
    let st_gem = ctx.apply_custom_pred_simple(
        false,
        "LiveGem",
        vec![st_live, st_entry, st_table],
    )?;
    Ok((st_gem, tag))
}

/// Proves `BountyClaim(state_header, recipient, threshold)` as a public
/// statement: the items' total, each counted once, is at least `threshold`.
pub fn prove_claim(
    ctx: &mut BuildContext,
    header: &StateHeader,
    items: &[LiveItem],
    recipient: &Value,
    threshold: i64,
) -> Result<(Statement, i64)> {
    ensure!(!items.is_empty(), "a claim needs at least one gem");
    let gem_values = gem_values()?;
    let mut total_statement: Option<(Statement, i64, Set)> = None;
    for item in items {
        let (st_gem, tag) = prove_live_gem(ctx, header, &gem_values, item)?;
        total_statement = Some(match total_statement {
            None => {
                let empty = Set::new(HashSet::new());
                let mut tags = empty.clone();
                tags.insert(&Value::from(tag))?;
                let st_insert = ctx.builder.priv_op(op!(SetInsert(empty, tag, tags)))?;
                let st_one =
                    ctx.apply_custom_pred_simple(false, "GemsTotalOne", vec![st_gem, st_insert])?;
                let st_total = ctx.apply_custom_pred_simple(
                    false,
                    "GemsTotal",
                    vec![st_one, Statement::None],
                )?;
                (st_total, item.value, tags)
            }
            Some((st_prev, prev_total, prev_tags)) => {
                let mut tags = prev_tags.clone();
                if tags.contains(&Value::from(tag))? {
                    bail!("item {} is the same object as an earlier one", item.live.obj.commitment());
                }
                tags.insert(&Value::from(tag))?;
                let total = prev_total + item.value;
                let st_sum = ctx
                    .builder
                    .priv_op(op!(Sum(prev_total, item.value, total)))?;
                let st_insert = ctx
                    .builder
                    .priv_op(op!(SetInsert(prev_tags, tag, tags)))?;
                let st_more = ctx.apply_custom_pred_simple(
                    false,
                    "GemsTotalMore",
                    vec![st_prev, st_gem, st_sum, st_insert],
                )?;
                let st_total = ctx.apply_custom_pred_simple(
                    false,
                    "GemsTotal",
                    vec![Statement::None, st_more],
                )?;
                (st_total, total, tags)
            }
        });
    }
    let (st_total, total, _) = total_statement.expect("at least one item");
    ensure!(
        total >= threshold,
        "the gems are worth {total} BIT, below the threshold of {threshold}"
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
        backends::plonky2::mock::mainpod::MockProver, frontend::MultiPodBuilder,
        middleware::{StrKey, VDSet},
    };

    use super::*;
    use crate::live::testing::{State, object};

    fn gem(name: &str, key: i64) -> Dictionary {
        let class = gems()
            .unwrap()
            .into_iter()
            .find(|gem| gem.name == name)
            .unwrap()
            .class;
        object(class, key)
    }

    /// `obj` as the claim sees it, worth `value`.
    fn item_worth(obj: &Dictionary, value: i64, state: &State) -> LiveItem {
        LiveItem {
            live: state.live(obj),
            value,
        }
    }

    fn item(obj: &Dictionary, state: &State) -> LiveItem {
        let class = Hash::from(field(obj, "type").unwrap().raw());
        let value = gems().unwrap().iter().find(|g| g.class == class).unwrap().value;
        item_worth(obj, value, state)
    }

    fn claim(header: &StateHeader, items: &[LiveItem], threshold: i64) -> Result<i64> {
        let params = Params::default();
        let mut ctx = crate::live::context(MultiPodBuilder::new(&params, &VDSet::new(&[])), bounty_module()?)?;
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
    fn three_gems_reach_the_threshold() -> Result<()> {
        let emerald = gem("Emerald", 1);
        let sapphire = gem("Sapphire", 2);
        let diamond = gem("Diamond", 3);
        let created = vec![emerald.clone(), sapphire.clone(), diamond.clone()];
        let state = State::new(&created, &[]);
        let items: Vec<LiveItem> = created.iter().map(|obj| item(obj, &state)).collect();
        assert_eq!(claim(&state.header, &items, 320_840_000)?, 320_840_000);
        assert!(claim(&state.header, &items, 320_840_001).is_err(), "below the threshold");
        Ok(())
    }

    #[test]
    fn a_spent_gem_cannot_be_claimed() -> Result<()> {
        let emerald = gem("Emerald", 1);
        let spent = gem("Ruby", 2);
        let created = vec![emerald.clone(), spent.clone()];
        let state = State::new(&created, std::slice::from_ref(&spent));
        // its nullifier is in the set, so there is no proof of its absence;
        // with the proof of presence instead, the claim fails
        assert!(state.is_spent(&spent));
        let ruby = item(&spent, &state);
        assert!(claim(&state.header, &[ruby], 1).is_err());
        // the unspent Emerald alone is fine
        let fine = item(&emerald, &state);
        assert_eq!(claim(&state.header, &[fine], 800_000)?, 800_000);
        Ok(())
    }

    #[test]
    fn the_same_object_counts_once() -> Result<()> {
        let emerald = gem("Emerald", 1);
        let created = vec![emerald.clone()];
        let state = State::new(&created, &[]);
        let one = item(&emerald, &state);
        let again = item(&emerald, &state);
        assert!(claim(&state.header, &[one, again], 1).is_err());
        Ok(())
    }

    #[test]
    fn a_non_gem_or_a_wrong_value_is_refused() -> Result<()> {
        // an object of another class, claimed at an Emerald's value
        let mut other = gem("Emerald", 1);
        other.update(&StrKey::from("type"), &Value::from(Hash::default()))?;
        let emerald = gem("Emerald", 2);
        let created = vec![other.clone(), emerald.clone()];
        let state = State::new(&created, &[]);
        let fake = item_worth(&other, 800_000, &state);
        assert!(claim(&state.header, &[fake], 1).is_err());
        // a real Emerald claimed at a Ruby's value
        let inflated = item_worth(&emerald, 16_000_000, &state);
        assert!(claim(&state.header, &[inflated], 1).is_err());
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
