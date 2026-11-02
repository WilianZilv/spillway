// Masters public/sfx-raw.wav to -14 LUFS / -1.5 dBTP (glue compressor + limiter,
// then a two-pass linear loudnorm), muxes it with the silent render and draws a
// waveform with the timeline beats on it for a sync check.
//
//   node scripts/master.mjs [silent.mp4] [out.mp4] [raw.wav] [timeline.json]
import {spawnSync} from 'node:child_process';
import fs from 'node:fs';

const silent = process.argv[2] ?? 'out/spillway-silent.mp4';
const out = process.argv[3] ?? 'out/spillway.mp4';
const raw = process.argv[4] ?? 'public/sfx-raw.wav';
const timeline = process.argv[5] ?? 'timeline.json';
const mastered = raw.replace('-raw', '');

/** Runs ffmpeg, returns stderr (where loudnorm prints its JSON). */
const ff = (args) => {
  const r = spawnSync('ffmpeg', ['-hide_banner', '-y', ...args], {encoding: 'utf8'});
  if (r.status !== 0) throw new Error(r.stderr.slice(-800));
  return r.stderr;
};
const loudness = (file, pre = '') => {
  const e = ff(['-i', file, '-af', `${pre}loudnorm=I=-14:TP=-1.5:LRA=11:print_format=json`, '-f', 'null', '-']);
  return JSON.parse(e.slice(e.lastIndexOf('{'), e.lastIndexOf('}') + 1));
};

const glue = 'acompressor=threshold=-20dB:ratio=2.5:attack=8:release=150:makeup=2,alimiter=limit=0.85:level=false';
const m = loudness(raw, `${glue},`);
console.log(`pass 1: ${m.input_i} LUFS, ${m.input_tp} dBTP`);
const ln = `loudnorm=I=-14:TP=-1.5:LRA=11:measured_I=${m.input_i}:measured_TP=${m.input_tp}:measured_LRA=${m.input_lra}:measured_thresh=${m.input_thresh}:offset=${m.target_offset}:linear=true`;
ff(['-i', raw, '-af', `${glue},${ln}`, '-ar', '48000', mastered]);
const c = loudness(mastered);
console.log(`master: ${c.input_i} LUFS, true peak ${c.input_tp} dBTP`);

if (fs.existsSync(silent)) {
  ff(['-i', silent, '-i', mastered, '-map', '0:v', '-map', '1:a', '-c:v', 'copy', '-c:a', 'aac', '-b:a', '256k', '-movflags', '+faststart', '-shortest', out]);
  const o = loudness(out);
  console.log(`${out}: ${o.input_i} LUFS, true peak ${o.input_tp} dBTP`);
}

// waveform with the timeline beats drawn on it
const tl = JSON.parse(fs.readFileSync(timeline, 'utf8'));
const beats = [];
const walk = (v) => (typeof v === 'number' ? beats.push(v) : Array.isArray(v) ? v.forEach(walk) : v && typeof v === 'object' ? Object.values(v).forEach(walk) : 0);
for (const [k, v] of Object.entries(tl)) if (k !== 'fps' && k !== 'duration') walk(v);
const W = 2400;
const marks = beats.map((b) => `drawbox=x=${Math.round((b / tl.duration) * W)}:y=0:w=2:h=400:color=0xE5534B@0.7:t=fill`).join(',');
ff(['-i', mastered, '-filter_complex', `showwavespic=s=${W}x400:split_channels=0:colors=0x7DB2FF,${marks}`, '-frames:v', '1', out.replace(/\.mp4$/, '-wave.png')]);
console.log(`waveform → ${out.replace(/\.mp4$/, '-wave.png')}`);
