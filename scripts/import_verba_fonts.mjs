#!/usr/bin/env node
/** Export saved Verba font pixels into Texted's local, bounded bitmap catalog. */
import { readFile, writeFile, mkdir, rename } from "node:fs/promises";
import { resolve, join, dirname } from "node:path";
import { createRequire } from "node:module";
import { createHash } from "node:crypto";
import { pathToFileURL } from "node:url";
import { homedir, tmpdir } from "node:os";
import { mkdtemp, rm } from "node:fs/promises";
const args = process.argv.slice(2),
  option = (name) => {
    const i = args.indexOf(name);
    return i < 0 ? undefined : args[i + 1];
  };
if (!option("--verba"))
  throw Error(
    "Usage: node scripts/import_verba_fonts.mjs --verba CHECKOUT [--project PATH] [--output PATH] [--fonts id,id]",
  );
const verba = resolve(option("--verba")),
  corpus = join(verba, "app"),
  output = resolve(
    option("--output") ??
      join(homedir(), "Library/Application Support/Texted/fonts"),
  );
const sha = (data) => createHash("sha256").update(data).digest("hex");
const catalog = JSON.parse(
  await readFile(join(corpus, "font-corpus/catalog.json"), "utf8"),
);
const project = resolve(
  option("--project") ??
    join(
      homedir(),
      "Library/Application Support/VerbaGlyphEditor/Projects",
      catalog.snapshot,
    ),
);
for (const source of [corpus, project])
  if (
    output === source ||
    output.startsWith(source + "/") ||
    source.startsWith(output + "/")
  )
    throw Error("Font output must not overlap corpus or working drafts");
const selected = option("--fonts")?.split(",");
const fonts = catalog.fonts.filter((f) => !selected || selected.includes(f.id));
if (!fonts.length || fonts.length > 4096) throw Error("Choose 1–4096 fonts");
if (selected?.some((id) => !fonts.some((f) => f.id === id)))
  throw Error("Unknown requested font ID");
const require = createRequire(join(corpus, "package.json")),
  { build } = require("esbuild");
const ua = "АБВГҐДЕЄЖЗИІЇЙКЛМНОПРСТУФХЦЧШЩЬЮЯабвгґдеєжзиіїйклмнопрстуфхцчшщьюя";
function crc32(data) {
  let crc = 0xffffffff;
  for (const byte of data) {
    crc ^= byte;
    for (let b = 0; b < 8; b++) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
  }
  return ~crc >>> 0;
}
function replacement(width,height) {
 const rb=Math.ceil(width/8), bytes=Buffer.alloc(rb*height);
 const left=width>2?1:0,top=height>2?1:0,right=width-1-left,bottom=height-1-top;
 for(let y=top;y<=bottom;y++)for(let x=left;x<=right;x++)if(y===top||y===bottom||x===left||x===right)bytes[y*rb+(x>>3)]|=0x80>>(x%8);
 return bytes;
}

const entries = [];
let bytesTotal = 0;
let previous = [];
try {
  const old = JSON.parse(await readFile(join(output, "library.json"), "utf8"));
  if (
    old.format !== "texted-bitmap-library" ||
    old.schemaVersion !== 1 ||
    !Array.isArray(old.fonts)
  )
    throw Error("Unsupported existing font library");
  previous = old.fonts;
} catch (error) {
  if (error.code !== "ENOENT") throw error;
}

const scratch = await mkdtemp(join(tmpdir(), "texted-verba-import."));
try {
  await build({
    stdin: {
      contents: 'export * from "./ui/model"; export * from "./ui/monospace";',
      resolveDir: join(verba, "desktop/glyph-editor"),
    },
    bundle: true,
    platform: "node",
    format: "esm",
    outfile: join(scratch, "export.mjs"),
  });
  const m = await import(pathToFileURL(join(scratch, "export.mjs")));
  await mkdir(output, { recursive: true });
  for (const font of fonts) {
    const sourceBytes = await readFile(join(corpus, "public", font.url));
    if (sha(sourceBytes) !== font.sha256)
      throw Error("Pinned source changed: " + font.id);
    const record = JSON.parse(sourceBytes);
    let draft = null;
    try {
      draft = JSON.parse(
        await readFile(join(project, "fonts", font.id + ".json"), "utf8"),
      );
    } catch (e) {
      if (e.code !== "ENOENT") throw e;
    }
    if (
      draft &&
      (draft.snapshot !== catalog.snapshot ||
        draft.baseSha256 !== font.sha256 ||
        draft.fontId !== font.id)
    )
      throw Error("Stale Verba draft: " + font.id);
    const effective = draft
      ? m.exportRecord(record, draft)
      : m.exportRecord(record, m.newDraft(font, catalog.snapshot));
    const width = effective.cell.width,
      height = effective.cell.height,
      rowBytes = Math.ceil(width / 8),
      glyphs = new Map(),
      ambiguous = new Set();
    const baseline = Math.max(
      0,
      Math.min(
        height,
        Math.round(
          height +
            (effective.glyphs[0]?.metrics.shiftUp ??
              effective.metrics.baseline - height),
        ),
      ),
    );
    for (const g of effective.glyphs)
      for (const label of g.labels) {
        if (label.kind !== "unicode" || [...label.value].length !== 1) continue;
        const cp = label.value.codePointAt(0);
        if (cp >= 0xd800 && cp <= 0xdfff) continue;
        if (
          g.width !== width ||
          g.height !== height ||
          g.rowBytes !== rowBytes ||
          g.rows.length !== height
        )
          throw Error("Non-monospace export: " + font.id);
        const data = Buffer.concat(
          g.rows.map((row) => {
            if (!new RegExp("^[0-9a-fA-F]{" + rowBytes * 2 + "}$").test(row))
              throw Error("Invalid bitmap row");
            return Buffer.from(row, "hex");
          }),
        );
        if (
          width % 8 &&
          Array.from(
            { length: height },
            (_, y) => data[(y + 1) * rowBytes - 1],
          ).some((v) => v & ((1 << (8 - (width % 8))) - 1))
        )
          throw Error("Nonzero padding bits");
        if (ambiguous.has(cp)) continue;
        if (glyphs.has(cp) && !glyphs.get(cp).equals(data)) {
          glyphs.delete(cp); ambiguous.add(cp); continue;
        }
        glyphs.set(cp, data);
      }
    const latin = [
      ..."ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
    ].filter((c) => glyphs.get(c.codePointAt(0))?.some((v) => v)).length;
    const ukrainian = [...ua].filter((c) =>
      glyphs.get(c.codePointAt(0))?.some((v) => v),
    ).length;
    const duplicates =
      catalog.fonts.filter((f) => f.name === font.name).length > 1;
    const collection = font.id.includes("breadbox")
      ? "Breadbox"
      : font.id.includes("geoworks")
        ? "GeoWorks"
        : font.id.includes("newdeal")
          ? "NewDeal"
          : font.id.slice(-24);
    const entry = {
      id: font.id,
      name: font.name + (duplicates ? " · " + collection : ""),
      width,
      height,
      baseline,
      sourceSnapshot: catalog.snapshot,
      sourceSha256: font.sha256,
      draftRevision: draft?.revision ?? null,
      draftSha256: draft ? sha(JSON.stringify(draft)) : null,
      latin,
      ukrainian,
      license: record.license,
      source: record.source,
      ambiguousUnicode: [...ambiguous].map(
        (cp) => "U+" + cp.toString(16).toUpperCase(),
      ),
      containsGeneratedGlyphs:
        !!draft?.derivations && Object.keys(draft.derivations).length > 0,
    };
    if (
      width < 1 ||
      width > 1022 ||
      height < 1 ||
      height > 1022 ||
      width * height > 262144
    )
      entry.disabledReason = "Native cell exceeds the renderer limit";
    else if (
      ![...glyphs].some(
        ([cp, data]) => cp !== 0xfffd && cp > 32 && data.some((v) => v),
      )
    )
      entry.disabledReason = "No unambiguous mapped Unicode glyphs";
    else {
      if (!glyphs.has(0x20)) glyphs.set(0x20, Buffer.alloc(rowBytes * height));
      if (!glyphs.get(0xfffd)?.some((v) => v))
        glyphs.set(0xfffd, replacement(width, height));
      const chunks = [];
      for (const [cp, data] of [...glyphs].sort((a, b) => a[0] - b[0])) {
        const header = Buffer.alloc(4);
        header.writeUInt32LE(cp);
        chunks.push(header, data);
      }
      const data = Buffer.concat(chunks);
      if (
        glyphs.size > 65536 ||
        data.length > 2 * 1024 * 1024 ||
        bytesTotal + data.length > 64 * 1024 * 1024
      )
        entry.disabledReason = "Bitmap library byte budget exceeded";
      else {
        entry.sha256 = sha(data);
        entry.file = font.id + "-" + entry.sha256.slice(0, 16) + ".bin";
        entry.crc32 = crc32(data);
        entry.glyphs = glyphs.size;
        const dest = join(output, entry.file),
          tmp = dest + ".tmp-" + process.pid;
        await writeFile(tmp, data);
        await rename(tmp, dest);
        bytesTotal += data.length;
      }
    }
    entries.push(entry);
    if (entries.length % 100 === 0)
      console.error(
        `Exported ${entries.length}/${fonts.length}: ${(bytesTotal / 1048576).toFixed(1)} MiB`,
      );
  }
  const replaced = new Set(entries.map((e) => e.id));
  const merged = [
    ...previous.filter((f) => !replaced.has(f.id)),
    ...entries,
  ].sort((a, b) => a.name.localeCompare(b.name, "en", { numeric: true }));
  if (merged.length > 4096)
    throw Error("Merged font library exceeds 4096 faces");
  const manifest =
    JSON.stringify(
      {
        format: "texted-bitmap-library",
        schemaVersion: 1,
        source: "Verba saved drafts",
        fonts: merged,
      },
      null,
      2,
    ) + "\n";
  if (Buffer.byteLength(manifest) > 16 * 1024 * 1024)
    throw Error("Font manifest exceeds 16 MiB");
  const temp = join(output, ".library-" + process.pid + ".tmp");
  await writeFile(temp, manifest);
  await rename(temp, join(output, "library.json"));
  console.log(
    JSON.stringify(
      {
        output,
        updated: entries.length,
        fonts: merged.length,
        ready: merged.filter((f) => !f.disabledReason).length,
        disabled: merged
          .filter((f) => f.disabledReason)
          .map((f) => ({ id: f.id, reason: f.disabledReason })),
        bytes: bytesTotal,
      },
      null,
      2,
    ),
  );
} finally {
  await rm(scratch, { recursive: true, force: true });
}
