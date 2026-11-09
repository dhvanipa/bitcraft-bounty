//! `holdings`: prove that you hold Bitcraft items without showing them, and
//! check such a proof. A prototype of a leaderboard whose scores are proofs.
//!
//! - `prove` reads item files (`.dobj`, as in `~/.dobj/objects/`) and proves,
//!   for each item live at one state, `Holds(state_root, type, tag)`
//!   (`holdings.podlang`) in pod2 MainPods, with the player's name as a public
//!   statement. Live means created and unspent at that state, both proved
//!   in-circuit from a synchronizer's witness. The item files never leave the
//!   machine: holding the file is owning the item.
//! - `verify` checks that the MainPods are real proofs over the network's
//!   circuits, that each claim is our `Holds` about the file's state root,
//!   which Bitcraft class each `type` is (by compiling the plugin), and asks
//!   the synchronizer that the state root is the network's. It prints the
//!   claims, the total BIT value and how old the state is.
//! - `classes` prints the plugin's module hash and each class's type hash.
//!
//! Values are the Bitcraft app's (`apps/bitcraft/ui/inventory-values.json`).
//! `Holds` does not prove an item's level, fill or durability, so only
//! classes with one fixed value (gems, resources) are counted.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use payload::{decode_hash_hex, encode_hash_hex};
use pod2::{
    backends::plonky2::{basetypes::DEFAULT_VD_SET, mainpod::Prover, mock::mainpod::MockProver},
    frontend::{MainPod, MultiPodBuilder},
    lang::Module,
    middleware::{
        CustomPredicateRef, Hash, MainPodProver, Params, PodType, Predicate, Statement, VDSet,
        Value, ValueRef, containers::Dictionary,
    },
};
use pod2utils::{macros::BuildContext, op};
use serde::{Deserialize, Serialize};
use txlib::StateHeader;
use wire_types::synchronizer::StateHeadResponse;

use crate::live::{
    LiveObject, context, fetch_witness, field, holdings_module, live_objects, prove_live_object,
};

mod bounty;
mod live;

/// The Bitcraft app's value table, at the game commit this is built from.
const INVENTORY_VALUES: &str = include_str!("../../../apps/bitcraft/ui/inventory-values.json");
/// 2: one state root for every claim, and no public nullifiers (2026-11-09).
const FILE_VERSION: u32 = 2;
/// Where `verify` checks state roots; it needs only `/v1/state/by_el`.
const DEFAULT_SYNC: &str = "https://synchronizer.digiobjects.net";
/// Items per MainPod: a pod exposes at most 24 public statements, and the
/// prover splits each chunk's private work across inner pods.
const DEFAULT_CHUNK: usize = 9;
/// Sepolia's beacon genesis time; slot `s` starts 12 s × `s` after it.
const SEPOLIA_GENESIS_TIME: u64 = 1_655_733_600;
/// The Bitcraft plugin's module hash on the network (rc.50, and rc.51 whose
/// change added no constraints), as `holdings classes` prints it.
const BITCRAFT_MODULE_HASH: &str =
    "0x54548dac8ef19568c4ccbbb7912c18c6a70941eba3953ce5c95acbbd3eade36e";

fn usage() -> ! {
    eprintln!(
        "usage:
  holdings classes --pexe BITCRAFT.pexe
  holdings prove --player NAME --sync URL [--block N] --out PROOF.json [--chunk N] [--mock] ITEM.dobj...
  holdings verify --pexe BITCRAFT.pexe [--sync URL | --offline] PROOF.json
  holdings onchain-pod --sync URL [--block N] --out POD.json ITEM.dobj
  holdings bounty-pod --sync URL [--block N] --recipient 0xADDRESS --threshold BIT --out POD.json ITEM.dobj...
  holdings bounty-info --threshold BIT [--recipient 0xADDRESS] [--pexe BITCRAFT.pexe]"
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
    // the state to prove against: an execution block's, or the head's
    let block: Option<i64> = flags
        .get("--block")
        .map(|n| n.parse().context("--block takes a block number"))
        .transpose()?;
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
                &flag("--sync"),
                block,
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
            onchain_pod(&flag("--sync"), block, Path::new(&flag("--out")), &paths[0])
        }
        "bounty-pod" => {
            ensure!(!paths.is_empty(), "no item files given");
            let threshold = centi_bit(&flag("--threshold"))?;
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
            let threshold = centi_bit(&flag("--threshold"))?;
            bounty_info(
                flags.get("--recipient").map(String::as_str),
                threshold,
                flags.get("--pexe").map(Path::new),
            )
        }
        _ => usage(),
    }
}

/// A BIT amount from the command line, in centi-BIT: the bounty counts in
/// 1/100 BIT so that every item's value is a whole number.
fn centi_bit(bit: &str) -> Result<i64> {
    let bit: f64 = bit.parse().context("--threshold takes BIT")?;
    let centi = (bit * 100.0).round();
    ensure!(
        bit > 0.0 && (centi - bit * 100.0).abs() < 1e-6,
        "--threshold must be a positive number of BIT with at most 2 decimals"
    );
    Ok(centi as i64)
}

/// An item file, which must be a Bitcraft item's.
fn read_item(path: &Path) -> Result<ItemFile> {
    let item: ItemFile = serde_json::from_str(
        &fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?,
    )
    .with_context(|| format!("{} is not an item file", path.display()))?;
    ensure!(
        item.metadata.class.plugin_name == "bitcraft",
        "{} is not a Bitcraft item",
        path.display()
    );
    Ok(item)
}

fn read_object(path: &Path) -> Result<Dictionary> {
    Ok(read_item(path)?.obj)
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
    let witness = fetch_witness(sync, block, &objects)?;
    let header = witness.header();
    let (live, skipped) = bounty::live_items(objects, &witness)?;
    for (i, name, why) in &skipped {
        let file = items[*i].file_name().unwrap_or_default().to_string_lossy();
        eprintln!("skip   {name:<12} {file}: {why}");
    }
    ensure!(!live.is_empty(), "no item is live and worth something at block {}", witness.block_number);
    let mut by_class: std::collections::BTreeMap<&str, (usize, i64)> = Default::default();
    for item in &live {
        let entry = by_class.entry(item.name).or_default();
        entry.0 += 1;
        entry.1 += item.valued.value();
    }
    for (name, (count, centi)) in &by_class {
        eprintln!("claim  {count:>5} {name:<15} {:>15} BIT", *centi as f64 / 100.0);
    }
    let params = Params::default();
    let mut ctx = context(MultiPodBuilder::new(&params, &DEFAULT_VD_SET), bounty::bounty_module()?)?;
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
        "claimed {} items worth {} BIT (threshold {} BIT; skipped {}) for {recipient} in {:.1} s, \
         against state root {} (block {}); pod in {}",
        live.len(),
        total as f64 / 100.0,
        threshold as f64 / 100.0,
        skipped.len(),
        started.elapsed().as_secs_f64(),
        encode_hash_hex(&witness.state_root),
        witness.block_number,
        out.display()
    );
    Ok(())
}

/// What a bounty contract is deployed with, and checks a claim's public
/// inputs against: the `BountyClaim` predicate hash, and the threshold's
/// value in centi-BIT (and a recipient's, if given), each packed limb 3 first
/// into 32 bytes as the contract packs them; then each class's value rule.
/// With a plugin, checks the class hashes against it.
fn bounty_info(recipient: Option<&str>, threshold: i64, pexe: Option<&Path>) -> Result<()> {
    let module = bounty::bounty_module()?;
    let claim = module
        .predicate_ref_by_name("BountyClaim")
        .expect("bounty.podlang defines BountyClaim");
    let packed = |raw: pod2::middleware::RawValue| encode_hash_hex(&Hash::from(raw));
    println!("BountyClaim predicate  {}", encode_hash_hex(&Predicate::Custom(claim).hash()));
    println!(
        "threshold {} BIT = {threshold} centi-BIT  {}",
        threshold as f64 / 100.0,
        packed(Value::from(threshold).raw())
    );
    if let Some(recipient) = recipient {
        println!("recipient {recipient}  {}", packed(bounty::recipient_value(recipient)?.raw()));
    }
    let rules = bounty::rules()?;
    println!("values, in BIT, as the Bitcraft app counts them:");
    for rule in &rules {
        let bit = |c: &i64| *c as f64 / 100.0;
        let how = match &rule.rule {
            bounty::Rule::Fixed { value: 0 } => "nothing; not counted".to_string(),
            bounty::Rule::Fixed { value } => format!("{}", bit(value)),
            bounty::Rule::Bucket { base, per_unit } => format!("{} + {} per unit (n)", bit(base), bit(per_unit)),
            bounty::Rule::Leveled { by_level } => format!(
                "by level 1 to {}: {}",
                by_level.len(),
                by_level.iter().map(|v| bit(v).to_string()).collect::<Vec<_>>().join(", ")
            ),
            bounty::Rule::Axe { full_bit } => format!("{full_bit} x durability / 100"),
        };
        println!("  {:<15} {how}", rule.name);
    }
    if let Some(pexe) = pexe {
        let plugin = load_plugin(pexe)?;
        for rule in &rules {
            ensure!(
                plugin.classes.get(&rule.class) == Some(&rule.name.to_string()),
                "{}'s hash in bounty.rs is not the plugin's",
                rule.name
            );
        }
        ensure!(plugin.classes.len() == rules.len(), "the plugin has classes bounty.rs lacks");
        println!("all {} class hashes match {}", rules.len(), pexe.display());
    }
    Ok(())
}

// ============================================================================
// The predicate
// ============================================================================


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
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClassName {
    name: String,
    plugin_name: String,
}

// ============================================================================
// prove
// ============================================================================

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProofFile {
    version: u32,
    /// Display only; the binding is each pod's public `Equal(player, player)`.
    player: String,
    /// The state every claim is about, and its execution block, where the
    /// verifier looks the root up. Not trusted: the verifier checks that each
    /// claim states this root and that it is the network's at the block.
    state_root: String,
    block_number: i64,
    /// One MainPod per chunk of items: a pod exposes at most
    /// `max_public_statements` (24) statements.
    pods: Vec<MainPod>,
}

/// Proves `Holds` for one item live at `header` and returns the public
/// statement.
fn prove_holds(ctx: &mut BuildContext, header: &StateHeader, item: &LiveObject) -> Result<Statement> {
    let obj = &item.obj;
    let (st_live, _tag) = prove_live_object(ctx, header, item)?;
    let type_value = field(obj, "type")?;
    let st_type = ctx
        .builder
        .priv_op(op!(DictContains(obj, "type", type_value)))?;
    ctx.apply_custom_pred_simple(true, "Holds", vec![st_live, st_type])
}

#[allow(clippy::too_many_arguments)]
fn prove(
    player: &str,
    sync: &str,
    block: Option<i64>,
    out: &Path,
    items: &[PathBuf],
    mock: bool,
    chunk: usize,
) -> Result<()> {
    ensure!(chunk >= 1, "--chunk must be at least 1");
    let params = Params::default();
    let (vd_set, prover): (VDSet, Box<dyn MainPodProver>) = if mock {
        (VDSet::new(&[]), Box::new(MockProver {}))
    } else {
        (DEFAULT_VD_SET.clone(), Box::new(Prover {}))
    };
    let module = holdings_module()?;

    // One state for every item: the synchronizer's proofs that each was
    // created, and is unspent, there.
    let files = items.iter().map(|p| read_item(p)).collect::<Result<Vec<_>>>()?;
    let objects: Vec<Dictionary> = files.iter().map(|f| f.obj.clone()).collect();
    let witness = fetch_witness(sync, block, &objects)?;
    let header = witness.header();
    eprintln!(
        "state  {} at block {}",
        encode_hash_hex(&witness.state_root),
        witness.block_number
    );
    let mut live = Vec::new();
    let mut skipped = 0;
    for ((path, file), item) in items.iter().zip(&files).zip(live_objects(objects, &witness)?) {
        let name = &file.metadata.class.name;
        let file_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        match item {
            Ok(item) => live.push((name.clone(), file_name, item)),
            Err(why) => {
                eprintln!("skip   {name:<10} {file_name}: {why}");
                skipped += 1;
            }
        }
    }
    ensure!(!live.is_empty(), "no item is live at block {}", witness.block_number);

    let mut pods = Vec::new();
    let all = Instant::now();
    let chunks = live.len().div_ceil(chunk);
    for (number, batch) in live.chunks(chunk).enumerate() {
        let mut ctx = context(MultiPodBuilder::new(&params, &vd_set), module.clone())?;
        for (name, file_name, item) in batch {
            prove_holds(&mut ctx, &header, item).with_context(|| format!("proving {file_name}"))?;
            eprintln!("claim  {name:<10} {file_name}");
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
            "pod {} of {chunks}: {} item(s) in {:.1} s",
            number + 1,
            batch.len(),
            started.elapsed().as_secs_f64()
        );
        pods.push(pod);
    }
    let proving = all.elapsed().as_secs_f64();

    let file = ProofFile {
        version: FILE_VERSION,
        player: player.to_string(),
        state_root: encode_hash_hex(&witness.state_root),
        block_number: witness.block_number,
        pods,
    };
    let json = serde_json::to_string(&file)?;
    fs::write(out, &json)?;
    eprintln!(
        "proved {} item(s) in {} pod(s) in {proving:.1} s{}, skipped {skipped} not live; {} bytes in {}",
        live.len(),
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
fn onchain_pod(sync: &str, block: Option<i64>, out: &Path, path: &Path) -> Result<()> {
    let record = read_item(path)?;
    let witness = fetch_witness(sync, block, std::slice::from_ref(&record.obj))?;
    let item = live_objects(vec![record.obj.clone()], &witness)?
        .pop()
        .context("no witness for the item")?
        .map_err(|why| anyhow!("{} is {why} (block {})", path.display(), witness.block_number))?;
    let params = Params::default();
    let mut ctx = context(MultiPodBuilder::new(&params, &DEFAULT_VD_SET), holdings_module()?)?;
    prove_holds(&mut ctx, &witness.header(), &item)?;
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
        "proved one {} live at state root {} (block {}) in {:.1} s; pod in {}",
        record.metadata.class.name,
        encode_hash_hex(&witness.state_root),
        witness.block_number,
        started.elapsed().as_secs_f64(),
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
    class: Hash,
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
    let state_root = decode_hash_hex(&file.state_root)
        .with_context(|| format!("{} is not a state root", file.state_root))?;
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
                    ensure!(args.len() == 3, "a Holds claim has {} arguments", args.len());
                    let hash = |i: usize| literal(&args[i]).map(|value| Hash::from(value.raw()));
                    ensure!(
                        hash(0)? == state_root,
                        "a claim is about another state than the file's {}",
                        file.state_root
                    );
                    claims.push(Claim {
                        class: hash(1)?,
                        tag: hash(2)?,
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
    let mut seen_tags = HashSet::new();
    for claim in &claims {
        ensure!(seen_tags.insert(claim.tag), "an object is claimed twice");
    }

    // The network's view: the state root is the chain's at the block. Every
    // claim proved its item created and unspent there.
    let mut slot = None;
    if let Some(sync) = sync {
        let url = format!("{}/v1/state/by_el/{}", sync.trim_end_matches('/'), file.block_number);
        let state: StateHeadResponse = reqwest::blocking::Client::new()
            .get(&url)
            .send()?
            .error_for_status()
            .with_context(|| format!("no state at block {}", file.block_number))?
            .json()?;
        ensure!(
            state.state_root == state_root,
            "state root {} is not the network's at block {}",
            file.state_root,
            file.block_number
        );
        slot = Some(state.slot);
    }

    println!("player   {player}");
    println!(
        "proof    valid: {} real MainPod(s), verified in {verifying:.2} s",
        file.pods.len()
    );
    match slot {
        Some(slot) => {
            let at = SEPOLIA_GENESIS_TIME + 12 * u64::from(slot);
            let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
            println!(
                "network  every item live at block {} (slot {slot}, {} min before this check)",
                file.block_number,
                now.saturating_sub(at) / 60
            );
        }
        None => println!("network  not checked (--offline): the state root is unverified"),
    }
    let mut total = 0.0;
    for claim in &claims {
        let name = plugin
            .classes
            .get(&claim.class)
            .cloned()
            .unwrap_or_else(|| format!("unknown type {}", encode_hash_hex(&claim.class)));
        let value = class_value(&values, &name);
        let status = if value.is_none() {
            "value depends on fields Holds does not prove; not counted"
        } else if slot.is_some() {
            "live"
        } else {
            "unchecked"
        };
        let counted = value.is_some();
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

#[cfg(test)]
mod tests {
    use pod2::middleware::StrKey;

    use super::*;
    use crate::live::{
        nullifier_of, tag_of,
        testing::{State, object},
    };

    fn emerald(key: i64) -> Dictionary {
        object(bounty::class_hash("Emerald").unwrap(), key)
    }

    /// Proves `Holds` for `items` with the mock prover, which checks every
    /// operation, and returns the pod's public statements.
    fn holds(state: &State, items: &[LiveObject]) -> Result<Vec<Statement>> {
        let params = Params::default();
        let mut ctx = context(MultiPodBuilder::new(&params, &VDSet::new(&[])), holdings_module()?)?;
        for item in items {
            prove_holds(&mut ctx, &state.header, item)?;
        }
        let pod = ctx
            .builder
            .solve()
            .map_err(|err| anyhow!(err))?
            .prove(&MockProver {})
            .map_err(|err| anyhow!(err))?
            .output_pod()
            .clone();
        pod.pod.verify().map_err(|err| anyhow!("{err}"))?;
        Ok(pod.pod.pub_statements())
    }

    fn args(statement: &Statement) -> Vec<Hash> {
        match statement {
            Statement::Custom(_, args) => args
                .iter()
                .map(|arg| Hash::from(literal(arg).unwrap().raw()))
                .collect(),
            other => panic!("not a Holds claim: {other}"),
        }
    }

    #[test]
    fn a_live_item_is_claimed_without_its_nullifier() -> Result<()> {
        let one = emerald(1);
        let two = emerald(2);
        let state = State::new(&[one.clone(), two.clone()], &[]);
        let statements = holds(&state, &[state.live(&one), state.live(&two)])?;
        assert_eq!(statements.len(), 2);
        for (statement, obj) in statements.iter().zip([&one, &two]) {
            let args = args(statement);
            let class = Hash::from(field(obj, "type")?.raw());
            assert_eq!(args, vec![state.header.hash(), class, tag_of(obj)?]);
            assert!(!args.contains(&nullifier_of(obj)?), "the nullifier is public");
        }
        Ok(())
    }

    #[test]
    fn a_spent_item_cannot_be_claimed() -> Result<()> {
        let spent = emerald(1);
        let state = State::new(std::slice::from_ref(&spent), std::slice::from_ref(&spent));
        // its nullifier is in the set: the only proof there is is of presence
        assert!(state.is_spent(&spent));
        assert!(holds(&state, &[state.live(&spent)]).is_err());
        Ok(())
    }

    #[test]
    fn an_item_not_in_the_state_cannot_be_claimed() -> Result<()> {
        let real = emerald(1);
        let state = State::new(std::slice::from_ref(&real), &[]);
        // a made-up item, given the real one's place in the created array
        let mut forged = state.live(&real);
        forged.obj = emerald(2);
        assert!(holds(&state, &[forged]).is_err());
        Ok(())
    }

    #[test]
    fn a_re_keyed_item_is_claimed_only_in_its_new_state_with_the_same_tag() -> Result<()> {
        // Rekey consumes the old state (its nullifier is recorded) and creates
        // one with a new key and the same stable identifier
        let old = emerald(1);
        let mut new = old.clone();
        new.update(&StrKey::from("key"), &Value::from(99))?;
        let state = State::new(&[old.clone(), new.clone()], std::slice::from_ref(&old));
        assert!(holds(&state, &[state.live(&old)]).is_err(), "the old key still claims it");
        let statements = holds(&state, &[state.live(&new)])?;
        assert_eq!(args(&statements[0])[2], tag_of(&old)?);
        Ok(())
    }
}
