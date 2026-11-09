//! `holdings`: prove that you hold Bitcraft items without showing them, and
//! check such a proof. A prototype of a leaderboard whose scores are proofs.
//!
//! - `prove` reads item files (`.dobj`, as in `~/.dobj/objects/`) and proves,
//!   for each, `Holds(state_root, type, nullifier, tag)` (`holdings.podlang`)
//!   in one pod2 MainPod, with the player's name as a public statement. The
//!   item files never leave the machine: holding the file is owning the item.
//! - `verify` checks that the MainPod is a real proof over the network's
//!   circuits, that each claim is our `Holds`, which Bitcraft class each
//!   `type` is (by compiling the plugin), and asks the synchronizer that each
//!   state root is one the chain reached and that no claimed item has been
//!   spent. It prints the claims and the total BIT value.
//! - `classes` prints the plugin's module hash and each class's type hash.
//!
//! Values are the Bitcraft app's (`apps/bitcraft/ui/inventory-values.json`).
//! `Holds` does not prove an item's level, fill or durability, so only
//! classes with one fixed value (gems, resources) are counted.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use payload::{db_bytes_to_hash, encode_hash_hex};
use pod2::{
    backends::plonky2::{
        basetypes::DEFAULT_VD_SET, mainpod::Prover, mock::mainpod::MockProver,
        primitives::merkletree::MerkleProof,
    },
    frontend::{MainPod, MultiPodBuilder, Operation, entry},
    lang::{Module, load_module},
    middleware::{
        CustomPredicateRef, Hash, MainPodProver, NativeOperation, OperationAux, OperationType,
        Predicate,
        Params, PodType, Statement, StrKey, VDSet, Value, ValueRef,
        containers::{Array, Dictionary},
        hash_values,
    },
};
use pod2utils::{macros::BuildContext, op};
use serde::{Deserialize, Serialize};
use txlib::{STATE_HEADER_CREATED_SLOT, StateHeader};
use wire_types::synchronizer::{MembershipRequest, MembershipResponse, StateHeadResponse};

mod bounty;

const HOLDINGS_PODLANG: &str = include_str!("holdings.podlang");
/// The Bitcraft app's value table, at the game commit this is built from.
const INVENTORY_VALUES: &str = include_str!("../../../apps/bitcraft/ui/inventory-values.json");
/// txlib's nullifier domain (`OBJECT_NULLIFIER_VERSION`, crate-private there).
const NULLIFIER_VERSION: &str = "txlib-nullifier-v1";
/// The tag's domain; `holdings.podlang` writes the same string.
const TAG_VERSION: &str = "bitcraft-holdings-v1";
const FILE_VERSION: u32 = 1;
const DEFAULT_SYNC: &str = "https://synchronizer.digiobjects.net";
/// Items per pod. A pod proves at most 10 created-array memberships (pod2's
/// `max_medium` Merkle proofs) and exposes at most 24 statements.
const DEFAULT_CHUNK: usize = 9;
/// The Bitcraft plugin's module hash on the network (rc.50, and rc.51 whose
/// change added no constraints), as `holdings classes` prints it.
const BITCRAFT_MODULE_HASH: &str =
    "0x54548dac8ef19568c4ccbbb7912c18c6a70941eba3953ce5c95acbbd3eade36e";

fn usage() -> ! {
    eprintln!(
        "usage:
  holdings classes --pexe BITCRAFT.pexe
  holdings prove --player NAME --out PROOF.json [--chunk N] [--mock] ITEM.dobj...
  holdings verify --pexe BITCRAFT.pexe [--sync URL | --offline] PROOF.json
  holdings onchain-pod --out POD.json ITEM.dobj
  holdings bounty-pod --sync URL [--block N] --recipient 0xADDRESS --threshold BIT --out POD.json ITEM.dobj...
  holdings bounty-info --threshold BIT [--recipient 0xADDRESS]"
    );
    std::process::exit(2)
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| usage());
    let mut flags: HashMap<String, String> = HashMap::new();
    let mut switches: HashSet<String> = HashSet::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mock" | "--offline" => {
                switches.insert(arg);
            }
            "--pexe" | "--player" | "--out" | "--sync" | "--chunk" | "--block" | "--recipient"
            | "--threshold" => {
                let value = args.next().unwrap_or_else(|| usage());
                flags.insert(arg, value);
            }
            _ if arg.starts_with("--") => usage(),
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    let flag = |name: &str| flags.get(name).cloned().unwrap_or_else(|| usage());
    match command.as_str() {
        "classes" => classes(Path::new(&flag("--pexe"))),
        "prove" => {
            ensure!(!paths.is_empty(), "no item files given");
            let chunk = match flags.get("--chunk") {
                Some(n) => n.parse().context("--chunk takes a number")?,
                None => DEFAULT_CHUNK,
            };
            prove(
                &flag("--player"),
                Path::new(&flag("--out")),
                &paths,
                switches.contains("--mock"),
                chunk,
            )
        }
        "verify" => {
            ensure!(paths.len() == 1, "verify takes one proof file");
            let sync = (!switches.contains("--offline"))
                .then(|| flags.get("--sync").cloned().unwrap_or(DEFAULT_SYNC.into()));
            verify(Path::new(&flag("--pexe")), sync.as_deref(), &paths[0])
        }
        "onchain-pod" => {
            ensure!(paths.len() == 1, "onchain-pod takes one item file");
            onchain_pod(Path::new(&flag("--out")), &paths[0])
        }
        "bounty-pod" => {
            ensure!(!paths.is_empty(), "no item files given");
            let block = flags
                .get("--block")
                .map(|n| n.parse().context("--block takes a block number"))
                .transpose()?;
            let threshold = flag("--threshold").parse().context("--threshold takes BIT")?;
            bounty_pod(
                &flag("--sync"),
                block,
                &flag("--recipient"),
                threshold,
                Path::new(&flag("--out")),
                &paths,
            )
        }
        "bounty-info" => {
            let threshold = flag("--threshold").parse().context("--threshold takes BIT")?;
            bounty_info(flags.get("--recipient").map(String::as_str), threshold)
        }
        _ => usage(),
    }
}

/// An item file's object.
fn read_object(path: &Path) -> Result<Dictionary> {
    let item: ItemFile = serde_json::from_str(
        &fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?,
    )
    .with_context(|| format!("{} is not an item file", path.display()))?;
    ensure!(
        item.metadata.class.plugin_name == "bitcraft",
        "{} is not a Bitcraft item",
        path.display()
    );
    Ok(item.obj)
}

/// Proves `BountyClaim(state_header, recipient, threshold)` for the items, as a
/// pod's only public statement, against the state at `block` (the head when
/// None) of a synchronizer with the holdings witness endpoint; writes the pod
/// for pod2-onchain's `wrap`.
fn bounty_pod(
    sync: &str,
    block: Option<i64>,
    recipient: &str,
    threshold: i64,
    out: &Path,
    items: &[PathBuf],
) -> Result<()> {
    let objects = items.iter().map(|p| read_object(p)).collect::<Result<Vec<_>>>()?;
    let witness = bounty::fetch_witness(sync, block, &objects)?;
    let header = witness.header();
    let live = bounty::live_items(objects, &witness)?;
    let params = Params::default();
    let mut ctx = BuildContext::new(
        MultiPodBuilder::new(&params, &DEFAULT_VD_SET),
        vec![bounty::bounty_module()?],
    );
    let recipient_value = bounty::recipient_value(recipient)?;
    let (_, total) = bounty::prove_claim(&mut ctx, &header, &live, &recipient_value, threshold)?;
    let solution = ctx.builder.solve().map_err(|err| anyhow!(err))?;
    eprintln!("{}", solution.solution_breakdown());
    let started = Instant::now();
    let pod = solution
        .prove(&Prover {})
        .map_err(|err| anyhow!(err))?
        .output_pod()
        .clone();
    pod.pod.verify().map_err(|err| anyhow!("own proof fails: {err}"))?;
    ensure!(pod.pod.pub_statements().len() == 1, "the pod has more than one public statement");
    fs::write(out, serde_json::to_string(&pod)?)?;
    eprintln!(
        "claimed {} gems worth {total} BIT (threshold {threshold}) for {recipient} in {:.1} s, \
         against state root {} (block {}); pod in {}",
        live.len(),
        started.elapsed().as_secs_f64(),
        encode_hash_hex(&witness.state_root),
        witness.block_number,
        out.display()
    );
    Ok(())
}

/// What a bounty contract is deployed with, and checks a claim's public
/// inputs against: the `BountyClaim` predicate hash, and the threshold's
/// value (and a recipient's, if given), each packed limb 3 first into 32
/// bytes as the contract packs them.
fn bounty_info(recipient: Option<&str>, threshold: i64) -> Result<()> {
    let module = bounty::bounty_module()?;
    let claim = module
        .predicate_ref_by_name("BountyClaim")
        .expect("bounty.podlang defines BountyClaim");
    let packed = |raw: pod2::middleware::RawValue| encode_hash_hex(&Hash::from(raw));
    println!("BountyClaim predicate  {}", encode_hash_hex(&Predicate::Custom(claim).hash()));
    println!("threshold {threshold:<13}{}", packed(Value::from(threshold).raw()));
    if let Some(recipient) = recipient {
        println!("recipient {recipient}  {}", packed(bounty::recipient_value(recipient)?.raw()));
    }
    println!("gems counted:");
    for gem in bounty::gems()? {
        println!("  {:<9} {:>11} BIT  class {}", gem.name, gem.value, encode_hash_hex(&gem.class));
    }
    Ok(())
}

// ============================================================================
// The predicate
// ============================================================================

fn holdings_module() -> Result<Arc<Module>> {
    let module = load_module(HOLDINGS_PODLANG, "holdings", &Params::default(), &[])
        .map_err(|err| anyhow!("holdings.podlang does not compile: {err}"))?;
    Ok(Arc::new(module))
}

fn holds_ref(module: &Module) -> CustomPredicateRef {
    module
        .predicate_ref_by_name("Holds")
        .expect("holdings.podlang defines Holds")
}

// ============================================================================
// Item files
// ============================================================================

/// The parts of a driver `.dobj` file (`ObjectRecord`) this tool reads.
#[derive(Deserialize)]
struct ItemFile {
    metadata: ItemMetadata,
    obj: Dictionary,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemMetadata {
    class: ClassName,
    status: String,
    creation_proof: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClassName {
    name: String,
    plugin_name: String,
}

/// Proof that an object's commitment sits at `index` of the created array of
/// `header`. The driver's `CreationProof` (crate-private there), decoded from
/// the same base64 form: version byte, block number, block timestamp, block
/// hash, created root, nullifiers root, prior state history root, index, then
/// the Merkle siblings from the root down; integers little-endian i64, hashes
/// 32 bytes each.
struct CreationProof {
    header: StateHeader,
    index: i64,
    proof: MerkleProof,
}

fn decode_creation_proof(encoded: &str) -> Result<CreationProof> {
    const HASH: usize = 32;
    const I64: usize = 8;
    const FIXED: usize = 1 + 2 * I64 + 4 * HASH + I64;
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|err| anyhow!("creation proof is not base64: {err}"))?;
    ensure!(
        bytes.len() >= FIXED && (bytes.len() - FIXED).is_multiple_of(HASH),
        "creation proof has an invalid length of {} bytes",
        bytes.len()
    );
    ensure!(bytes[0] == 1, "creation proof has unknown version {}", bytes[0]);
    let i64_at = |at: usize| i64::from_le_bytes(bytes[at..at + I64].try_into().unwrap());
    let hash_at = |at: usize| db_bytes_to_hash(&bytes[at..at + HASH]);
    let mut at = 1;
    let block_number = i64_at(at);
    at += I64;
    let block_timestamp = i64_at(at);
    at += I64;
    let mut hashes = Vec::new();
    for _ in 0..4 {
        hashes.push(hash_at(at)?);
        at += HASH;
    }
    let index = i64_at(at);
    at += I64;
    let siblings = bytes[at..]
        .chunks(HASH)
        .map(db_bytes_to_hash)
        .collect::<Result<Vec<_>>>()?;
    let proof: MerkleProof = serde_json::from_value(serde_json::json!({
        "existence": true,
        "siblings": siblings,
        "other_leaf": null,
    }))?;
    Ok(CreationProof {
        header: StateHeader::new(
            block_number,
            block_timestamp,
            hashes[0],
            hashes[1],
            hashes[2],
            hashes[3],
        ),
        index,
        proof,
    })
}

fn read_item(path: &Path) -> Result<(ItemFile, CreationProof)> {
    let item: ItemFile = serde_json::from_str(
        &fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?,
    )
    .with_context(|| format!("{} is not an item file", path.display()))?;
    ensure!(
        item.metadata.class.plugin_name == "bitcraft",
        "{} is not a Bitcraft item",
        path.display()
    );
    let encoded = item.metadata.creation_proof.as_deref().with_context(|| {
        format!(
            "{} has no creation proof yet (its transaction has not landed)",
            path.display()
        )
    })?;
    let creation = decode_creation_proof(encoded)?;
    // Checked here for a clear message; the circuit checks it again.
    if std::env::var_os("HOLDINGS_SKIP_PRECHECK").is_none() {
        let index = usize::try_from(creation.index)?;
        Array::verify(
            creation.header.created_root,
            &creation.proof,
            index,
            &Value::from(item.obj.commitment()),
        )
        .map_err(|err| {
            anyhow!(
                "{}: its creation proof does not place it in the created set: {err}",
                path.display()
            )
        })?;
    }
    Ok((item, creation))
}

// ============================================================================
// prove
// ============================================================================

/// What the verifier needs beside the pod to look a claim up: the block whose
/// state root the claim is grounded in. Not trusted: the verifier checks the
/// root the pod states against the synchronizer's root at that block.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RootHint {
    state_root: String,
    block_number: i64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProofFile {
    version: u32,
    /// Display only; the binding is each pod's public `Equal(player, player)`.
    player: String,
    roots: Vec<RootHint>,
    /// One MainPod per chunk of items: a pod exposes at most
    /// `max_public_statements` (24) statements and proves at most
    /// `max_medium` (10) created-array memberships.
    pods: Vec<MainPod>,
}

/// Proves `Holds` for one item and returns the public statement.
fn prove_holds(ctx: &mut BuildContext, obj: &Dictionary, creation: &CreationProof) -> Result<Statement> {
    let header = creation.header.array();
    let st_created = ctx.builder.priv_op(Operation(
        OperationType::Native(NativeOperation::ArrayContainsFromEntries),
        vec![
            Value::from(creation.header.created_root).into(),
            Value::from(creation.index).into(),
            Value::from(obj.clone()).into(),
        ],
        OperationAux::MerkleProof(creation.proof.clone()),
    ))?;
    // Bind the created root to slot `created` of the state header record.
    let st_created = ctx.builder.priv_op(Operation::replace_value_with_entry(
        vec![
            Some(entry(&header, STATE_HEADER_CREATED_SLOT as i64)?),
            None,
            None,
        ],
        st_created,
    ))?;

    let field = |name: &str| -> Result<Value> {
        obj.get(&StrKey::from(name))?
            .with_context(|| format!("the item has no {name} field"))
    };
    let type_value = field("type")?;
    let st_type = ctx
        .builder
        .priv_op(op!(DictContains(obj, "type", type_value)))?;

    let key_hash = hash_values(&[Value::from(obj.commitment()), field("key")?]);
    let st_key_hash = ctx
        .builder
        .priv_op(op!(Hash(obj, entry(obj, "key")?, key_hash)))?;
    let nullifier = hash_values(&[Value::from(key_hash), Value::from(NULLIFIER_VERSION)]);
    ensure!(
        nullifier == txlib::object_nullifier_hash(obj)?,
        "nullifier differs from txlib's"
    );
    let st_nullifier = ctx
        .builder
        .priv_op(op!(Hash(key_hash, NULLIFIER_VERSION, nullifier)))?;

    let tag = hash_values(&[field("stable_identifier")?, Value::from(TAG_VERSION)]);
    let st_tag = ctx.builder.priv_op(op!(Hash(
        entry(obj, "stable_identifier")?,
        TAG_VERSION,
        tag
    )))?;

    ctx.apply_custom_pred_simple(
        true,
        "Holds",
        vec![st_created, st_type, st_key_hash, st_nullifier, st_tag],
    )
}

fn prove(player: &str, out: &Path, items: &[PathBuf], mock: bool, chunk: usize) -> Result<()> {
    ensure!(chunk >= 1, "--chunk must be at least 1");
    let params = Params::default();
    let (vd_set, prover): (VDSet, Box<dyn MainPodProver>) = if mock {
        (VDSet::new(&[]), Box::new(MockProver {}))
    } else {
        (DEFAULT_VD_SET.clone(), Box::new(Prover {}))
    };
    let module = holdings_module()?;

    let mut roots: BTreeMap<String, i64> = BTreeMap::new();
    let mut pods = Vec::new();
    let all = Instant::now();
    for (number, paths) in items.chunks(chunk).enumerate() {
        let mut ctx = BuildContext::new(MultiPodBuilder::new(&params, &vd_set), vec![module.clone()]);
        for path in paths {
            let (item, creation) = read_item(path)?;
            if item.metadata.status != "live" {
                eprintln!(
                    "note: {} is {} in its file; proving it anyway",
                    path.display(),
                    item.metadata.status
                );
            }
            prove_holds(&mut ctx, &item.obj, &creation)
                .with_context(|| format!("proving {}", path.display()))?;
            roots.insert(
                encode_hash_hex(&creation.header.hash()),
                creation.header.block_number,
            );
            eprintln!(
                "claim  {:<10} {}",
                item.metadata.class.name,
                path.file_name().unwrap_or_default().to_string_lossy()
            );
        }
        ctx.builder
            .pub_op(op!(Equal(Value::from(player), Value::from(player))))?;

        let solution = ctx.builder.solve().map_err(|err| anyhow!(err))?;
        if number == 0 {
            eprintln!("{}", solution.solution_breakdown());
        }
        let started = Instant::now();
        let pod = solution
            .prove(prover.as_ref())
            .map_err(|err| anyhow!(err))?
            .output_pod()
            .clone();
        pod.pod.verify().map_err(|err| anyhow!("own proof fails: {err}"))?;
        eprintln!(
            "pod {} of {}: {} item(s) in {:.1} s",
            number + 1,
            items.len().div_ceil(chunk),
            paths.len(),
            started.elapsed().as_secs_f64()
        );
        pods.push(pod);
    }
    let proving = all.elapsed().as_secs_f64();

    let file = ProofFile {
        version: FILE_VERSION,
        player: player.to_string(),
        roots: roots
            .into_iter()
            .map(|(state_root, block_number)| RootHint {
                state_root,
                block_number,
            })
            .collect(),
        pods,
    };
    let json = serde_json::to_string(&file)?;
    fs::write(out, &json)?;
    eprintln!(
        "proved {} item(s) in {} pod(s) in {proving:.1} s{}; {} bytes in {}",
        items.len(),
        file.pods.len(),
        if mock { " (mock)" } else { "" },
        json.len(),
        out.display()
    );
    Ok(())
}

/// One item's `Holds` as the only public statement of a MainPod, written as
/// the MainPod's JSON. That is the input of pod2-onchain's `wrap`, which turns
/// it into a proof a contract can check: a contract reads a pod's one public
/// statement, so this pod names no player.
fn onchain_pod(out: &Path, item: &Path) -> Result<()> {
    let params = Params::default();
    let mut ctx = BuildContext::new(
        MultiPodBuilder::new(&params, &DEFAULT_VD_SET),
        vec![holdings_module()?],
    );
    let (record, creation) = read_item(item)?;
    prove_holds(&mut ctx, &record.obj, &creation)?;
    let solution = ctx.builder.solve().map_err(|err| anyhow!(err))?;
    let started = Instant::now();
    let pod = solution
        .prove(&Prover {})
        .map_err(|err| anyhow!(err))?
        .output_pod()
        .clone();
    pod.pod.verify().map_err(|err| anyhow!("own proof fails: {err}"))?;
    ensure!(
        pod.pod.pub_statements().len() == 1,
        "the pod has {} public statements",
        pod.pod.pub_statements().len()
    );
    fs::write(out, serde_json::to_string(&pod)?)?;
    eprintln!(
        "proved one {} in {:.1} s against state root {} (block {}); pod in {}",
        record.metadata.class.name,
        started.elapsed().as_secs_f64(),
        encode_hash_hex(&creation.header.hash()),
        creation.header.block_number,
        out.display()
    );
    Ok(())
}

// ============================================================================
// classes
// ============================================================================

struct Plugin {
    module_hash: String,
    /// `Is<Class>` predicate hash -> class name
    classes: HashMap<Hash, String>,
}

fn load_plugin(pexe_path: &Path) -> Result<Plugin> {
    let bytes = pexe::read_pexe_file(pexe_path)?;
    let (manifest, script) = pexe::unpack(&bytes)?;
    let module = sdk::Sdk::default()
        .load_module_from_src_manifest(&script, &manifest)
        .map_err(|err| anyhow!("compiling {}: {err}", pexe_path.display()))?;
    let mut classes = HashMap::new();
    for class in module.classes() {
        let hash = module
            .class_hash(&class.name)
            .with_context(|| format!("class {} has no hash", class.name))?;
        classes.insert(hash, class.name.clone());
    }
    Ok(Plugin {
        module_hash: format!("{:#}", module.module().batch.id()),
        classes,
    })
}

fn classes(pexe_path: &Path) -> Result<()> {
    let plugin = load_plugin(pexe_path)?;
    println!("module {}", plugin.module_hash);
    let module = holdings_module()?;
    let holds = Predicate::Custom(holds_ref(&module)).hash();
    println!("Holds predicate          {}", encode_hash_hex(&holds));
    let mut rows: Vec<_> = plugin.classes.iter().collect();
    rows.sort_by(|a, b| a.1.cmp(b.1));
    for (hash, name) in rows {
        println!("{name:<24} {}", encode_hash_hex(hash));
    }
    Ok(())
}

// ============================================================================
// verify
// ============================================================================

struct Claim {
    state_root: Hash,
    class: Hash,
    nullifier: Hash,
    tag: Hash,
}

fn literal(arg: &ValueRef) -> Result<Value> {
    match arg {
        ValueRef::Literal(value) => Ok(value.clone()),
        ValueRef::Key(key) => bail!("a claim argument is an entry, not a value: {key:?}"),
    }
}

/// The value of one item of `class`, if it has one fixed value.
fn class_value(values: &serde_json::Value, class: &str) -> Option<f64> {
    let varies = ["LEVELED_TOTAL_BIT_VALUES", "BUCKET_CONTENT_BIT_VALUES", "FULL_DURABILITY"]
        .iter()
        .any(|table| values[table].get(class).is_some());
    if varies {
        return None;
    }
    values["TOTAL_BIT_VALUES"][class].as_f64().or(Some(0.0))
}

fn verify(pexe_path: &Path, sync: Option<&str>, path: &Path) -> Result<()> {
    let file: ProofFile = serde_json::from_str(&fs::read_to_string(path)?)
        .with_context(|| format!("{} is not a holdings proof", path.display()))?;
    ensure!(file.version == FILE_VERSION, "unknown proof file version {}", file.version);
    ensure!(!file.pods.is_empty(), "the proof has no pods");
    let module = holdings_module()?;
    let holds = holds_ref(&module);
    let mut players: HashSet<String> = HashSet::new();
    let mut claims = Vec::new();
    let started = Instant::now();
    for main_pod in &file.pods {
        let pod = &main_pod.pod;

        // A real proof, over the network's circuits.
        ensure!(!pod.is_mock(), "the proof is a mock pod: it proves nothing");
        ensure!(
            pod.pod_type().0 == PodType::Main as usize,
            "the proof is a {} pod, not a MainPod",
            pod.pod_type().1
        );
        ensure!(*pod.params() == Params::default(), "the proof uses other circuit parameters");
        ensure!(
            pod.vd_set().root() == DEFAULT_VD_SET.root(),
            "the proof's verifier data set is not pod2's default"
        );
        pod.verify()
            .map_err(|err| anyhow!("the proof does not verify: {err}"))?;

        // Its public statements: our Holds claims and one player.
        let mut player: Option<String> = None;
        for statement in pod.pub_statements() {
            match &statement {
                Statement::None => {}
                Statement::Custom(cpr, args)
                    if cpr.batch.id() == holds.batch.id() && cpr.index == holds.index =>
                {
                    ensure!(args.len() == 4, "a Holds claim has {} arguments", args.len());
                    let hash = |i: usize| literal(&args[i]).map(|value| Hash::from(value.raw()));
                    claims.push(Claim {
                        state_root: hash(0)?,
                        class: hash(1)?,
                        nullifier: hash(2)?,
                        tag: hash(3)?,
                    });
                }
                Statement::Equal(ValueRef::Literal(a), ValueRef::Literal(b))
                    if a == b && a.as_str().is_some() =>
                {
                    ensure!(player.is_none(), "a pod names more than one player");
                    player = Some(a.as_str().unwrap().to_string());
                }
                other => bail!("unexpected public statement: {other}"),
            }
        }
        players.insert(player.context("a pod names no player")?);
    }
    let verifying = started.elapsed().as_secs_f64();
    ensure!(players.len() == 1, "the pods name different players");
    let player = players.into_iter().next().unwrap();
    ensure!(!claims.is_empty(), "the proof claims no items");

    // Which class each claim is, from the plugin itself.
    let plugin = load_plugin(pexe_path)?;
    ensure!(
        plugin.module_hash == BITCRAFT_MODULE_HASH,
        "{} is not the network's Bitcraft plugin (module {})",
        pexe_path.display(),
        plugin.module_hash
    );
    let values: serde_json::Value = serde_json::from_str(INVENTORY_VALUES)?;

    // Each object once.
    let mut seen_nullifiers = HashSet::new();
    let mut seen_tags = HashSet::new();
    for claim in &claims {
        ensure!(seen_nullifiers.insert(claim.nullifier), "an item is claimed twice");
        ensure!(seen_tags.insert(claim.tag), "an object is claimed twice");
    }

    // The network's view: real state roots, and nothing spent.
    let mut spent: HashSet<Hash> = HashSet::new();
    let mut head = None;
    if let Some(sync) = sync {
        let client = reqwest::blocking::Client::new();
        let hints: HashMap<&str, i64> = file
            .roots
            .iter()
            .map(|hint| (hint.state_root.as_str(), hint.block_number))
            .collect();
        let roots: HashSet<Hash> = claims.iter().map(|claim| claim.state_root).collect();
        for root in roots {
            let hex = encode_hash_hex(&root);
            let block = *hints
                .get(hex.as_str())
                .with_context(|| format!("no block given for state root {hex}"))?;
            let url = format!("{}/v1/state/by_el/{block}", sync.trim_end_matches('/'));
            let state: StateHeadResponse = client
                .get(&url)
                .send()?
                .error_for_status()
                .with_context(|| format!("no state at block {block}"))?
                .json()?;
            ensure!(
                state.state_root == root,
                "state root {hex} is not the network's at block {block}"
            );
        }
        let url = format!("{}/v1/state/membership", sync.trim_end_matches('/'));
        let request = MembershipRequest {
            object_commitments: vec![],
            nullifiers: claims.iter().map(|claim| claim.nullifier).collect(),
        };
        let response: MembershipResponse = client
            .post(&url)
            .json(&request)
            .send()?
            .error_for_status()?
            .json()?;
        for entry in response.nullifier_results {
            if entry.present {
                spent.insert(entry.nullifier);
            }
        }
        head = Some(response.block_number);
    }

    println!("player   {player}");
    println!(
        "proof    valid: {} real MainPod(s), verified in {verifying:.2} s",
        file.pods.len()
    );
    match head {
        Some(block) => println!("network  checked at block {block}"),
        None => println!("network  not checked (--offline): roots and spends unverified"),
    }
    let mut total = 0.0;
    for claim in &claims {
        let name = plugin
            .classes
            .get(&claim.class)
            .cloned()
            .unwrap_or_else(|| format!("unknown type {}", encode_hash_hex(&claim.class)));
        let value = class_value(&values, &name);
        let status = if spent.contains(&claim.nullifier) {
            "SPENT, not counted".to_string()
        } else if value.is_none() {
            "value depends on fields Holds does not prove; not counted".to_string()
        } else if head.is_some() {
            "live".to_string()
        } else {
            "unchecked".to_string()
        };
        let counted = !spent.contains(&claim.nullifier) && value.is_some();
        println!(
            "  {name:<12} {:>12} BIT  tag {}  {status}",
            value.map(|v| format!("{v:.0}")).unwrap_or("-".into()),
            &encode_hash_hex(&claim.tag)[..12],
        );
        if counted {
            total += value.unwrap();
        }
    }
    println!("total    {total:.0} BIT");
    Ok(())
}
