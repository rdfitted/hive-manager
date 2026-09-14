import { afterEach, describe, expect, it, vi } from 'vitest';
import { ReplayGate, decodeBase64, ptyOutputEventName, type PtyOutputEvent } from './ptyStream';

const encoder = new TextEncoder();
const decoder = new TextDecoder();

function toBase64(bytes: Uint8Array): string {
  let binary = '';
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

function b64(text: string): string {
  return toBase64(encoder.encode(text));
}

function chunk(offset: number, text: string): PtyOutputEvent {
  return { id: 'agent', offset, data: b64(text) };
}

function collect(): { sink: (bytes: Uint8Array) => void; text: () => string } {
  const parts: string[] = [];
  return {
    sink: (bytes) => parts.push(decoder.decode(bytes)),
    text: () => parts.join(''),
  };
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('ptyOutputEventName', () => {
  it('matches the backend rule for agent and scratch ids', () => {
    expect(ptyOutputEventName('7c4790a1-370c-4d98-8690-a5fbe4b35e5b-worker-1')).toBe(
      'pty-output:7c4790a1-370c-4d98-8690-a5fbe4b35e5b-worker-1',
    );
    expect(ptyOutputEventName('scratch:7c4790a1:9f1e')).toBe('pty-output:scratch:7c4790a1:9f1e');
  });

  it('maps characters Tauri rejects to underscores exactly like the backend', () => {
    expect(ptyOutputEventName('odd id.1')).toBe('pty-output:odd_id_1');
    expect(ptyOutputEventName('Ω')).toBe('pty-output:_');
  });
});

describe('decodeBase64', () => {
  it('round-trips arbitrary bytes', () => {
    const bytes = Uint8Array.from({ length: 300 }, (_, i) => i % 256);
    expect(decodeBase64(toBase64(bytes))).toEqual(bytes);
    expect(decodeBase64('')).toEqual(new Uint8Array(0));
  });
});

describe('ReplayGate', () => {
  it('queues live chunks until the snapshot lands, then produces a contiguous stream', () => {
    const gate = new ReplayGate('seam');
    const out = collect();

    // Stream: "0123456789" recorded as 5 chunks of two bytes. The snapshot covers 0..6.
    gate.push(chunk(4, '45'), out.sink); // recorded before the snapshot: covered, drop
    gate.push(chunk(6, '67'), out.sink); // recorded after: keep
    expect(gate.isReplaying).toBe(true);
    expect(out.text()).toBe('');

    gate.complete({ id: 'agent', offset_start: 0, offset_end: 6, data: b64('012345') }, out.sink);
    gate.push(chunk(8, '89'), out.sink);

    expect(gate.isReplaying).toBe(false);
    expect(out.text()).toBe('0123456789');
    expect(gate.position).toBe(10);
  });

  it('drops chunks entirely covered by the snapshot without rewinding the position clock', () => {
    const gate = new ReplayGate();
    const out = collect();
    gate.complete({ id: 'agent', offset_start: 0, offset_end: 6, data: b64('abcdef') }, out.sink);

    // Late duplicates from before the snapshot. A filter that merely trims them to an
    // empty write but lets the clock rewind would re-apply the second one.
    gate.push(chunk(2, 'cd'), out.sink);
    expect(gate.position).toBe(6);
    gate.push(chunk(4, 'ef'), out.sink);
    gate.push(chunk(6, 'g'), out.sink);

    expect(out.text()).toBe('abcdefg');
    expect(gate.position).toBe(7);
  });

  it('applies only the uncovered tail of a coalesced chunk that straddles the snapshot end', () => {
    const gate = new ReplayGate();
    const out = collect();
    gate.complete({ id: 'agent', offset_start: 0, offset_end: 4, data: b64('abcd') }, out.sink);

    // The backend merged a chunk recorded before the snapshot (2..4) with one after (4..8).
    gate.push(chunk(2, 'cdefgh'), out.sink);
    expect(out.text()).toBe('abcdefgh');
    expect(gate.position).toBe(8);
  });

  it('goes live without a snapshot and applies queued chunks in order', () => {
    const gate = new ReplayGate();
    const out = collect();
    gate.push(chunk(0, 'he'), out.sink);
    gate.push(chunk(2, 'llo'), out.sink);

    expect(gate.complete(null, out.sink)).toBe(0);
    expect(out.text()).toBe('hello');
    expect(gate.isReplaying).toBe(false);
  });

  it('warns once about a gap and still writes the bytes', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const gate = new ReplayGate('gappy');
    const out = collect();
    gate.complete({ id: 'agent', offset_start: 0, offset_end: 2, data: b64('ab') }, out.sink);

    gate.push(chunk(10, 'xy'), out.sink);
    gate.push(chunk(20, 'z'), out.sink);

    expect(out.text()).toBe('abxyz');
    expect(warn).toHaveBeenCalledTimes(1);
    expect(warn.mock.calls[0][0]).toContain('gappy');
  });

  it('starts the offset clock at the snapshot start when history was evicted', () => {
    const gate = new ReplayGate();
    const out = collect();
    gate.complete({ id: 'agent', offset_start: 1000, offset_end: 1004, data: b64('tail') }, out.sink);

    gate.push(chunk(1004, '!'), out.sink);
    expect(out.text()).toBe('tail!');
  });
});
