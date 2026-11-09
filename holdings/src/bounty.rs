//! The bounty claim, `BountyClaim(state_header, recipient, threshold)`
//! (bounty.podlang): unspent gems worth at least `threshold` BIT at one state
//! root, for `recipient`, proved as a pod's only public statement so that
//! pod2-onchain's wrapper can turn it into a proof a contract checks.
//!
//! The proofs that each gem was created and is unspent at that root come from
//! a synchronizer with the holdings witness endpoint
//! (`POST /v1/holdings/witness`, synchronizer/patches/): the hosted one only
//! answers against its head and cannot prove a nullifier absent.

use std::{collections::HashSet, sync::Arc};

use anyhow::{Context, Result, anyhow, bail, ensure};
use pod2::{
    backends::plonky2::primitives::merkletree::MerkleProof,
    frontend::{Operation, entry},
    lang::{Module, load_module},
    middleware::{
        F, Hash, NativeOperation, OperationAux, OperationType, Params, RawValue, Statement,
        StrKey, Value,
        containers::{Dictionary, Set},
        hash_values,
    },
};
use pod2utils::{macros::BuildContext, op};
use serde::{Deserialize, Serialize};
use txlib::{STATE_HEADER_CREATED_SLOT, STATE_HEADER_NULLIFIERS_SLOT, StateHeader};

use crate::{NULLIFIER_VERSION, TAG_VERSION};

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
    let module = load_module(&source, "bounty", &Params::default(), &[])
        .map_err(|err| anyhow!("bounty.podlang does not compile: {err}"))?;
    Ok(Arc::new(module))
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
// The synchronizer's witness
// ============================================================================

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WitnessRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    block_number: Option<i64>,
    object_commitments: Vec<Hash>,
    nullifiers: Vec<Hash>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Witness {
    pub state_root: Hash,
    pub block_number: i64,
    block_timestamp: i64,
    block_hash: Hash,
    created_root: Hash,
    nullifiers_root: Hash,
    prior_state_history_root: Hash,
    created_proofs: Vec<CreatedProof>,
    nullifier_proofs: Vec<NullifierProof>,
}

#[derive(Deserialize)]
struct CreatedProof {
    present: bool,
    index: Option<i64>,
    proof: Option<MerkleProof>,
}

#[derive(Deserialize)]
struct NullifierProof {
    spent: bool,
    proof: MerkleProof,
}

impl Witness {
    pub fn header(&self) -> StateHeader {
        StateHeader::new(
            self.block_number,
            self.block_timestamp,
            self.block_hash,
            self.created_root,
            self.nullifiers_root,
            self.prior_state_history_root,
        )
    }
}

/// An object's nullifier, as txlib computes it.
pub fn nullifier_of(obj: &Dictionary) -> Result<Hash> {
    Ok(txlib::object_nullifier_hash(obj)?)
}

/// Asks a synchronizer with the holdings witness endpoint for the proofs about
/// `objects` at the state of `block_number` (the head when None), and checks
/// they place every object, unspent, under the state root it returns.
pub fn fetch_witness(sync: &str, block_number: Option<i64>, objects: &[Dictionary]) -> Result<Witness> {
    let request = WitnessRequest {
        block_number,
        object_commitments: objects.iter().map(|obj| obj.commitment()).collect(),
        nullifiers: objects.iter().map(nullifier_of).collect::<Result<_>>()?,
    };
    let url = format!("{}/v1/holdings/witness", sync.trim_end_matches('/'));
    let witness: Witness = reqwest::blocking::Client::new()
        .post(&url)
        .json(&request)
        .send()
        .with_context(|| format!("asking {url}"))?
        .error_for_status()?
        .json()?;
    ensure!(
        witness.header().hash() == witness.state_root,
        "the witness's header does not hash to its state root"
    );
    ensure!(witness.created_proofs.len() == objects.len());
    ensure!(witness.nullifier_proofs.len() == objects.len());
    for (i, obj) in objects.iter().enumerate() {
        let created = &witness.created_proofs[i];
        ensure!(
            created.present,
            "item {} is not in the created set at block {}",
            obj.commitment(),
            witness.block_number
        );
        ensure!(
            !witness.nullifier_proofs[i].spent,
            "item {} is spent at block {}",
            obj.commitment(),
            witness.block_number
        );
    }
    Ok(witness)
}

// ============================================================================
// The claim
// ============================================================================

/// What the claim needs for one item: the object, its gem value, and the proofs
/// of its creation and of its nullifier's absence under the header's roots.
pub struct LiveItem {
    pub obj: Dictionary,
    pub value: i64,
    pub index: i64,
    pub created_proof: MerkleProof,
    pub nullifier_proof: MerkleProof,
}

/// The items of `objects` with their proofs from `witness`, in order; each must
/// be a gem.
pub fn live_items(objects: Vec<Dictionary>, witness: &Witness) -> Result<Vec<LiveItem>> {
    let gems = gems()?;
    objects
        .into_iter()
        .zip(witness.created_proofs.iter().zip(&witness.nullifier_proofs))
        .map(|(obj, (created, nullified))| {
            let class = Hash::from(field(&obj, "type")?.raw());
            let gem = gems
                .iter()
                .find(|gem| gem.class == class)
                .with_context(|| format!("item {} is not a gem", obj.commitment()))?;
            Ok(LiveItem {
                value: gem.value,
                index: created.index.context("no created index")?,
                created_proof: created.proof.clone().context("no created proof")?,
                nullifier_proof: nullified.proof.clone(),
                obj,
            })
        })
        .collect()
}

fn field(obj: &Dictionary, name: &str) -> Result<Value> {
    obj.get(&StrKey::from(name))?
        .with_context(|| format!("the item has no {name} field"))
}

/// Proves `LiveGem(state_header, value, tag)` for one item; returns the
/// statement and the item's tag.
fn prove_live_gem(
    ctx: &mut BuildContext,
    header: &StateHeader,
    gem_values: &Set,
    item: &LiveItem,
) -> Result<(Statement, Hash)> {
    let header_array = header.array();
    let obj = &item.obj;

    // LiveObject
    let st_created = ctx.builder.priv_op(Operation(
        OperationType::Native(NativeOperation::ArrayContainsFromEntries),
        vec![
            Value::from(header.created_root).into(),
            Value::from(item.index).into(),
            Value::from(obj.clone()).into(),
        ],
        OperationAux::MerkleProof(item.created_proof.clone()),
    ))?;
    let st_created = ctx.builder.priv_op(Operation::replace_value_with_entry(
        vec![Some(entry(&header_array, STATE_HEADER_CREATED_SLOT as i64)?), None, None],
        st_created,
    ))?;
    let key_hash = hash_values(&[Value::from(obj.commitment()), field(obj, "key")?]);
    let st_key_hash = ctx
        .builder
        .priv_op(op!(Hash(obj, entry(obj, "key")?, key_hash)))?;
    let nullifier = hash_values(&[Value::from(key_hash), Value::from(NULLIFIER_VERSION)]);
    ensure!(nullifier == nullifier_of(obj)?, "nullifier differs from txlib's");
    let st_nullifier = ctx
        .builder
        .priv_op(op!(Hash(key_hash, NULLIFIER_VERSION, nullifier)))?;
    let st_unspent = ctx.builder.priv_op(Operation(
        OperationType::Native(NativeOperation::SetNotContainsFromEntries),
        vec![
            Value::from(header.nullifiers_root).into(),
            Value::from(nullifier).into(),
        ],
        OperationAux::MerkleProof(item.nullifier_proof.clone()),
    ))?;
    let st_unspent = ctx.builder.priv_op(Operation::replace_value_with_entry(
        vec![Some(entry(&header_array, STATE_HEADER_NULLIFIERS_SLOT as i64)?), None],
        st_unspent,
    ))?;
    let st_live = ctx.apply_custom_pred_simple(
        false,
        "LiveObject",
        vec![st_created, st_key_hash, st_nullifier, st_unspent],
    )?;

    // LiveGem
    let class = field(obj, "type")?;
    let gem_entry = hash_values(&[class, Value::from(item.value)]);
    let st_entry = ctx
        .builder
        .priv_op(op!(Hash(entry(obj, "type")?, item.value, gem_entry)))?;
    let st_table = ctx.builder.priv_op(op!(SetContains(gem_values, gem_entry)))?;
    let tag = hash_values(&[field(obj, "stable_identifier")?, Value::from(TAG_VERSION)]);
    let st_tag = ctx.builder.priv_op(op!(Hash(
        entry(obj, "stable_identifier")?,
        TAG_VERSION,
        tag
    )))?;
    let st_gem = ctx.apply_custom_pred_simple(
        false,
        "LiveGem",
        vec![st_live, st_entry, st_table, st_tag],
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
                    bail!("item {} is the same object as an earlier one", item.obj.commitment());
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
        backends::plonky2::mock::mainpod::MockProver,
        frontend::MultiPodBuilder,
        middleware::{VDSet, containers::Array},
    };

    use super::*;

    fn gem(name: &str, key: i64) -> Dictionary {
        let class = gems()
            .unwrap()
            .into_iter()
            .find(|gem| gem.name == name)
            .unwrap()
            .class;
        let initial = Dictionary::new(
            [
                (StrKey::from("type"), Value::from(class)),
                (StrKey::from("key"), Value::from(key)),
            ]
            .into(),
        );
        txlib::with_stable_identifier(&initial)
    }

    /// A state with `created` in its created array and `spent`'s nullifiers in
    /// its nullifier set, and a witness for `claimed` against it.
    fn state(created: &[Dictionary], spent: &[Dictionary]) -> (StateHeader, Array, Set) {
        let array = Array::new(created.iter().map(|obj| Value::from(obj.clone())).collect());
        let nullifiers = Set::new(
            spent
                .iter()
                .map(|obj| Value::from(nullifier_of(obj).unwrap()))
                .collect(),
        );
        let header = StateHeader::new(
            7,
            7000,
            Hash::default(),
            array.commitment(),
            nullifiers.commitment(),
            Hash::default(),
        );
        (header, array, nullifiers)
    }

    /// `obj` as the claim sees it, worth `value`, with its creation proof and
    /// the proof its nullifier is absent (or, when it is spent, present: the
    /// only proof there is).
    fn item_worth(
        obj: &Dictionary,
        value: i64,
        created: &[Dictionary],
        array: &Array,
        nullifiers: &Set,
    ) -> LiveItem {
        let index = created.iter().position(|o| o == obj).unwrap();
        let (_, created_proof) = array.prove(index).unwrap();
        let nullifier = Value::from(nullifier_of(obj).unwrap());
        let nullifier_proof = nullifiers
            .prove_nonexistence(&nullifier)
            .or_else(|_| nullifiers.prove(&nullifier))
            .unwrap();
        LiveItem {
            obj: obj.clone(),
            value,
            index: index as i64,
            created_proof,
            nullifier_proof,
        }
    }

    fn item(obj: &Dictionary, created: &[Dictionary], array: &Array, nullifiers: &Set) -> LiveItem {
        let class = Hash::from(field(obj, "type").unwrap().raw());
        let value = gems().unwrap().iter().find(|g| g.class == class).unwrap().value;
        item_worth(obj, value, created, array, nullifiers)
    }

    fn claim(header: &StateHeader, items: &[LiveItem], threshold: i64) -> Result<i64> {
        let params = Params::default();
        let mut ctx = BuildContext::new(
            MultiPodBuilder::new(&params, &VDSet::new(&[])),
            vec![bounty_module()?],
        );
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
        let (header, array, nullifiers) = state(&created, &[]);
        let items: Vec<LiveItem> = created
            .iter()
            .map(|obj| item(obj, &created, &array, &nullifiers))
            .collect();
        assert_eq!(claim(&header, &items, 320_840_000)?, 320_840_000);
        assert!(claim(&header, &items, 320_840_001).is_err(), "below the threshold");
        Ok(())
    }

    #[test]
    fn a_spent_gem_cannot_be_claimed() -> Result<()> {
        let emerald = gem("Emerald", 1);
        let spent = gem("Ruby", 2);
        let created = vec![emerald.clone(), spent.clone()];
        let (header, array, nullifiers) = state(&created, &[spent.clone()]);
        // its nullifier is in the set, so there is no proof of its absence
        assert!(
            nullifiers
                .prove_nonexistence(&Value::from(nullifier_of(&spent)?))
                .is_err()
        );
        // with the proof of presence instead, the claim fails
        let ruby = item(&spent, &created, &array, &nullifiers);
        assert!(claim(&header, &[ruby], 1).is_err());
        // the unspent Emerald alone is fine
        let fine = item(&emerald, &created, &array, &nullifiers);
        assert_eq!(claim(&header, &[fine], 800_000)?, 800_000);
        Ok(())
    }

    #[test]
    fn the_same_object_counts_once() -> Result<()> {
        let emerald = gem("Emerald", 1);
        let created = vec![emerald.clone()];
        let (header, array, nullifiers) = state(&created, &[]);
        let one = item(&emerald, &created, &array, &nullifiers);
        let again = item(&emerald, &created, &array, &nullifiers);
        assert!(claim(&header, &[one, again], 1).is_err());
        Ok(())
    }

    #[test]
    fn a_non_gem_or_a_wrong_value_is_refused() -> Result<()> {
        // an object of another class, claimed at an Emerald's value
        let mut other = gem("Emerald", 1);
        other.update(&StrKey::from("type"), &Value::from(Hash::default()))?;
        let emerald = gem("Emerald", 2);
        let created = vec![other.clone(), emerald.clone()];
        let (header, array, nullifiers) = state(&created, &[]);
        let fake = item_worth(&other, 800_000, &created, &array, &nullifiers);
        assert!(claim(&header, &[fake], 1).is_err());
        // a real Emerald claimed at a Ruby's value
        let inflated = item_worth(&emerald, 16_000_000, &created, &array, &nullifiers);
        assert!(claim(&header, &[inflated], 1).is_err());
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
