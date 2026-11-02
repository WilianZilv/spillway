import {Easing, spring, SpringConfig} from 'remotion';
import timeline from '../timeline.json';

export const TL = timeline;
export const FPS = TL.fps;
export const W = 1080;
export const H = 1350;

/** Palette: one accent (blue = the spillway mark). Red/green only for a destination's state. */
export const C = {
  bg: '#0E0F11',
  panel: '#16181B',
  panelHi: '#1E2125',
  line: 'rgba(232,238,246,0.10)',
  lineHi: 'rgba(232,238,246,0.20)',
  ink: '#EEF1F5',
  dim: 'rgba(238,241,245,0.66)',
  muted: 'rgba(238,241,245,0.40)',
  faint: 'rgba(238,241,245,0.14)',
  blue: '#3D8BF2',
  blueHi: '#7DB2FF',
  red: '#E5534B',
  green: '#2FB36B',
};

export const F = {
  serif: '"Instrument Serif", Georgia, serif',
  sans: '"Inter Variable", Inter, system-ui, sans-serif',
  mono: '"JetBrains Mono", ui-monospace, monospace',
};

export const clamp01 = (x: number) => Math.min(1, Math.max(0, x));
export const lerp = (a: number, b: number, k: number) => a + (b - a) * k;
const smooth = (x: number) => {
  const k = clamp01(x);
  return k * k * (3 - 2 * k);
};
/** Smoothstepped 0..1 ramp of x between a and b (derive one animation from another value). */
export const band = (x: number, a: number, b: number) => smooth((x - a) / (b - a));

// Entrances arrive fast and settle; exits leave fast; state moves ease both ways.
export const easeOut = Easing.bezier(0.16, 1, 0.3, 1);
export const easeIn = Easing.bezier(0.6, 0, 0.9, 0.35);
export const easeInOut = Easing.bezier(0.65, 0, 0.35, 1);
export const overshoot = Easing.bezier(0.55, 0, 0.2, 1.12);

/** Tween progress of time t between t0 and t1. */
export const prog = (t: number, t0: number, t1: number, ease: (x: number) => number = easeInOut) =>
  ease(clamp01((t - t0) / (t1 - t0)));

/** Spring starting at `startSec` (0 before it). */
export const sp = (frame: number, startSec: number, config: Partial<SpringConfig> = {}) => {
  const fr = frame - startSec * FPS;
  if (fr <= 0) return 0;
  return spring({frame: fr, fps: FPS, config: {damping: 14, mass: 0.8, stiffness: 140, ...config}});
};

export const POP: Partial<SpringConfig> = {damping: 10, stiffness: 220, mass: 0.7};
export const HEAVY: Partial<SpringConfig> = {damping: 17, stiffness: 170, mass: 0.9};
export const ELASTIC: Partial<SpringConfig> = {damping: 8, stiffness: 200};

/** Deterministic pseudo-random in [0,1). Never Math.random in a render. */
export const rand = (n: number) => {
  const x = Math.sin(n * 127.1 + 311.7) * 43758.5453;
  return x - Math.floor(x);
};

/** Entrance (0→1) × exit (1→0) visibility of a scene element. */
export const life = (t: number, inAt: number, inDur: number, outAt: number, outDur: number) =>
  prog(t, inAt, inAt + inDur, easeOut) * (1 - prog(t, outAt, outAt + outDur, easeIn));
