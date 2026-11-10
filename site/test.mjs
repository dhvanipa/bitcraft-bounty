// node --test site/test.mjs  (Node 22+). Checks chain.js against the real claim in
// onchain/test/fixtures and the constants BountyEscrowProof.t.sol pins.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import * as chain from "./chain.js";
import config from "./config.js";

const here = new URL(".", import.meta.url);
const fixture = JSON.parse(readFileSync(new URL("../onchain/test/fixtures/bounty_calldata.json", here)));

test("reads the real claim as BountyEscrowProof.t.sol does", () => {
  const c = chain.readClaim(chain.parseClaim(fixture));
  assert.equal(c.predicate, "0xaae74e677f89c82dc523ce1850a26d075d457f85bedbdf35005f3918526a772a");
  assert.equal(c.root, "0x5c6b1fe2caf14ed7746c657c67a3844d99bbbd94472faf445ffeb0521874f706");
  assert.equal(c.threshold, "0xffffffff0000000000000000000000000000000000000000000000000ed2deb8");
  assert.equal(chain.formatBit(c.thresholdCenti), "2,487,006");
  assert.equal(c.recipient, "0xa1b2c3d4e5f60718293a4b5c6d7e8f9012345678");
  assert.ok(c.canonical);
  assert.ok(c.unusedZero);
});

test("claim calldata is the selector and 36 static words", () => {
  const claim = chain.parseClaim(JSON.stringify(fixture));
  const data = chain.claimCalldata(claim);
  assert.equal(data.slice(0, 10), "0xaf867c6c");
  assert.equal(data.length, 10 + 36 * 64);
  const words = chain.words("0x" + data.slice(10));
  assert.deepEqual(words, [...claim.proof, ...claim.commitments, ...claim.commitmentPok, ...claim.input]);
});

test("the planned threshold is 50,000,000 BIT", () => {
  assert.equal(chain.intOf(config.planned.threshold), 5_000_000_000n);
  assert.equal(chain.formatBit(chain.intOf(config.planned.threshold)), "50,000,000");
  // not an Int: the mock test's threshold has no Int tag
  assert.equal(chain.intOf("0x0000000000000005000000000000000000000000000000000000000002faf080"), null);
});

test("a recipient round-trips through its 40-bit limbs", () => {
  const a = "0xfa9d2103fe14b4a64f90cf2272fc079a6a44bcbc";
  assert.equal(chain.recipientOf(chain.recipientLimbs(a)), a);
  assert.equal(chain.recipientOf([1n << 40n, 0n, 0n, 0n]), null);
});

test("pack and limbs are inverses, limb 3 first", () => {
  const root = "0x5c6b1fe2caf14ed7746c657c67a3844d99bbbd94472faf445ffeb0521874f706";
  assert.equal(chain.pack(chain.limbs(root)), root);
  assert.equal(chain.limbs(root)[3], 0x5c6b1fe2caf14ed7n);
});

test("revert reasons", () => {
  // cast abi-encode 'f(string)' 'state root not posted', after Error(string)'s selector
  const error = "0x08c379a0"
    + "0000000000000000000000000000000000000000000000000000000000000020"
    + "0000000000000000000000000000000000000000000000000000000000000015"
    + "737461746520726f6f74206e6f7420706f737465640000000000000000000000";
  assert.equal(chain.revertReason(error), "state root not posted");
  assert.match(chain.revertReason("0x7fcdd1f4"), /ProofInvalid/);
});

test("a bad claim file says what is wrong", () => {
  assert.throws(() => chain.parseClaim("{"), /isn't JSON/);
  assert.throws(() => chain.parseClaim({ ...fixture, input: fixture.input.slice(1) }), /"input" should be a list of 24/);
  assert.throws(() => chain.parseClaim({ ...fixture, proof: ["x", ...fixture.proof.slice(1)] }), /isn't a uint256/);
});

test("the page's setup sums are the repository's", () => {
  const sums = Object.fromEntries(readFileSync(new URL("../onchain/groth16-setup.sha256", here), "utf8")
    .trim().split("\n").map((line) => line.split(/\s+/).reverse()));
  assert.deepEqual(sums, config.setup.files);
});

test("formatting", () => {
  assert.equal(chain.formatUnits(100_000_000n, 6), "100");
  assert.equal(chain.formatUnits(1_500_000n, 6), "1.5");
  assert.equal(chain.formatBit(333n), "3.33");
  assert.equal(chain.group(12098545), "12,098,545");
  assert.equal(chain.toText("0x" + chain.word(32) + chain.word(4) + "55534443" + "0".repeat(56)), "USDC");
});
