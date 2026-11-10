// What the page reads. Set `bounty` to the BountyEscrow's address once it is
// deployed; until then the page shows the planned terms. For testing,
// `?bounty=0x…&rpc=http://127.0.0.1:8545` overrides both.
export default {
  bounty: null,
  chainId: 11155111,
  // Tried in order. Both send CORS headers and return 24 hours of logs in one
  // request (publicnode up to about 8,000 blocks; checked 2026-11-10).
  rpcs: ["https://ethereum-sepolia-rpc.publicnode.com", "https://sepolia.gateway.tenderly.co"],
  // The newest root is checked against both; the poster checks them too.
  synchronizers: [
    { name: "ours", url: "https://synchronizer.don.pateldhvani.com" },
    { name: "the game's hosted one", url: "https://synchronizer.digiobjects.net" },
  ],
  explorer: "https://sepolia.etherscan.io",
  repo: "https://github.com/dhvanipa/bitcraft-leaderboard",
  game: "https://github.com/0xPARC/digital-objects-network",
  pod2Onchain: "https://github.com/dhvanipa/pod2-onchain",
  // The Groth16 setup src/Verifier.sol was generated from. A claim needs its
  // proving key. Sums: onchain/groth16-setup.sha256 (test.mjs checks they match).
  setup: {
    base: "https://github.com/dhvanipa/bitcraft-leaderboard/releases/download/groth16-setup-1",
    files: {
      r1cs: "3b72086e3bd3de899ab1fb3a9c323d0b10cbe1eb2e9f0d836b598d832c2ad4fc",
      "proving.key": "d094f61ec9708d8f462520342e113d031fa21d7314bdc60ca20d6f481c9c5c7b",
      "verifying.key": "7696c7ee75fc3e8e225df7d171c241a19dcd5835dc1e2853ef9567fe94288a92",
    },
  },
  // The terms the bounty is to be deployed with (onchain/README.md), shown
  // until `bounty` is set.
  planned: {
    claimPredicate: "0xaae74e677f89c82dc523ce1850a26d075d457f85bedbdf35005f3918526a772a",
    threshold: "0xffffffff0000000000000000000000000000000000000001000000002a05f200",
    token: "0x1c7d4b196cb0c7b01d743fbc6116a902379c7238",
    prize: "100 USDC",
  },
};
