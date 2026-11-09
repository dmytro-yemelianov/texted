// Monospace grid. Letters sit in cw×ch cells (glyph box 8 wide, centred; baseline at the cell's y).
// A neighbour is always at the same offset, so every ligature is a table entry:
//   h  — next cell in the row            s  — across one empty cell (a space)
//   v  — cell below, same column         dr / dl — cell below, one column right / left
const REL = { h: [0, 1], s: [0, 2], v: [1, 0], dr: [1, 1], dl: [1, -1] };
const REL_CTX = { h: 'word', s: 'words', v: 'lines', dr: 'diag', dl: 'diag' };
const TABLE = new Map();
let GLYPH_W = 8; // drawn width of a glyph; the cell centres it
const cellOrigin = (r, c, cw, ch) => ({ x: c * cw + Math.max(0, cw - GLYPH_W) / 2, y: -r * ch, rot: 0 });

function tableEntry(rel, a, b, cw, ch) {
  const key = rel + a + b + cw + 'x' + ch;
  if (TABLE.has(key)) return TABLE.get(key);
  const [dr, dc] = REL[rel];
  const g = makeGlyph(a, cellOrigin(0, 0, cw, ch), {}), h = makeGlyph(b, cellOrigin(dr, dc, cw, ch), {});
  g.id = 'g'; h.id = 'h';
  const near = [g, h];
  const list = rel === 'h' ? pairCandidates(g, h, near, Math.max(cw - 8 + 2.5, 0.75 * cw))
    : rel === 's' ? wordCandidates(g, h, 2 * cw - 8, near)
    : lineCandidates(g, h, ch, near);
  const side = (G) => (G === g ? 'g' : 'h');
  // One entry per set of connectors: keep the cheapest variant.
  const best = new Map();
  for (const c of list) { const k = c.mech + '|' + c.keys.slice().sort().join(','); if (!best.has(k) || c.cost < best.get(k).cost) best.set(k, c); }
  const out = [...best.values()].map((c) => ({
    mech: c.mech, cost: c.cost, keys: c.keys, pts: c.pts, segs: c.segs,
    trim: c.trim ? [side(c.trim[0]), c.trim[1], c.trim[2]] : null,
    absorb: c.absorb ? [side(c.absorb[0]), c.absorb[1]] : null,
    drop: c.drop ? [side(c.drop[0]), c.drop[1]] : null,
  }));
  TABLE.set(key, out);
  return out;
}

// rows: array of strings. en: { word, words, lines, diag }.
function ligateGrid(rows, cw, ch, en, density, seed) {
  const cells = [], at = new Map();
  rows.forEach((row, r) => [...row].forEach((chr, c) => {
    if (!FONT[chr]) return;
    const g = makeGlyph(chr, cellOrigin(r, c, cw, ch), { r, c });
    g.id = cells.length; cells.push(g); at.set(r + ',' + c, g);
  }));
  const rng = mulberry32(seed * 7919 + 13), cands = [];
  for (const g of cells) for (const [rel, [dr, dc]] of Object.entries(REL)) {
    const ctx = REL_CTX[rel];
    if (!en[ctx]) continue;
    const h = at.get(g.r + dr + ',' + (g.c + dc));
    if (!h || (rel === 's' && at.has(g.r + ',' + (g.c + 1)))) continue;
    const off = [g.c * cw, -g.r * ch], T = (p) => [p[0] + off[0], p[1] + off[1]];
    // One ligature per pair, and per letter at most one on each side: forward, back, up, down.
    const slots = ['pair:' + g.id + ':' + h.id].concat(rel === 'h' || rel === 's' ? [g.id + ':F', h.id + ':B'] : [g.id + ':D', h.id + ':U']);
    // Between two rows, each pair of neighbouring columns (0–1, 2–3, …) carries at most one connection.
    if (LIG.bandPairs && rel !== 'h' && rel !== 's') slots.push('band:' + g.r + ':' + Math.floor(Math.min(g.c, h.c + (rel === 'dl' ? 1 : 0)) / 2));
    for (const e of tableEntry(rel, g.ch, h.ch, cw, ch)) cands.push({
      mech: e.mech, ctx, cost: e.cost, g, h,
      keys: e.keys.map((k) => (k[0] === 'g' ? g.id : h.id) + ':' + k.slice(1)).concat(slots),
      pts: e.pts && e.pts.map(T), segs: e.segs && e.segs.map(([p, q]) => [T(p), T(q)]),
      trim: e.trim && [e.trim[0] === 'g' ? g : h, e.trim[1], Object.assign({}, e.trim[2], { c: T(e.trim[2].c) })],
      absorb: e.absorb && [e.absorb[0] === 'g' ? g : h, e.absorb[1]],
      drop: e.drop && [e.drop[0] === 'g' ? g : h, e.drop[1]],
    });
  }
  const ORDER = { word: 0, words: 1, lines: 2, diag: 3 };
  cands.sort((a, b) => ORDER[a.ctx] - ORDER[b.ctx] || MECH_ORDER[a.mech] - MECH_ORDER[b.mech] || a.cost - b.cost);
  // A table entry only knows its two cells; check the path against the other neighbours too.
  const clearOfOthers = (c) => {
    const pts = c.pts || c.segs.flat();
    if (pts.length < 2) return true;
    const S = pts[0], E = pts[pts.length - 1];
    for (let r = Math.min(c.g.r, c.h.r) - 1; r <= Math.max(c.g.r, c.h.r) + 1; r++)
      for (let k = Math.min(c.g.c, c.h.c) - 1; k <= Math.max(c.g.c, c.h.c) + 1; k++) {
        const o = at.get(r + ',' + k);
        if (!o || o === c.g || o === c.h) continue;
        for (const p of pts) {
          if (dist(p, S) < 0.7 || dist(p, E) < 0.7) continue;
          if (o.prims.some((q) => distPrim(q, p) < 0.4)) return false;
        }
      }
    return true;
  };
  const used = new Set(), out = [];
  for (const c of cands) {
    if (c.keys.some((k) => used.has(k))) continue;
    if (c.ctx !== 'word' && rng() > density) continue;
    if (!clearOfOthers(c)) continue;
    c.keys.forEach((k) => used.add(k));
    out.push(c);
  }
  return { cells, ligs: out };
}

// Fill cols×rows with a word stream, wrapping at word boundaries like an editor.
function fillGrid(words, cols, nrows, seed) {
  const rng = mulberry32(seed), rows = [];
  let k = Math.floor(rng() * words.length), line = '';
  while (rows.length < nrows) {
    const w = words[k++ % words.length];
    if ((line ? line.length + 1 : 0) + w.length > cols) { rows.push(line); line = w; }
    else line = line ? line + ' ' + w : w;
  }
  return rows;
}
