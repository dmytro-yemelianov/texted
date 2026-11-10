const fs = require('fs');
const strip = (f) => fs.readFileSync(__dirname + '/' + f, 'utf8').replace(/if \(typeof module[^\n]*/, '');
const M = (0, eval)(strip('core.js') + '\n' + strip('font.js') + '\n' + strip('lig.js') + '\n' + strip('grid.js') + '\n;({ tableEntry, ligateGrid, fillGrid, ALPHABETS, REL });');
const cw = 10, ch = 20;
for (const [name, abc] of [['latin', M.ALPHABETS.latinLower], ['cyr', M.ALPHABETS.cyrLower]]) {
  for (const rel of Object.keys(M.REL)) {
    const t = Date.now(); let any = 0; const mech = {};
    for (const a of abc) for (const b of abc) { const e = M.tableEntry(rel, a, b, cw, ch); if (e.length) any++; for (const x of e) mech[x.mech] = (mech[x.mech] || 0) + 1; }
    console.log(name, rel, `${any}/${abc.length ** 2} pairs`, JSON.stringify(mech), (Date.now() - t) + 'ms');
  }
}
const words = 'листопад щастя ступінь пташка вітер плетиво мереживо office fifth staff lattice thrifty after first'.split(' ');
const rows = M.fillGrid(words, 48, 14, 3);
let t = Date.now();
const res = M.ligateGrid(rows, cw, ch, { word: 1, words: 1, lines: 1, diag: 1 }, 0.6, 3);
const c = {}; for (const l of res.ligs) c[l.ctx + ':' + l.mech] = (c[l.ctx + ':' + l.mech] || 0) + 1;
console.log('grid', rows.length + 'x48', res.cells.length, 'letters', res.ligs.length, 'ligs', Date.now() - t, 'ms', JSON.stringify(c));
console.log(['ff','fi','fl','ft','Th','ct','st','sp','ст','тт'].map(p => p + ':' + M.tableEntry('h', p[0], p[1], cw, ch).map(e => e.mech + (e.absorb ? '+dot' : '')).join(',')).join(' | '));
