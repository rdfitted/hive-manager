# Synthetic orbital report

This deliberately long synthetic conversation exercises wrapping, scrolling, and structured Markdown in a narrow pane. Every name and measurement here is invented for a local render check.

## Orbital checks

| Station | Result | Next action |
| --- | --- | --- |
| Amber | Ready | Inspect navigation |
| Violet | Waiting | Review synthetic samples |
| Silver | Finished | Record the result |

```javascript
const syntheticOrbit = true;
const stations = ['Amber', 'Violet', 'Silver'];
console.log(stations.map(station => `${station}: ready`));
```

Read the [synthetic reference](https://example.com/synthetic-orbit) for a harmless example link.

Inline HTML must remain literal text: <span data-fixture="literal">synthetic inline HTML</span>

## Observation log

The first synthetic station reports a steady signal. Its coordinator has reviewed the proposed sequence and left a concise explanation so that the next principal can follow the handoff without guessing.

The second station is intentionally quiet. Its heartbeat is older than three minutes, which lets the render check inspect the attention card, composer placement, and sidebar count using the same application data as the other screens.

The third station has finished its assigned work. Its old heartbeat is deliberate: a finished principal should remain finished as the clock advances and should not acquire stale attention merely because it has stopped reporting activity.

The fourth station represents a process that exited during startup. No process is launched to produce this state; the isolated overlay supplies a synthetic exit code for the existing application presentation.

The fifth station represents a failed message delivery. Its attention card gives the reader a concrete action while leaving enough room for the composer at both wide and narrow pane sizes.

## Review notes

- Compare the short navigation file with its recorded synthetic base commit.
- Keep the eight seats visible in Grid.
- Inspect the table, code block, and literal HTML in Focus.
- Open the launch form and settings dialog without submitting changes.

All observations describe fixtures. The remaining paragraphs add enough content to make scrolling meaningful without copying a real conversation, transcript, task, or project.

A synthetic navigation check follows three steps: read the proposed course, compare the station labels, and record the final result. Each step has its own sentence so that line wrapping remains visible in a constrained chat column.

A second pass examines the same labels from a different pane width. The content remains readable when the code block needs horizontal space and the table has several columns. This is useful evidence for the render driver and does not require a live agent.

The final result is intentionally ordinary. Amber is ready, Violet is waiting, and Silver is finished. The fixture leaves the reader with a stable report, a small review diff, and a clear example of every required attention state.
