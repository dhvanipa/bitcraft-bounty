//! holdings.podlang, the generic module: loading it, and proving its
//! `LiveObject(state_header, obj, tag)`, which `Holds` and the bounty's
//! `LiveGem` (bounty.podlang, which imports holdings.podlang by its hash) both
//! build on: `obj` is in the created array of `state_header`, its nullifier is
//! absent from its nullifier set, so the object is unspent at that state
//! root, and `tag` is its tag.
//!
//! The proofs come from a synchronizer with the holdings witness endpoint
//! (`POST /v1/holdings/witness`, synchronizer/patches/): the hosted one cannot
//! prove a nullifier absent, and only answers against its head.

use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail, ensure};
use pod2::{
    backends::plonky2::primitives::merkletree::MerkleProof,
    frontend::{MultiPodBuilder, Operation, entry},
    lang::{Module, load_module},
    middleware::{
        Hash, NativeOperation, OperationAux, OperationType, Params, Statement, StrKey, Value,
        containers::Dictionary, hash_values,
    },
};
use pod2utils::{macros::BuildContext, op};
use serde::{Deserialize, Serialize};
use txlib::{STATE_HEADER_CREATED_SLOT, STATE_HEADER_NULLIFIERS_SLOT, StateHeader};

const HOLDINGS_PODLANG: &str = include_str!("holdings.podlang");
/// txlib's nullifier domain (`OBJECT_NULLIFIER_VERSION`, crate-private there).
pub const NULLIFIER_VERSION: &str = "txlib-nullifier-v1";
/// The tag's domain; holdings.podlang writes the same string.
pub const TAG_VERSION: &str = "dobj-object-tag-v1";

/// holdings.podlang: `StateHeader`, `LiveObject` and `Holds`.
pub fn holdings_module() -> Result<Arc<Module>> {
    let module = load_module(HOLDINGS_PODLANG, "holdings", &Params::default(), &[])
        .map_err(|err| anyhow!("holdings.podlang does not compile: {err}"))?;
    Ok(Arc::new(module))
}

/// Compiles `source`, which imports holdings.podlang as
/// `use module HOLDINGS_MODULE as holdings`, with its hash written in.
pub fn import_holdings(source: &str, name: &str) -> Result<Arc<Module>> {
    let holdings = holdings_module()?;
    let source = source.replace("HOLDINGS_MODULE", &format!("{:#}", holdings.id()));
    let module = load_module(&source, name, &Params::default(), &[holdings])
        .map_err(|err| anyhow!("{name}.podlang does not compile: {err}"))?;
    Ok(Arc::new(module))
}

/// A proving context for `module`'s predicates and, when `module` imports
/// it, holdings.podlang's.
pub fn context(builder: MultiPodBuilder, module: Arc<Module>) -> Result<BuildContext> {
    let holdings = holdings_module()?;
    let modules = if module.id() == holdings.id() {
        vec![module]
    } else {
        vec![module, holdings]
    };
    Ok(BuildContext::new(builder, modules))
}

/// Items per witness request: the synchronizer takes at most 256 hashes in
/// one query (`MAX_HASH_QUERY_ITEMS`), and each item sends two.
const ITEMS_PER_REQUEST: usize = 128;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WitnessRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    block_number: Option<i64>,
    object_commitments: Vec<Hash>,
    nullifiers: Vec<Hash>,
}

/// The synchronizer's answer: one state's header, and for each object asked
/// about, its creation proof and its nullifier's proof under that state.
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
    txlib::object_nullifier_hash(obj)
}

/// Asks a synchronizer with the holdings witness endpoint for the proofs about
/// `objects` at the state of execution block `block_number` (the head when
/// None), in requests of at most 128 objects, all at the first answer's block.
pub fn fetch_witness(sync: &str, block_number: Option<i64>, objects: &[Dictionary]) -> Result<Witness> {
    let url = format!("{}/v1/holdings/witness", sync.trim_end_matches('/'));
    let client = reqwest::blocking::Client::new();
    let mut witness: Option<Witness> = None;
    for batch in objects.chunks(ITEMS_PER_REQUEST) {
        let request = WitnessRequest {
            block_number: witness.as_ref().map(|w| w.block_number).or(block_number),
            object_commitments: batch.iter().map(|obj| obj.commitment()).collect(),
            nullifiers: batch.iter().map(nullifier_of).collect::<Result<_>>()?,
        };
        let response = client
            .post(&url)
            .json(&request)
            .send()
            .with_context(|| format!("asking {url}"))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            // an unknown route is a bare 404; the endpoint's own 404 says why
            if status == reqwest::StatusCode::NOT_FOUND && body.is_empty() {
                bail!(
                    "{sync} has no /v1/holdings/witness: prove against a synchronizer with \
                     bitcraft-bounty's patch (synchronizer/)"
                );
            }
            bail!("{url} answered {status}: {body}");
        }
        let part: Witness = response.json()?;
        ensure!(
            part.header().hash() == part.state_root,
            "the witness's header does not hash to its state root"
        );
        ensure!(
            part.created_proofs.len() == batch.len() && part.nullifier_proofs.len() == batch.len(),
            "the witness answers for {} objects, not {}",
            part.created_proofs.len(),
            batch.len()
        );
        match &mut witness {
            None => witness = Some(part),
            Some(all) => {
                ensure!(
                    part.state_root == all.state_root,
                    "the synchronizer answered at two states for block {}",
                    all.block_number
                );
                all.created_proofs.extend(part.created_proofs);
                all.nullifier_proofs.extend(part.nullifier_proofs);
            }
        }
    }
    witness.context("no objects to ask about")
}

/// An object with the proofs that it is in the created array and that its
/// nullifier is absent from the nullifier set of one state.
pub struct LiveObject {
    pub obj: Dictionary,
    pub index: i64,
    pub created_proof: MerkleProof,
    pub nullifier_proof: MerkleProof,
}

/// Why an object can't be proved live at a state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotLive {
    /// Its commitment is not in the created array: its transaction had not
    /// landed by then, or the file is not a real object state.
    NotCreated,
    /// Its nullifier is in the nullifier set: consumed, or re-keyed, by then.
    Spent,
}

impl std::fmt::Display for NotLive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            NotLive::NotCreated => "not created at this state",
            NotLive::Spent => "spent at this state",
        })
    }
}

/// Each of `objects` with its proofs from `witness`, in order, or why it is not
/// live there.
pub fn live_objects(
    objects: Vec<Dictionary>,
    witness: &Witness,
) -> Result<Vec<Result<LiveObject, NotLive>>> {
    ensure!(objects.len() == witness.created_proofs.len(), "the witness is for other objects");
    objects
        .into_iter()
        .zip(witness.created_proofs.iter().zip(&witness.nullifier_proofs))
        .map(|(obj, (created, nullified))| {
            if !created.present {
                return Ok(Err(NotLive::NotCreated));
            }
            if nullified.spent {
                return Ok(Err(NotLive::Spent));
            }
            Ok(Ok(LiveObject {
                index: created.index.context("no created index")?,
                created_proof: created.proof.clone().context("no created proof")?,
                nullifier_proof: nullified.proof.clone(),
                obj,
            }))
        })
        .collect()
}

pub fn field(obj: &Dictionary, name: &str) -> Result<Value> {
    obj.get(&StrKey::from(name))?
        .with_context(|| format!("the item has no {name} field"))
}

/// An object's tag: H(stable_identifier, TAG_VERSION), the same for every
/// state of the object.
pub fn tag_of(obj: &Dictionary) -> Result<Hash> {
    Ok(hash_values(&[field(obj, "stable_identifier")?, Value::from(TAG_VERSION)]))
}

/// Proves `LiveObject(state_header, obj, tag)` for one object, in a context
/// from `context`; returns the statement and the object's tag.
pub fn prove_live_object(
    ctx: &mut BuildContext,
    header: &StateHeader,
    item: &LiveObject,
) -> Result<(Statement, Hash)> {
    let header_array = header.array();
    let obj = &item.obj;
    let st_created = ctx.builder.priv_op(Operation(
        OperationType::Native(NativeOperation::ArrayContainsFromEntries),
        vec![
            Value::from(header.created_root).into(),
            Value::from(item.index).into(),
            Value::from(obj.clone()).into(),
        ],
        OperationAux::MerkleProof(item.created_proof.clone()),
    ))?;
    // Bind the created root to slot `created` of the state header record.
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
    // ...and the nullifiers root to slot `nullifiers`.
    let st_unspent = ctx.builder.priv_op(Operation::replace_value_with_entry(
        vec![Some(entry(&header_array, STATE_HEADER_NULLIFIERS_SLOT as i64)?), None],
        st_unspent,
    ))?;
    let tag = tag_of(obj)?;
    let st_tag = ctx.builder.priv_op(op!(Hash(
        entry(obj, "stable_identifier")?,
        TAG_VERSION,
        tag
    )))?;
    let st_live = ctx.apply_custom_pred_simple(
        false,
        "LiveObject",
        vec![st_created, st_key_hash, st_nullifier, st_unspent, st_tag],
    )?;
    Ok((st_live, tag))
}

/// States and objects for tests: the mock prover checks every operation, so
/// these exercise the predicates without real proofs.
#[cfg(test)]
pub mod testing {
    use pod2::middleware::containers::{Array, Set};

    use super::*;

    /// An object of class `class` whose key is `key`, with its stable
    /// identifier stamped as TxInsert does.
    pub fn object(class: Hash, key: i64) -> Dictionary {
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
    /// its nullifier set.
    pub struct State {
        pub header: StateHeader,
        created: Vec<Dictionary>,
        array: Array,
        nullifiers: Set,
    }

    impl State {
        pub fn new(created: &[Dictionary], spent: &[Dictionary]) -> State {
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
            State {
                header,
                created: created.to_vec(),
                array,
                nullifiers,
            }
        }

        /// `obj` with its creation proof and the proof its nullifier is absent
        /// (or, when it is spent, present: the only proof there is).
        pub fn live(&self, obj: &Dictionary) -> LiveObject {
            let index = self.created.iter().position(|o| o == obj).unwrap();
            let (_, created_proof) = self.array.prove(index).unwrap();
            let nullifier = Value::from(nullifier_of(obj).unwrap());
            let nullifier_proof = self
                .nullifiers
                .prove_nonexistence(&nullifier)
                .or_else(|_| self.nullifiers.prove(&nullifier))
                .unwrap();
            LiveObject {
                obj: obj.clone(),
                index: index as i64,
                created_proof,
                nullifier_proof,
            }
        }

        pub fn is_spent(&self, obj: &Dictionary) -> bool {
            self.nullifiers
                .contains(&Value::from(nullifier_of(obj).unwrap()))
                .unwrap()
        }
    }
}
