// Contact sheets for visual review: one frame per timeline beat (or explicit
// times), tiled with its timestamp so frames can be checked without watching.
//
//   node scripts/sheet.mjs                       beats of timeline.json from out/draft.mp4
//   node scripts/sheet.mjs --video out/x.mp4     another render
//   node scripts/sheet.mjs 3.2 3.4 3.6           exact moments
//   node scripts/sheet.mjs --from 14 --to 16 --every 0.25
import {execFileSync} from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const args = process.argv.slice(2);
const opt = (name, def) => {
  const i = args.indexOf(name);
  return i >= 0 ? args.splice(i, 2)[1] : def;
};
const video = opt('--video', 'out/draft.mp4');
const from = opt('--from');
const to = opt('--to');
const every = opt('--every', '0.25');
const settle = Number(opt('--settle', '0.3'));
const cols = Number(opt('--cols', '4'));
const tl = JSON.parse(fs.readFileSync('timeline.json', 'utf8'));

let times;
if (from !== undefined) {
  times = [];
  for (let x = Number(from); x <= Number(to) + 1e-9; x += Number(every)) times.push(Math.round(x * 1000) / 1000);
} else if (args.length) {
  times = args.map(Number);
} else {
  const beats = new Set([0]);
  const walk = (v) => {
    if (typeof v === 'number') beats.add(Math.round((v + settle) * 1000) / 1000);
    else if (Array.isArray(v)) v.forEach(walk);
    else if (v && typeof v === 'object') Object.values(v).forEach(walk);
  };
  for (const [k, v] of Object.entries(tl)) if (k !== 'fps' && k !== 'duration') walk(v);
  times = [...beats].filter((x) => x < tl.duration).sort((a, b) => a - b);
}

const tmp = 'out/sheets/tmp';
fs.rmSync(tmp, {recursive: true, force: true});
fs.mkdirSync(tmp, {recursive: true});
const font = process.platform === 'win32' ? 'C\\:/Windows/Fonts/consola.ttf' : '/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf';
times.forEach((t, i) => {
  const out = path.join(tmp, `${String(i).padStart(3, '0')}.png`);
  execFileSync('ffmpeg', [
    '-loglevel', 'error', '-y', '-ss', String(t), '-i', video, '-frames:v', '1',
    '-vf', `scale=432:-1,drawtext=fontfile='${font}':text='${t.toFixed(2)}s':x=10:y=10:fontsize=22:fontcolor=white:box=1:boxcolor=black@0.6:boxborderw=6`,
    out,
  ]);
});
const perSheet = cols * 2;
const n = Math.ceil(times.length / perSheet);
const prefix = from !== undefined || args.length ? 'scrub' : 'beats';
for (const f of fs.readdirSync('out/sheets')) if (f.startsWith(prefix)) fs.rmSync(path.join('out/sheets', f));
for (let s = 0; s < n; s++) {
  execFileSync('ffmpeg', [
    '-loglevel', 'error', '-y', '-start_number', String(s * perSheet), '-i', path.join(tmp, '%03d.png'),
    '-frames:v', '1', '-vf', `tile=${cols}x2:padding=6:color=0x333333`, path.join('out/sheets', `${prefix}-${s + 1}.png`),
  ]);
}
console.log(`${times.length} frames → ${n} sheet(s) in out/sheets/${prefix}-*.png`);
