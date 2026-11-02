import React from 'react';
import {C, F, clamp01, lerp} from '../lib';
import {Chip} from './primitives';

/** Camera tile with a blinking REC dot. */
export const CameraTile: React.FC<{x: number; y: number; k: number; t: number; seed: number}> = ({x, y, k, t, seed}) => {
  const rec = Math.floor(t * 2 + seed * 3) % 2 === 0;
  return (
    <div
      style={{
        position: 'absolute',
        left: x,
        top: y + (1 - k) * 30,
        width: 118,
        height: 80,
        borderRadius: 14,
        background: C.panel,
        border: `1.5px solid ${C.line}`,
        opacity: k,
        transform: `scale(${0.85 + 0.15 * k})`,
      }}
    >
      <div style={{position: 'absolute', left: 14, top: 22, width: 36, height: 36, borderRadius: 18, border: `3px solid ${C.faint}`}} />
      <div style={{position: 'absolute', left: 25, top: 33, width: 14, height: 14, borderRadius: 7, background: C.panelHi}} />
      <div style={{position: 'absolute', right: 14, top: 14, width: 10, height: 10, borderRadius: 5, background: C.red, opacity: rec ? 1 : 0.25}} />
      <div style={{position: 'absolute', right: 14, bottom: 14, width: 34, height: 6, borderRadius: 3, background: C.faint}} />
    </div>
  );
};

export type DiskState = 'ok' | 'fail' | 'back';

/** Destination disk card: name, capacity, usage bar, live state. */
export const DiskCard: React.FC<{
  cx: number;
  y: number;
  k: number;
  name: string;
  cap: string;
  use: number;
  state: DiskState;
  stateK: number;
  share?: string;
  shareK?: number;
  shake?: number;
  remote?: boolean;
}> = ({cx, y, k, name, cap, use, state, stateK, share, shareK = 0, shake = 0, remote}) => {
  const w = 280;
  const border = state === 'fail' ? `rgba(229,83,75,${0.25 + 0.75 * stateK})` : state === 'back' ? `rgba(47,179,107,${0.9 * stateK})` : C.lineHi;
  const status = state === 'fail' ? 'OFFLINE' : state === 'back' ? 'ONLINE' : 'GRAVANDO';
  const statusColor = state === 'fail' ? C.red : state === 'back' ? C.green : C.muted;
  return (
    <div
      style={{
        position: 'absolute',
        left: cx - w / 2 + shake,
        top: y + (1 - k) * 60,
        width: w,
        height: 200,
        borderRadius: 22,
        background: C.panel,
        border: `2px solid ${border}`,
        opacity: k,
        padding: '22px 24px',
        boxSizing: 'border-box',
        boxShadow: state === 'fail' ? `0 0 ${50 * stateK}px rgba(229,83,75,${0.25 * stateK})` : 'none',
      }}
    >
      <div style={{display: 'flex', alignItems: 'center', gap: 12}}>
        <DiskIcon remote={remote} color={state === 'fail' ? C.red : C.dim} />
        <div style={{fontFamily: F.sans, fontWeight: 600, fontSize: 28, color: C.ink, letterSpacing: '-0.01em'}}>{name}</div>
      </div>
      <div style={{fontFamily: F.mono, fontSize: 18, color: C.muted, marginTop: 8, letterSpacing: '0.08em'}}>{cap}</div>
      <div style={{position: 'relative', height: 10, borderRadius: 5, background: C.faint, marginTop: 22, overflow: 'hidden'}}>
        <div style={{position: 'absolute', inset: 0, width: `${use * 100}%`, borderRadius: 5, background: state === 'fail' ? 'rgba(229,83,75,0.55)' : C.blue}} />
      </div>
      <div style={{display: 'flex', justifyContent: 'space-between', marginTop: 18, fontFamily: F.mono, fontSize: 17, letterSpacing: '0.16em'}}>
        <span style={{color: statusColor}}>{status}</span>
        {share && (
          <span style={{color: C.ink, opacity: shareK, transform: `translateY(${(1 - shareK) * 8}px)`, display: 'inline-block'}}>{share}</span>
        )}
      </div>
    </div>
  );
};

const DiskIcon: React.FC<{remote?: boolean; color: string}> = ({remote, color}) =>
  remote ? (
    <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <rect width="20" height="8" x="2" y="2" rx="2" />
      <rect width="20" height="8" x="2" y="14" rx="2" />
      <line x1="6" x2="6.01" y1="6" y2="6" />
      <line x1="6" x2="6.01" y1="18" y2="18" />
    </svg>
  ) : (
    <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <line x1="22" x2="2" y1="12" y2="12" />
      <path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z" />
      <line x1="6" x2="6.01" y1="16" y2="16" />
      <line x1="10" x2="10.01" y1="16" y2="16" />
    </svg>
  );

export const RAM_SLOTS = 10;

/**
 * The RAM block: slots filled with recording chips (oldest on the left).
 * `fill` 0..1 is how full it is; `evict` 0..1 animates the thief taking the
 * oldest `evictN` chips out of the left side.
 */
export const RamBlock: React.FC<{
  cx: number;
  y: number;
  k: number;
  fill: number;
  ceiling: number;
  resume: number;
  markersK: number;
  hot: number;
  evict: number;
  evictN: number;
  /** 0..1: after eviction the remaining chips slide left into the freed slots. */
  shift: number;
  flame: number;
}> = ({cx, y, k, fill, ceiling, resume, markersK, hot, evict, evictN, shift, flame}) => {
  const w = 760;
  const h = 170;
  const left = cx - w / 2;
  const slotW = 70;
  const x0 = 40;
  const filled = fill * RAM_SLOTS;
  return (
    <div style={{position: 'absolute', left, top: y + (1 - k) * 50, width: w, height: h, opacity: k}}>
      <div
        style={{
          position: 'absolute',
          inset: 0,
          borderRadius: 26,
          background: C.panel,
          border: `2px solid ${hot > 0 ? `rgba(229,83,75,${0.3 + 0.6 * hot})` : 'rgba(61,139,242,0.55)'}`,
          boxShadow: `0 0 70px rgba(61,139,242,${0.12 * (1 - hot)}), 0 0 60px rgba(229,83,75,${0.22 * hot})`,
        }}
      />
      {/* memory-stick notches */}
      {[0, 1, 2, 3, 4, 5, 6, 7].map((i) => (
        <div key={i} style={{position: 'absolute', left: 70 + i * 92, bottom: -14, width: 34, height: 14, borderRadius: '0 0 6px 6px', background: C.panelHi, border: `1.5px solid ${C.line}`, borderTop: 'none'}} />
      ))}
      <div style={{position: 'absolute', left: 40, top: 20, fontFamily: F.mono, fontSize: 18, letterSpacing: '0.2em', color: C.muted}}>RAM · TMPFS</div>
      {Array.from({length: RAM_SLOTS}, (_, i) => {
        const on = clamp01(filled - i);
        if (on <= 0.001) return null;
        const gone = i < evictN ? clamp01(evict * evictN - i) : 0;
        return (
          <Chip
            key={i}
            x={x0 + 35 + (i - (i >= evictN ? shift * evictN : 0)) * slotW}
            y={100 + gone * 150}
            s={1}
            o={on * (1 - gone)}
            tone={gone > 0 ? 'red' : 'blue'}
            rot={gone * 30}
          />
        );
      })}
      {/* thresholds */}
      {[{v: resume, label: 'retoma', strong: false}, {v: ceiling, label: 'teto', strong: true}].map((m) => (
        <div key={m.label} style={{position: 'absolute', left: x0 + m.v * RAM_SLOTS * slotW - 1, top: 52, height: 96, width: 2, background: m.strong ? C.ink : C.muted, opacity: markersK}}>
          <div style={{position: 'absolute', top: -30, left: -40, width: 80, textAlign: 'center', fontFamily: F.mono, fontSize: 15, letterSpacing: '0.14em', color: m.strong ? C.ink : C.muted}}>
            {m.label.toUpperCase()}
          </div>
        </div>
      ))}
      {flame > 0 && (
        <div style={{position: 'absolute', left: x0 + ceiling * RAM_SLOTS * slotW - 32, top: -112, opacity: flame, transform: `scale(${0.6 + 0.4 * flame})`, transformOrigin: '50% 100%'}}>
          <svg width="64" height="64" viewBox="0 0 24 24" fill="none" stroke={C.red} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M8.5 14.5A2.5 2.5 0 0 0 11 12c0-1.38-.5-2-1-3-1.072-2.143-.224-4.054 2-6 .5 2.5 2 4.9 4 6.5 2 1.6 3 3.5 3 5.5a7 7 0 1 1-14 0c0-1.153.433-2.294 1-3a2.5 2.5 0 0 0 2.5 2.5z" />
          </svg>
        </div>
      )}
    </div>
  );
};

/** Quadratic bezier point. */
export const qb = (p0: [number, number], p1: [number, number], p2: [number, number], k: number): [number, number] => [
  lerp(lerp(p0[0], p1[0], k), lerp(p1[0], p2[0], k), k),
  lerp(lerp(p0[1], p1[1], k), lerp(p1[1], p2[1], k), k),
];
