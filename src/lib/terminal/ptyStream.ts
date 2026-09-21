/**
 * PTY output transport helpers (#287, #289).
 *
 * The backend delivers each agent's output on its own Tauri event name with a base64
 * payload and an absolute byte offset. A freshly mounted pane subscribes first, fetches a
 * snapshot of the retained history, writes it, and then applies live chunks that begin
 * at or after the snapshot's end. `ReplayGate` owns that splice so it can be tested
 * without xterm or Tauri.
 */

/**
 * Event name a pane subscribes to for one agent's output.
 *
 * Mirrors `pty_output_event_name` in `src-tauri/src/pty/manager.rs`; the character rule
 * must stay identical on both sides. Tauri only evaluates an emit in webviews that hold a
 * listener for the exact name, so agents nobody has mounted never reach the webview.
 */
export function ptyOutputEventName(agentId: string): string {
  return `pty-output:${agentId.replace(/[^A-Za-z0-9\-/:_]/g, '_')}`;
}

/** Payload of a `pty-output:<id>` event. */
export interface PtyOutputEvent {
  id: string;
  /** Absolute byte offset of the first byte in `data`. */
  offset: number;
  /** Standard base64 with padding. */
  data: string;
}

/** Result of `get_pty_snapshot`. */
export interface PtySnapshot {
  id: string;
  offset_start: number;
  offset_end: number;
  data: string;
}

export function decodeBase64(data: string): Uint8Array {
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

export type ChunkSink = (bytes: Uint8Array) => void;

/**
 * Splices a pane's history snapshot with its live event stream.
 *
 * Until `complete()` runs, incoming chunks are queued. `complete()` writes the snapshot
 * (if any), then applies every queued chunk through the same offset filter that later
 * live chunks go through: bytes already covered by the snapshot are skipped, and a chunk
 * that straddles the snapshot's end contributes only its uncovered tail. Coalesced events
 * can straddle because the backend merges chunks recorded on either side of the moment
 * the snapshot was taken.
 */
export class ReplayGate {
  private queue: PtyOutputEvent[] | null = [];
  private appliedThrough = 0;
  private warnedAboutGap = false;

  constructor(private readonly label: string = 'terminal') {}

  get isReplaying(): boolean {
    return this.queue !== null;
  }

  /** Bytes applied so far, as an absolute offset. Exposed for tests. */
  get position(): number {
    return this.appliedThrough;
  }

  push(chunk: PtyOutputEvent, sink: ChunkSink): void {
    if (this.queue) {
      this.queue.push(chunk);
      return;
    }
    this.apply(chunk, sink);
  }

  /**
   * Finish replay. A `null` snapshot means no history exists (the PTY was not spawned yet,
   * or was reaped); queued chunks are then applied as-is. Returns the snapshot byte count.
   */
  complete(snapshot: PtySnapshot | null, sink: ChunkSink): number {
    const queued = this.queue ?? [];
    this.queue = null;

    let written = 0;
    if (snapshot) {
      const bytes = decodeBase64(snapshot.data);
      if (bytes.length > 0) {
        sink(bytes);
        written = bytes.length;
      }
      this.appliedThrough = snapshot.offset_end;
    }

    for (const chunk of queued) this.apply(chunk, sink);
    return written;
  }

  private apply(chunk: PtyOutputEvent, sink: ChunkSink): void {
    const bytes = decodeBase64(chunk.data);
    const end = chunk.offset + bytes.length;
    if (end <= this.appliedThrough) return;

    if (chunk.offset > this.appliedThrough && !this.warnedAboutGap) {
      this.warnedAboutGap = true;
      console.warn(
        `[Terminal ${this.label}] output gap: expected offset ${this.appliedThrough}, got ${chunk.offset}`,
      );
    }

    const skip = this.appliedThrough - chunk.offset;
    sink(skip > 0 ? bytes.subarray(skip) : bytes);
    this.appliedThrough = end;
  }
}
