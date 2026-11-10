// Pure helpers for reading BountyEscrow and building a claim: ABI words,
// pod2's 4-limb values and the claim's calldata. No dependencies, so the page
// needs no build step; test.mjs checks them against cast and the fixtures.

/** The Goldilocks modulus; every public input of a claim is below it. */
export const GOLDILOCKS = 0xffffffff00000001n;
/** pod2 puts this in the last limb of an Int (basetypes.rs, VALUE_INT_TAG). */
export const INT_TAG = 0xffffffff00000000n;
/** Sepolia's beacon genesis; slot `s` starts 12 s × `s` after it. */
export const SEPOLIA_GENESIS_TIME = 1655733600;

/** Function selectors (`cast sig`). */
export const SEL = {
  winner: "0xdfbf53ae",
  deadline: "0x29dcb0cf",
  threshold: "0x42cde4e8",
  claimPredicate: "0x0f1540ae",
  poster: "0x80959721",
  token: "0xfc0c546a",
  owner: "0x8da5cb5b",
  verifier: "0x2b7ac3f3",
  rootBlock: "0xd9bcabd8",
  rootPostedAt: "0xaee45f94",
  claim: "0xaf867c6c",
  balanceOf: "0x70a08231",
  decimals: "0x313ce567",
  symbol: "0x95d89b41",
};

/** Event topics (`cast keccak`). */
export const TOPIC = {
  RootPosted: "0x038b148bd2b9a30cf2e581b95d2ea8f2229eddbdc7d7c05b94092b6d2965360f",
  Claimed: "0x9b01158d4bc10c112ba32b5240cda97e49e2eb86021f03f6a0f460342ac4dfda",
};

/** The Groth16 verifier's custom errors (Verifier.sol), by selector. */
const VERIFIER_ERRORS = {
  "0x7fcdd1f4": "the proof does not verify (ProofInvalid)",
  "0xa3a93fee": "the proof's commitment does not verify (CommitmentInvalid)",
  "0xa54f8e27": "a public input is out of range (PublicInputNotInField)",
};

const hex = (s) => s.replace(/^0x/i, "").toLowerCase();

/** One 32-byte ABI word, as 64 hex digits. */
export function word(value) {
  const v = typeof value === "bigint" ? value : BigInt(value);
  if (v < 0n || v >= 1n << 256n) throw new RangeError(`not a uint256: ${value}`);
  return v.toString(16).padStart(64, "0");
}

/** Calldata for `selector` with static arguments (uints, addresses, bytes32). */
export function calldata(selector, ...args) {
  return selector + args.map(word).join("");
}

/** The ABI words of a return value, as bigints. */
export function words(data) {
  const h = hex(data);
  const out = [];
  for (let i = 0; i + 64 <= h.length; i += 64) out.push(BigInt("0x" + h.slice(i, i + 64)));
  return out;
}

export const toUint = (data) => words(data)[0] ?? 0n;
export const toAddress = (data) => "0x" + word(toUint(data)).slice(24);
export const toBytes32 = (data) => "0x" + word(toUint(data));
export const ZERO_ADDRESS = "0x" + "0".repeat(40);

/** A dynamic `string` return value, as a token's `symbol()` gives it. */
export function toText(data) {
  const h = hex(data);
  if (h.length === 64) {
    // some old tokens return bytes32
    return new TextDecoder().decode(bytes(h)).replace(/\0+$/, "");
  }
  const offset = Number(BigInt("0x" + h.slice(0, 64))) * 2;
  const length = Number(BigInt("0x" + h.slice(offset, offset + 64))) * 2;
  return new TextDecoder().decode(bytes(h.slice(offset + 64, offset + 64 + length)));
}

function bytes(h) {
  const out = new Uint8Array(h.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(h.slice(2 * i, 2 * i + 2), 16);
  return out;
}

/** A packed pod2 value's 4 Goldilocks limbs, limb 0 first. */
export function limbs(bytes32) {
  const v = BigInt("0x" + hex(bytes32));
  const m = (1n << 64n) - 1n;
  return [v & m, (v >> 64n) & m, (v >> 128n) & m, v >> 192n];
}

/** 4 limbs packed limb 3 first, as the contract and the synchronizer print them. */
export function pack([l0, l1, l2, l3]) {
  return "0x" + word((l3 << 192n) | (l2 << 128n) | (l1 << 64n) | l0);
}

/** A packed pod2 Int's value, or null if `bytes32` isn't a non-negative Int. */
export function intOf(bytes32) {
  const [lo, hi, zero, tag] = limbs(bytes32);
  if (tag !== INT_TAG || zero !== 0n || lo >= 1n << 32n || hi >= 1n << 31n) return null;
  return (hi << 32n) | lo;
}

/** The recipient from its 4 limbs of 40 bits, least significant first. */
export function recipientOf(four) {
  if (four.some((l) => l >= 1n << 40n)) return null;
  const v = four[0] | (four[1] << 40n) | (four[2] << 80n) | (four[3] << 120n);
  return "0x" + v.toString(16).padStart(40, "0");
}

/** The inverse of recipientOf, as `holdings bounty-info --recipient` packs it. */
export function recipientLimbs(address) {
  const v = BigInt("0x" + hex(address));
  const m = (1n << 40n) - 1n;
  return [0n, 40n, 80n, 120n].map((s) => (v >> s) & m);
}

const SIZES = { proof: 8, commitments: 2, commitmentPok: 2, input: 24 };

/**
 * A claim from pod2-onchain's `solidity_calldata.json` (decimal or hex
 * strings), checked for shape. Throws with a message a claimant can act on.
 */
export function parseClaim(json) {
  let data = json;
  if (typeof json === "string") {
    try {
      data = JSON.parse(json);
    } catch {
      throw new Error("That isn't JSON. Paste the whole solidity_calldata.json.");
    }
  }
  const claim = {};
  for (const [key, size] of Object.entries(SIZES)) {
    const list = data?.[key];
    if (!Array.isArray(list) || list.length !== size) {
      throw new Error(`"${key}" should be a list of ${size} numbers. Is this solidity_calldata.json from pod2-onchain?`);
    }
    claim[key] = list.map((x) => {
      try {
        const v = BigInt(x);
        word(v);
        return v;
      } catch {
        throw new Error(`"${key}" holds ${String(x).slice(0, 20)}, which isn't a uint256.`);
      }
    });
  }
  return claim;
}

/** Calldata for `claim(proof, commitments, commitmentPok, input)`: all static. */
export function claimCalldata(claim) {
  return calldata(SEL.claim, ...claim.proof, ...claim.commitments, ...claim.commitmentPok, ...claim.input);
}

/** What a claim's 24 public inputs say, read as BountyEscrow reads them. */
export function readClaim(claim) {
  const i = claim.input;
  const threshold = pack(i.slice(12, 16));
  return {
    predicate: pack(i.slice(0, 4)),
    root: pack(i.slice(4, 8)),
    recipient: recipientOf(i.slice(8, 12)),
    threshold,
    thresholdCenti: intOf(threshold),
    canonical: i.every((x) => x < GOLDILOCKS),
    unusedZero: i.slice(16).every((x) => x === 0n),
  };
}

/** Why a call reverted, from its revert data. */
export function revertReason(data) {
  const h = typeof data === "string" ? hex(data) : "";
  if (h.startsWith("08c379a0")) return toText("0x" + h.slice(8));
  if (VERIFIER_ERRORS["0x" + h.slice(0, 8)]) return VERIFIER_ERRORS["0x" + h.slice(0, 8)];
  if (h === "") return "it reverted without a reason";
  return `it reverted with 0x${h.slice(0, 8)}`;
}

/** `v` in units of 10^-decimals, without trailing zeros. */
export function formatUnits(v, decimals) {
  const d = BigInt(decimals);
  const whole = v / 10n ** d;
  const frac = (v % 10n ** d).toString().padStart(Number(d), "0").replace(/0+$/, "");
  return group(whole) + (frac ? "." + frac : "");
}

/** Digits in groups of three: 12,098,545. */
export function group(v) {
  return BigInt(v).toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

/** A centi-BIT amount in BIT. */
export const formatBit = (centi) => formatUnits(BigInt(centi), 2);

/** 0x5c6b1f…74f706 */
export function shortHash(h, n = 6) {
  return h.length > 2 * n + 4 ? `${h.slice(0, n + 2)}…${h.slice(-n)}` : h;
}

/** The start time of a beacon slot, in Unix seconds. */
export const slotTime = (slot) => SEPOLIA_GENESIS_TIME + 12 * Number(slot);
