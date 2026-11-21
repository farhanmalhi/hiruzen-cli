// Converts the Fig completion spec library (@withfig/autocomplete, MIT) into
// the archive that hiruzen embeds: one zstd-compressed JSON document per spec.
//
// Fig specs are JavaScript modules, and some values in them are functions
// (generators that post-process command output, dynamic specs). Functions
// cannot be carried into Rust, so:
//   - a generator that is a named export of its module (for example
//     gitGenerators.localBranches) is tagged {"native": "git:localBranches"}
//     so hiruzen can use its native implementation of it
//   - any other generator that needs a function is emitted as
//     {"dynamic": true, "js": [path]}, where the path locates the generator
//     inside the spec object, and the spec's JavaScript module is stored in
//     the archive as "js/<key>" so hiruzen can run the function
//   - every other function-valued key is dropped
//
// Usage: node build.mjs [output path]

import { readdirSync, readFileSync, statSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { zstdCompressSync, constants } from "node:zlib";

const here = dirname(fileURLToPath(import.meta.url));
const specRoot = join(here, "node_modules", "@withfig", "autocomplete", "build");
const outPath = resolve(process.argv[2] ?? join(here, "..", "..", "specs", "specs.bin"));

// Keys that only matter to Fig's own UI.
const droppedKeys = new Set(["icon", "cache"]);

const walk = (dir) =>
  readdirSync(dir).flatMap((entry) => {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) return walk(full);
    return entry.endsWith(".js") ? [full] : [];
  });

const specKey = (file) => {
  const rel = relative(specRoot, file).split(sep).join("/").replace(/\.js$/, "");
  return rel.endsWith("/index") ? rel.slice(0, -"/index".length) : rel;
};

const isGenerator = (value) =>
  value != null &&
  typeof value === "object" &&
  !Array.isArray(value) &&
  ("script" in value || "custom" in value || "template" in value || "postProcess" in value);

// Maps generator object identity to a stable name, from the module's named exports.
const collectNamedGenerators = (key, mod) => {
  const named = new Map();
  for (const [exportName, value] of Object.entries(mod)) {
    if (exportName === "default" || value == null || typeof value !== "object") continue;
    if (isGenerator(value)) {
      named.set(value, `${key}:${exportName}`);
      continue;
    }
    for (const [name, inner] of Object.entries(value)) {
      if (isGenerator(inner)) named.set(inner, `${key}:${name}`);
    }
  }
  return named;
};

const convertGenerator = (gen, named, path) => {
  const out = {};
  if (typeof gen.template === "string" || Array.isArray(gen.template)) out.template = gen.template;
  const scriptOk =
    typeof gen.script === "string" || (Array.isArray(gen.script) && gen.script.every((s) => typeof s === "string"));
  if (scriptOk) out.script = gen.script;
  if (typeof gen.splitOn === "string") out.splitOn = gen.splitOn;
  const nativeName = named.get(gen);
  if (nativeName) out.native = nativeName;
  const needsFunction =
    typeof gen.script === "function" ||
    typeof gen.custom === "function" ||
    typeof gen.postProcess === "function" ||
    (gen.script != null && !scriptOk);
  if (typeof gen.getQueryTerm === "string") out.getQueryTerm = gen.getQueryTerm;
  if (needsFunction) {
    out.dynamic = true;
    if (!nativeName) out.js = path;
  } else if (typeof gen.getQueryTerm === "function" && !nativeName) {
    // The command is plain, but which part of the word it completes is computed.
    out.js = path;
  }
  return out;
};

// `path` is the route from the spec root to `value` in the original object,
// which is how hiruzen finds a generator again inside the JavaScript module.
const convert = (value, named, stack, path) => {
  if (value == null) return undefined;
  if (typeof value === "function") return undefined;
  if (typeof value !== "object") return value;
  if (stack.has(value)) return undefined; // cycle
  stack.add(value);
  let out;
  if (Array.isArray(value)) {
    out = value.map((v, i) => convert(v, named, stack, [...path, i])).filter((v) => v !== undefined);
  } else {
    out = {};
    for (const [k, v] of Object.entries(value)) {
      if (droppedKeys.has(k)) continue;
      if (k === "generators") {
        const gens = (Array.isArray(v) ? v.map((g, i) => [g, [...path, k, i]]) : [[v, [...path, k]]])
          .filter(([g]) => isGenerator(g))
          .map(([g, genPath]) => convertGenerator(g, named, genPath));
        if (gens.length > 0) out.generators = gens;
        continue;
      }
      const converted = convert(v, named, stack, [...path, k]);
      if (converted !== undefined) out[k] = converted;
    }
  }
  stack.delete(value);
  return out;
};

// The spec modules are bundles with no imports that end in one export
// statement. Rewrites a module into an expression that evaluates to its
// default export, so it can be run as a plain script.
const moduleAsExpression = (source) => {
  const at = source.lastIndexOf("export{");
  if (at === -1 || /(^|[;}])import[ {*"']/.test(source)) return null;
  const end = source.indexOf("}", at);
  const bindings = source.slice(at + "export{".length, end).split(",");
  const local = bindings.map((b) => b.trim().split(/\s+as\s+/)).find(([, exported]) => exported === "default")?.[0];
  if (local == null || source.slice(end + 1).replace(/[;\s]/g, "") !== "") return null;
  return `(function(){${source.slice(0, at)}\nreturn ${local};})()`;
};

const entries = [];
const index = {};
const stats = { converted: 0, skipped: [], generators: 0, native: 0, dynamic: 0, modules: 0 };

for (const file of walk(specRoot).sort()) {
  const key = specKey(file);
  let mod;
  try {
    mod = await import(pathToFileURL(file).href);
  } catch (error) {
    stats.skipped.push(`${key}: import failed (${error.message})`);
    continue;
  }
  const spec = mod.default;
  if (spec == null || typeof spec !== "object") {
    stats.skipped.push(`${key}: ${typeof spec === "function" ? "dynamic spec" : "no default export"}`);
    continue;
  }
  if (Array.isArray(spec) || spec.name == null) {
    stats.skipped.push(`${key}: not a command spec`);
    continue;
  }
  const named = collectNamedGenerators(key, mod);
  const json = JSON.stringify(convert(spec, named, new Set(), []));
  stats.generators += (json.match(/"generators":/g) ?? []).length;
  stats.native += (json.match(/"native":/g) ?? []).length;
  stats.dynamic += (json.match(/"dynamic":true/g) ?? []).length;
  entries.push({ key, raw: Buffer.from(json) });
  if (json.includes('"js":[')) {
    const script = moduleAsExpression(readFileSync(file, "utf8"));
    if (script == null) {
      stats.skipped.push(`${key}: JavaScript module has an unexpected shape; its dynamic suggestions are unavailable`);
    } else {
      entries.push({ key: `js/${key}`, raw: Buffer.from(script) });
      stats.modules += 1;
    }
  }
  if (key.startsWith("js/")) throw new Error(`spec key collides with the module namespace: ${key}`);
  if (!key.includes("/") && typeof spec.description === "string") {
    index[key] = spec.description;
  } else if (!key.includes("/")) {
    index[key] = "";
  }
  stats.converted += 1;
}

entries.push({ key: "__index__", raw: Buffer.from(JSON.stringify(index)) });
entries.sort((a, b) => (a.key < b.key ? -1 : a.key > b.key ? 1 : 0));

// Archive layout (little endian):
//   magic "HZSP1\0"
//   u32 entry count
//   per entry: u16 key length, key bytes, u32 offset, u32 compressed length, u32 raw length
//   compressed blobs; offsets are relative to the start of the blob section
const compressed = entries.map(({ key, raw }) => ({
  key: Buffer.from(key),
  raw,
  data: zstdCompressSync(raw, { params: { [constants.ZSTD_c_compressionLevel]: 19 } }),
}));

const header = [Buffer.from("HZSP1\0")];
const count = Buffer.alloc(4);
count.writeUInt32LE(compressed.length);
header.push(count);
let offset = 0;
for (const entry of compressed) {
  const meta = Buffer.alloc(2 + entry.key.length + 12);
  meta.writeUInt16LE(entry.key.length, 0);
  entry.key.copy(meta, 2);
  meta.writeUInt32LE(offset, 2 + entry.key.length);
  meta.writeUInt32LE(entry.data.length, 6 + entry.key.length);
  meta.writeUInt32LE(entry.raw.length, 10 + entry.key.length);
  header.push(meta);
  offset += entry.data.length;
}

mkdirSync(dirname(outPath), { recursive: true });
writeFileSync(outPath, Buffer.concat([...header, ...compressed.map((e) => e.data)]));

const rawTotal = entries.reduce((sum, e) => sum + e.raw.length, 0);
console.log(`specs converted: ${stats.converted}, skipped: ${stats.skipped.length}`);
for (const line of stats.skipped) console.log(`  skipped ${line}`);
console.log(`generators: ${stats.generators} sites, native-tagged: ${stats.native}, need a function: ${stats.dynamic}`);
console.log(`JavaScript modules stored: ${stats.modules}`);
console.log(`raw JSON: ${(rawTotal / 1e6).toFixed(1)} MB, archive: ${(offset / 1e6).toFixed(1)} MB -> ${outPath}`);
