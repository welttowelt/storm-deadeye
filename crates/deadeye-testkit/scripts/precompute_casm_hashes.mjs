#!/usr/bin/env node
// Precompute compiled (CASM) class hashes for every contract in the
// the-situation target/dev directory. starknet-rs 0.16's
// CompiledClass::class_hash() disagrees with the chain for Cairo 2.14
// artifacts, so we generate a side-car JSON the Rust fixture consumes.
//
// Usage:
//   node precompute_casm_hashes.mjs <target-dev-dir> <output-json>

import { readFileSync, writeFileSync, readdirSync } from "node:fs";
import { join, basename } from "node:path";
import { hash } from "starknet";

const [, , dir, out] = process.argv;
if (!dir || !out) {
  console.error("usage: precompute_casm_hashes.mjs <target-dev-dir> <output-json>");
  process.exit(2);
}

const result = {};
const files = readdirSync(dir).filter((f) => f.endsWith(".compiled_contract_class.json"));
for (const file of files) {
  const base = basename(file, ".compiled_contract_class.json");
  try {
    const casm = JSON.parse(readFileSync(join(dir, file), "utf-8"));
    const h = hash.computeCompiledClassHash(casm);
    result[base] = h;
  } catch (e) {
    console.error(`skip ${base}: ${e.message}`);
  }
}
writeFileSync(out, JSON.stringify(result, null, 2));
console.log(`wrote ${Object.keys(result).length} hashes to ${out}`);
