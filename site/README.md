# site: the bounty page

A static page for the bounty: whether it is open, the roots a claim can use,
how to claim, and a checker that reads a claim as the contract does before it
is sent. Plain HTML, CSS and JavaScript modules, with no build step and no
dependencies, so it can be served from anywhere (GitHub Pages, Caddy, a
bucket).

| File | What |
|---|---|
| `index.html`, `style.css` | the page |
| `app.js` | reads the contract, renders, checks and sends a claim |
| `chain.js` | pure helpers: ABI words, pod2's 4-limb values, the claim's calldata, revert reasons |
| `config.js` | the bounty's address, the RPCs, the synchronizers, the setup's download |
| `test.mjs` | `node --test site/test.mjs` |

## What it reads

From a public Sepolia node (`config.rpcs`, tried in order): the contract's
state (`winner`, `deadline`, `threshold`, `claimPredicate`, `poster`,
`token`), the token's balance, the poster's ETH, and the last 7,500 blocks of
`RootPosted` and `Claimed` events, about 25 hours. A root's age comes from
`rootPostedAt`, as the contract reads it. On 2026-11-10 publicnode answered
`eth_getLogs` over 8,000 blocks but refused 12,000 ("pruned history
unavailable"); Tenderly's gateway answered 30,000.

From both synchronizers (`config.synchronizers`): their state root at the
selected root's block, to show whether the posted root matches them. Both
send `Access-Control-Allow-Origin: *`.

The page reads nothing from the user except what they type or paste, sends
nothing until they press Send, and keeps only the "Pay to" address in
`localStorage`.

## Before and after deployment

With `bounty: null` in `config.js` the page shows the planned terms
(`config.planned`) and says the bounty isn't deployed. Set `bounty` to the
`BountyEscrow`'s address once it is; `?bounty=0x…` does the same for one
visit, and `?rpc=URL` points the page at another node.

## Checking a claim

The checker parses pod2-onchain's `solidity_calldata.json`, reads the 24
public inputs as `BountyEscrow.claim` does (predicate, root, recipient,
threshold, unused arguments, Goldilocks range), looks the root up with
`rootPostedAt`, and then simulates the claim with `eth_call`. A revert shows
its reason: the contract's `require` messages, or the verifier's
`ProofInvalid`, `CommitmentInvalid` or `PublicInputNotInField`. If the
simulation pays, the page offers to send the claim from a browser wallet
(EIP-1193), switching it to Sepolia, or prints the `cast send` command with
the raw calldata.

## Tested (2026-11-10)

On a local `anvil` chain with `src/Verifier.sol`, a 6-decimal test USDC and a
`BountyEscrow` at the fixture's threshold, with 20 real roots from our
synchronizer posted an hour apart:

| Case | Page |
|---|---|
| open bounty, 20 roots over 20 hours | 20 bands; the newest selected, matching both synchronizers |
| the real 19-item claim | every check passes; the simulation says it pays 100 USDC |
| the same claim with one recipient limb changed | refused: `ProofInvalid` |
| the claim against a 50,000,000 BIT bounty with no roots | wrong threshold, root never posted, and the contract's own "state root not posted" |
| the claim sent with the page's `cast send` command | paid 100 USDC; the page then shows "Claimed" |
| on Sepolia, a real 3-Emerald claim against a 2,000,000 BIT test bounty, checked on the live page and sent from a browser wallet | paid 1 USDC ([0x7829dfc2…](https://sepolia.etherscan.io/tx/0x7829dfc2fd636ace84540e8c78edc335ca92125392d4a1bd7323e8ba151e737e)) |
| no bounty configured | the planned terms and "Not deployed yet" |

`chain.js`'s calldata for the fixture equals `cast calldata` byte for byte.
The layout was checked at 375 px and 1,024 px wide, light and dark, with no
horizontal scroll.

## Serving it

The page is live at <https://dhvanipa.github.io/bitcraft-bounty/>.
[pages.yml](../.github/workflows/pages.yml) publishes `site/` as it is to
GitHub Pages on every push to `main` that changes it.

Any static server works too; the page fetches only from the RPCs and
synchronizers in `config.js`. Locally:

```bash
python3 -m http.server 8000 -d site
```
