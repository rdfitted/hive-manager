export function classifyFinding(name, passed, details, known = {}) {
  const finding = known[name];
  if (!finding) return { passed: !!passed, accepted: !!passed, status: passed ? 'pass' : 'fail' };
  if (!Number.isInteger(finding.issue) || finding.issue <= 0) return { passed: !!passed, accepted: false, status: 'fail', reason: 'Known finding has no issue number' };
  if (passed) return { passed: true, accepted: false, status: 'unexpected-pass', issue: finding.issue, reason: 'Known finding started passing; reconcile the recorded issue instead of silently dropping it' };
  const box = details?.composer, pane = details?.pane;
  const overflow = box && pane && (box.x < pane.x - 1 || box.y < pane.y - 1 || box.right > pane.right + 1 || box.bottom > pane.bottom + 1);
  const expectedCard = name === 'grid-composer-with-attention';
  if (!overflow || box.width <= 0 || box.height <= 0 || details.contained !== false || details.card !== expectedCard) return { passed: false, accepted: false, status: 'fail', issue: finding.issue, reason: 'Failure does not match the recorded composer-overflow finding' };
  return { passed: false, accepted: true, status: 'known-finding', issue: finding.issue };
}
