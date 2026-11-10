// The bounty page: reads BountyEscrow from a public Sepolia node, checks the
// newest root against the synchronizers, fills in the claim commands, and
// checks (and, from a wallet, sends) a claim. No build step, no libraries.
import config from "./config.js";
import * as chain from "./chain.js";

const params = new URLSearchParams(location.search);
const BOUNTY = (params.get("bounty") || config.bounty || "").toLowerCase() || null;
const RPCS = params.get("rpc") ? [params.get("rpc")] : config.rpcs;
const DAY = 24 * 3600;
// 24 hours of Sepolia blocks (12 s) plus a margin
const LOG_WINDOW = 7500;
const REFRESH_MS = 60_000;

const $ = (id) => document.getElementById(id);

// ---------------------------------------------------------------- JSON-RPC

class Revert extends Error {
  constructor(data, message) {
    super(message || chain.revertReason(data));
    this.data = data;
  }
}

/** One JSON-RPC call, trying each node in turn. A revert is an answer, not a failure. */
async function rpc(method, rpcParams) {
  let last;
  for (const url of RPCS) {
    try {
      const response = await fetch(url, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params: rpcParams }),
      });
      const body = await response.json();
      if (body.error) {
        const data = typeof body.error.data === "string" ? body.error.data : body.error.data?.data;
        if (data !== undefined || /revert/i.test(body.error.message)) {
          throw new Revert(data, data ? undefined : body.error.message.replace(/^execution reverted:?\s*/i, ""));
        }
        throw new Error(`${method}: ${body.error.message}`);
      }
      return body.result;
    } catch (e) {
      if (e instanceof Revert) throw e;
      last = e;
    }
  }
  throw last;
}

const call = (to, data) => rpc("eth_call", [{ to, data }, "latest"]);
const read = (selector, ...args) => call(BOUNTY, chain.calldata(selector, ...args));

// ---------------------------------------------------------------- state

let state = null; // the last read of the contract
let selectedRoot = null;

async function readBounty() {
  const [winner, deadline, threshold, predicate, poster, token, owner, verifier, head] = await Promise.all([
    read(chain.SEL.winner).then(chain.toAddress),
    read(chain.SEL.deadline).then(chain.toUint),
    read(chain.SEL.threshold).then(chain.toBytes32),
    read(chain.SEL.claimPredicate).then(chain.toBytes32),
    read(chain.SEL.poster).then(chain.toAddress),
    read(chain.SEL.token).then(chain.toAddress),
    read(chain.SEL.owner).then(chain.toAddress),
    read(chain.SEL.verifier).then(chain.toAddress),
    rpc("eth_getBlockByNumber", ["latest", false]),
  ]);
  const headNumber = Number(head.number);
  const [symbol, decimals, prize, posterBalance, logs] = await Promise.all([
    call(token, chain.SEL.symbol).then(chain.toText).catch(() => "tokens"),
    call(token, chain.SEL.decimals).then((d) => Number(chain.toUint(d))).catch(() => 0),
    call(token, chain.calldata(chain.SEL.balanceOf, BigInt(BOUNTY))).then(chain.toUint),
    rpc("eth_getBalance", [poster, "latest"]).then(BigInt),
    rpc("eth_getLogs", [{
      address: BOUNTY,
      topics: [[chain.TOPIC.RootPosted, chain.TOPIC.Claimed]],
      fromBlock: "0x" + Math.max(0, headNumber - LOG_WINDOW).toString(16),
      toBlock: "latest",
    }]),
  ]);

  // A root can be posted again; rootPostedAt holds its latest posting.
  const seen = new Set();
  const posted = logs.filter((l) => l.topics[0] === chain.TOPIC.RootPosted).map((l) => l.topics[1]).reverse()
    .filter((root) => !seen.has(root) && seen.add(root));
  const roots = (await Promise.all(posted.map(async (root) => ({
    root,
    block: Number(chain.toUint(await read(chain.SEL.rootBlock, BigInt(root)))),
    postedAt: Number(chain.toUint(await read(chain.SEL.rootPostedAt, BigInt(root)))),
  })))).filter((r) => r.postedAt > 0).sort((a, b) => b.postedAt - a.postedAt);

  const claimed = logs.find((l) => l.topics[0] === chain.TOPIC.Claimed);
  return {
    winner, deadline: Number(deadline), threshold, predicate, poster, token, owner, verifier,
    symbol, decimals, prize, posterBalance, roots, claimed,
    head: { number: headNumber, time: Number(head.timestamp) },
    readAt: Date.now() / 1000,
  };
}

/** The synchronizers' roots at a block, to compare with a posted one. */
async function checkRoot(r) {
  if (r.sync) return r.sync;
  r.sync = await Promise.all(config.synchronizers.map(async (s) => {
    try {
      const response = await fetch(`${s.url}/v1/state/by_el/${r.block}`);
      if (!response.ok) return { ...s, error: `answered ${response.status}` };
      const body = await response.json();
      return { ...s, root: "0x" + body.state_root.replace(/^0x/, ""), slot: body.slot };
    } catch {
      return { ...s, error: "did not answer" };
    }
  }));
  return r.sync;
}

// ---------------------------------------------------------------- formatting

const now = () => Date.now() / 1000;

function duration(seconds) {
  const s = Math.max(0, Math.round(seconds));
  if (s < 90) return `${s} s`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min`;
  const h = Math.floor(m / 60);
  if (h < 48) return m % 60 ? `${h} h ${m % 60} min` : `${h} h`;
  return `${Math.round(h / 24)} days`;
}

const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });
const timeFormat = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" });
const when = (t) => dateFormat.format(new Date(t * 1000));

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === undefined || v === null || v === false) continue;
    if (k === "class") node.className = v;
    else if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
    else node.setAttribute(k, v === true ? "" : v);
  }
  node.append(...children.flat().filter((c) => c !== null && c !== undefined && c !== false));
  return node;
}

const link = (href, text) => el("a", { href, rel: "noopener" }, text);
const address = (a) => link(`${config.explorer}/address/${a}`, el("code", {}, a));
const hash = (h) => el("code", { class: "hash", title: h }, h);
const muted = (text) => el("span", { class: "muted" }, text);
/** Wei as ETH, to 4 decimals. */
const eth = (wei) => chain.formatUnits((wei / 10n ** 14n) * 10n ** 14n, 18);
const prizeText = (s) => `${chain.formatUnits(s.prize, s.decimals)} ${s.symbol}`;
const thresholdBit = (s) => chain.intOf(s?.threshold ?? config.planned.threshold);

// ---------------------------------------------------------------- render

function setStatus(kind, text) {
  const status = $("status");
  status.dataset.state = kind;
  status.textContent = text;
}

function renderHero(s) {
  const centi = thresholdBit(s);
  const bit = centi === null ? "the threshold" : chain.formatBit(centi);
  document.querySelectorAll('[data-fill="threshold"]').forEach((n) => (n.textContent = bit));
  if (!s) {
    $("headline").textContent = `${config.planned.prize} for the first proof of ${bit} BIT in unspent Bitcraft items`;
    return;
  }
  const paid = s.claimed ? `${chain.formatUnits(chain.toUint(s.claimed.data), s.decimals)} ${s.symbol}` : null;
  const prize = s.prize > 0n ? prizeText(s) : "A prize";
  $("headline").textContent = paid
    ? `${paid} paid for the first proof of ${bit} BIT in unspent Bitcraft items`
    : `${prize} for the first proof of ${bit} BIT in unspent Bitcraft items`;

  const t = now();
  if (s.winner !== chain.ZERO_ADDRESS || t >= s.deadline) {
    document.querySelector(".core-note").textContent = "The bounty is over. These are the last roots posted.";
  }
  if (s.winner !== chain.ZERO_ADDRESS) {
    setStatus("done", paid ? `Claimed. ${paid} went to ${s.winner}.` : `Claimed. ${s.winner} won it.`);
  } else if (t >= s.deadline) {
    setStatus("done", `Closed on ${when(s.deadline)}, unclaimed.`);
  } else if (s.prize === 0n) {
    setStatus("waiting", `Not funded yet. Claims close on ${when(s.deadline)}.`);
  } else if (!s.roots.some((r) => r.postedAt + DAY > t)) {
    setStatus("waiting", `Open, but no root was posted in the last 24 hours, so no claim can land right now. Claims close on ${when(s.deadline)}.`);
  } else {
    setStatus("open", `Open. Claims close on ${when(s.deadline)}, in ${duration(s.deadline - t)}.`);
  }
}

function renderColumn(s) {
  const column = $("column");
  const t = now();
  const live = (s?.roots ?? []).filter((r) => r.postedAt + DAY > t);
  column.replaceChildren();
  column.classList.toggle("empty", live.length === 0);
  if (live.length === 0) {
    column.append(el("li", { class: "none" }, s ? "No root in the last 24 hours" : "No roots yet"));
    renderDetail(null);
    return;
  }
  if (!selectedRoot || !live.some((r) => r.root === selectedRoot)) selectedRoot = live[0].root;
  for (const r of live) {
    const age = t - r.postedAt;
    const band = el("li", {
      class: "band",
      style: `top:${(age / DAY) * 100}%;--age:${Math.min(1, age / DAY)}`,
    }, el("button", {
      type: "button",
      "aria-pressed": String(r.root === selectedRoot),
      "aria-label": `Root at block ${r.block}, posted ${duration(age)} ago`,
      onclick: () => selectRoot(r.root),
    }));
    column.append(band);
  }
  renderDetail(live.find((r) => r.root === selectedRoot));
}

function selectRoot(root) {
  selectedRoot = root;
  const r = state.roots.find((x) => x.root === root);
  $("block").value = r.block;
  renderColumn(state);
  renderCommands();
}

async function renderDetail(r) {
  const detail = $("root-detail");
  if (!r) {
    detail.replaceChildren(el("p", { class: "muted" }, state
      ? "The poster posts the root at Sepolia's finalized block each hour. Without a recent one, no claim can land."
      : BOUNTY ? "Reading roots…" : "The bounty isn't deployed yet. Roots will show here once its poster starts."));
    return;
  }
  const t = now();
  const left = r.postedAt + DAY - t;
  const newest = r.root === state.roots[0].root;
  const syncLine = el("p", { class: "sync" }, "Checking it against the synchronizers…");
  detail.replaceChildren(
    el("p", { class: "which" }, newest ? "Newest root" : "Older root"),
    hash(r.root),
    el("dl", {},
      el("dt", {}, "Block"), el("dd", {}, link(`${config.explorer}/block/${r.block}`, chain.group(r.block))),
      el("dt", {}, "Posted"), el("dd", {}, `${duration(t - r.postedAt)} ago`),
      el("dt", {}, "Counts until"), el("dd", {}, `${timeFormat.format(new Date((r.postedAt + DAY) * 1000))}, ${duration(left)} from now`),
    ),
    syncLine,
  );
  const results = await checkRoot(r);
  const agree = results.filter((x) => x.root === r.root);
  const differ = results.filter((x) => x.root && x.root !== r.root);
  const silent = results.filter((x) => !x.root);
  if (differ.length) {
    syncLine.className = "sync bad";
    syncLine.textContent = `${differ.map((x) => x.name).join(" and ")} ${differ.length > 1 ? "have" : "has"} a different root at this block. Don't claim at it.`;
  } else {
    syncLine.className = agree.length ? "sync ok" : "sync";
    const blockTime = agree[0]?.slot ? ` The block is from ${when(chain.slotTime(agree[0].slot))}.` : "";
    syncLine.textContent = (agree.length ? `Matches ${agree.map((x) => x.name).join(" and ")} at this block.` : "")
      + (silent.length ? ` ${silent.map((x) => x.name).join(" and ")} did not answer.` : "") + blockTime;
  }
}

function renderTerms(s) {
  const terms = $("terms");
  const centi = thresholdBit(s);
  const rows = [];
  const row = (term, ...value) => rows.push(el("dt", {}, term), el("dd", {}, ...value));
  if (!s) {
    row("Contract", "not deployed yet");
    row("Prize", `${config.planned.prize}, in Circle's USDC on Sepolia`);
    row("Threshold", `${chain.formatBit(centi)} BIT, at the app's values`);
    row("Claim predicate", hash(config.planned.claimPredicate), el("span", { class: "muted" }, "BountyClaim, as holdings bounty-info prints it"));
  } else {
    row("Contract", address(BOUNTY), muted("BountyEscrow: holds the prize and pays it to the first valid claim"));
    const prize = s.claimed ? `${chain.formatUnits(chain.toUint(s.claimed.data), s.decimals)} ${s.symbol}, paid`
      : s.prize > 0n ? prizeText(s) : `nothing yet (${s.symbol})`;
    row("Prize", prize, el("span", { class: "muted" }, "the whole balance, paid once"), address(s.token));
    row("Threshold", `${centi === null ? s.threshold : chain.formatBit(centi)} BIT`, el("span", { class: "muted" }, "at the Bitcraft app's values: machines by level, buckets by contents, axes by durability"));
    row("Deadline", when(s.deadline), el("span", { class: "muted" }, "after it the owner can take the prize back"));
    row("Claim predicate", hash(s.predicate), el("span", { class: "muted" }, "BountyClaim; its hash fixes every class's value"));
    row("Verifier", address(s.verifier),
      muted("the Groth16 verifier the contract calls to check a claim's proof; generated from the published setup"));
    row("Root poster", address(s.poster),
      muted("posts the game's state root every hour from our synchronizer. The contract trusts it: a made-up root could hold made-up items"),
      el("span", {}, `${eth(s.posterBalance)} ETH left to pay for posting`));
    row("Owner", address(s.owner),
      muted("deployed and funded the bounty; can replace the root poster, and take the prize back only after the deadline, if no one claimed it"));
    if (s.winner !== chain.ZERO_ADDRESS) {
      row("Winner", address(s.winner), s.claimed ? link(`${config.explorer}/tx/${s.claimed.transactionHash}`, "the claim") : null);
    }
  }
  terms.replaceChildren(...rows);
}

function renderLinks(s) {
  const items = [
    [`${config.repo}`, "The source", "contracts, predicates, prover and this page"],
    [`${config.repo}/blob/main/onchain/src/BountyEscrow.sol`, "BountyEscrow.sol", "what a claim must satisfy"],
    [`${config.repo}/blob/main/holdings/src/bounty.podlang`, "bounty.podlang", "the statement a claim proves"],
    [`${config.synchronizers[0].url}/sync-progress`, "Our synchronizer", "the game's synchronizer, with an endpoint that proves an item unspent at a past state"],
    ["https://archiver.don.pateldhvani.com/healthz", "Our archiver", "the network's blobs since its first slot, for running a synchronizer"],
  ];
  if (s) items.splice(1, 0, [`${config.explorer}/address/${BOUNTY}#events`, "The contract's events", "every root posted, and the claim"]);
  $("links").replaceChildren(...items.map(([href, name, what]) => el("li", {}, link(href, name), el("span", {}, what))));
}

// ---------------------------------------------------------------- commands

const RECIPIENT_KEY = "bitcraft-bounty-recipient";

function validAddress(a) {
  return /^0x[0-9a-fA-F]{40}$/.test(a) && !/^0x0{40}$/.test(a);
}

function renderCommands() {
  const recipient = $("recipient").value.trim();
  const block = $("block").value.trim();
  const centi = thresholdBit(state);
  const bit = centi === null ? "THRESHOLD" : (Number(centi) / 100).toString();
  const to = validAddress(recipient) ? recipient : "0xYOUR_ADDRESS";
  const at = /^\d+$/.test(block) ? block : "BLOCK";
  const setup = config.setup;
  const pod = `"$HOLDINGS" bounty-pod --block ${at} \\\n  --recipient ${to} --threshold ${bit} \\\n  --out claim.json ~/.bitcraft/objects/*.dobj`;
  const commands = {
    build: [
      `git clone ${config.game}`,
      `git clone ${config.repo}`,
      `git clone ${config.pod2Onchain}`,
      `HOLDINGS=$(bitcraft-leaderboard/holdings/build.sh "$PWD/digital-objects-network")`,
      `(cd pod2-onchain && cargo +nightly build --release --bin wrap && go build -o groth16 ./cli)`,
    ].join("\n"),
    setup: [
      "mkdir -p groth && cd groth",
      ...Object.keys(setup.files).map((f) => `curl -fLO ${setup.base}/${f}`),
      "shasum -a 256 -c ../bitcraft-leaderboard/onchain/groth16-setup.sha256",
      "cd ..",
    ].join("\n"),
    mock: pod.replace("bounty-pod", "bounty-pod --mock").replace("claim.json", "dry-run.json"),
    prove: pod,
    wrap: [
      "pod2-onchain/target/release/wrap claim.json plonky2-proof",
      "pod2-onchain/groth16 -p -i plonky2-proof -o groth",
    ].join("\n"),
  };
  for (const [name, text] of Object.entries(commands)) {
    const code = document.querySelector(`[data-cmd="${name}"]`);
    if (code) code.textContent = text;
  }

  const note = [];
  if (recipient && !validAddress(recipient)) note.push("That isn't an address: 0x and 40 hex digits.");
  if (state && /^\d+$/.test(block) && !state.roots.some((r) => r.block === Number(block) && r.postedAt + DAY > now())) {
    note.push("No root from the last 24 hours is at that block. Pick one in the column.");
  }
  $("field-note").textContent = note.join(" ");
}

function addCopyButtons() {
  for (const pre of document.querySelectorAll("pre")) {
    // outside the <pre>, so it stays put when a long line scrolls
    const frame = el("div", { class: "code" });
    pre.replaceWith(frame);
    const button = el("button", { type: "button", class: "copy" }, "Copy");
    frame.append(pre, button);
    button.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(pre.querySelector("code").textContent);
        button.textContent = "Copied";
      } catch {
        button.textContent = "Select and copy";
      }
      setTimeout(() => (button.textContent = "Copy"), 1600);
    });
  }
}

// ---------------------------------------------------------------- the claim checker

let checked = null; // { claim, data } once a claim would pay

async function checkClaim() {
  const list = $("checks");
  const send = $("send");
  send.hidden = true;
  checked = null;
  const item = (ok, text, ...more) => el("li", { "data-ok": ok === null ? "unknown" : String(ok) }, el("span", { class: "mark" }, ok === null ? "…" : ok ? "Yes" : "No"), el("div", {}, text, ...more));
  let claim;
  try {
    claim = chain.parseClaim($("claim-input").value.trim());
  } catch (e) {
    list.replaceChildren(item(false, e.message));
    return;
  }
  const c = chain.readClaim(claim);
  const rows = [];
  const s = state;
  const predicate = s?.predicate ?? config.planned.claimPredicate;
  const threshold = s?.threshold ?? config.planned.threshold;
  rows.push(item(c.canonical, "Every public input is a Goldilocks element."));
  rows.push(item(c.predicate === predicate, c.predicate === predicate
    ? "It is a BountyClaim with this bounty's values."
    : "It isn't this bounty's BountyClaim. Did you wrap a different pod?", el("br"), hash(c.predicate)));
  rows.push(item(c.threshold === threshold, c.threshold === threshold
    ? `It proves at least ${chain.formatBit(c.thresholdCenti)} BIT.`
    : `It proves a threshold of ${c.thresholdCenti === null ? c.threshold : chain.formatBit(c.thresholdCenti) + " BIT"}; this bounty wants ${chain.formatBit(chain.intOf(threshold))}.`));
  rows.push(item(c.unusedZero, "The unused arguments are zero."));
  rows.push(item(c.recipient !== null && c.recipient !== chain.ZERO_ADDRESS, c.recipient
    ? el("span", {}, "It pays ", el("code", {}, c.recipient), ".")
    : "The recipient's limbs are out of range."));

  if (!BOUNTY || !s) {
    rows.push(item(null, "The bounty isn't deployed yet, so the page can't ask whether the claim would pay."));
    list.replaceChildren(...rows);
    return;
  }
  const r = s.roots.find((x) => x.root === c.root);
  let postedAt = r?.postedAt;
  if (postedAt === undefined) postedAt = Number(chain.toUint(await read(chain.SEL.rootPostedAt, BigInt(c.root))));
  const t = now();
  rows.push(item(postedAt > 0 && postedAt + DAY > t,
    postedAt === 0 ? "Its state root was never posted. Prove at a block whose root is in the column."
      : postedAt + DAY > t ? `Its state root was posted ${duration(t - postedAt)} ago, and counts for ${duration(postedAt + DAY - t)} more.`
        : `Its state root was posted ${duration(t - postedAt)} ago: more than 24 hours. Prove again at a newer root.`,
    el("br"), hash(c.root)));
  const simulated = item(null, "Asking a Sepolia node whether the contract would pay…");
  rows.push(simulated);
  list.replaceChildren(...rows);

  const data = chain.claimCalldata(claim);
  try {
    await rpc("eth_call", [{ to: BOUNTY, data }, "latest"]);
    simulated.replaceWith(item(true, `The contract would pay ${prizeText(s)} to ${c.recipient} if the claim were sent now.`));
    checked = { claim, data, recipient: c.recipient };
    $("cast-send").textContent = `cast send ${BOUNTY} \\\n  ${data} \\\n  --rpc-url ${RPCS[0]} --account YOUR_ACCOUNT`;
    $("send-status").textContent = "";
    send.hidden = false;
  } catch (e) {
    simulated.replaceWith(item(false, e instanceof Revert ? `The contract would refuse it: ${e.message}.` : `Couldn't ask a node: ${e.message}`));
  }
}

async function sendClaim() {
  const out = $("send-status");
  const ethereum = window.ethereum;
  if (!checked) return;
  if (!ethereum) {
    out.textContent = "No browser wallet found. Send it with Foundry instead.";
    return;
  }
  try {
    out.textContent = "Asking your wallet…";
    const [from] = await ethereum.request({ method: "eth_requestAccounts" });
    const chainId = "0x" + config.chainId.toString(16);
    if ((await ethereum.request({ method: "eth_chainId" })) !== chainId) {
      await ethereum.request({ method: "wallet_switchEthereumChain", params: [{ chainId }] });
    }
    const tx = await ethereum.request({ method: "eth_sendTransaction", params: [{ from, to: BOUNTY, data: checked.data }] });
    out.replaceChildren("Sent. ", link(`${config.explorer}/tx/${tx}`, "See the transaction"), ". This page updates once it is mined.");
    waitFor(tx, out);
  } catch (e) {
    out.textContent = e?.code === 4001 ? "You cancelled it in your wallet." : `It didn't send: ${e?.message ?? e}`;
  }
}

async function waitFor(tx, out) {
  for (let i = 0; i < 60; i++) {
    await new Promise((resolve) => setTimeout(resolve, 5000));
    const receipt = await rpc("eth_getTransactionReceipt", [tx]).catch(() => null);
    if (!receipt) continue;
    out.replaceChildren(receipt.status === "0x1" ? "Paid. " : "The transaction reverted. ", link(`${config.explorer}/tx/${tx}`, "See it on Etherscan"), ".");
    refresh();
    return;
  }
}

// ---------------------------------------------------------------- start

async function refresh() {
  if (!BOUNTY) {
    setStatus("waiting", "Not deployed yet. This page shows the live bounty once it is.");
    return;
  }
  try {
    state = await readBounty();
    renderHero(state);
    renderColumn(state);
    renderTerms(state);
    renderLinks(state);
    if (!$("block").value && state.roots[0]) $("block").value = state.roots[0].block;
    renderCommands();
  } catch (e) {
    if (!state) setStatus("error", `Couldn't read the contract: ${e.message}. Retrying in a minute.`);
  }
}

function start() {
  for (const node of document.querySelectorAll("[data-href]")) node.href = config[node.dataset.href];
  renderHero(null);
  renderColumn(null);
  renderTerms(null);
  renderLinks(null);
  try {
    $("recipient").value = localStorage.getItem(RECIPIENT_KEY) ?? "";
  } catch {}
  $("recipient").addEventListener("input", () => {
    try {
      localStorage.setItem(RECIPIENT_KEY, $("recipient").value.trim());
    } catch {}
    renderCommands();
  });
  $("block").addEventListener("input", renderCommands);
  renderCommands();
  addCopyButtons();

  $("check-button").addEventListener("click", checkClaim);
  $("send-button").addEventListener("click", sendClaim);
  $("claim-file").addEventListener("change", async (e) => {
    const file = e.target.files[0];
    if (!file) return;
    $("claim-input").value = await file.text();
    checkClaim();
  });
  const drop = $("drop");
  drop.addEventListener("dragover", (e) => {
    e.preventDefault();
    drop.classList.add("over");
  });
  drop.addEventListener("dragleave", () => drop.classList.remove("over"));
  drop.addEventListener("drop", async (e) => {
    e.preventDefault();
    drop.classList.remove("over");
    const file = e.dataTransfer.files[0];
    if (!file) return;
    $("claim-input").value = await file.text();
    checkClaim();
  });

  refresh();
  setInterval(refresh, REFRESH_MS);
  // the ages in the column and the detail move between reads
  setInterval(() => state && (renderColumn(state), renderHero(state)), 15_000);
}

start();
